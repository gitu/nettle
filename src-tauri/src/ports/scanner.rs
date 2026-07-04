use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tauri::AppHandle;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::ipc::events::emit_ports;
use crate::ipc::types::{PortsChanged, RemotePort};
use crate::ports::forwards::ForwardManager;
use crate::ports::parse;
use crate::ssh::session::SessionCmd;
use crate::ssh::{exec_capture, ConnectionEpoch, EpochRx};

const SCAN_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, PartialEq)]
enum Method {
    Ss,
    Netstat,
    ProcNet,
    Unsupported,
}

/// Periodically list listening TCP ports on the remote, diff, and fan out:
/// `ports-changed` events to the UI and the live-port set to the ForwardManager.
pub fn spawn(
    app: AppHandle,
    mut epoch_rx: EpochRx,
    forwards: Arc<ForwardManager>,
    ports_live_tx: watch::Sender<HashSet<u16>>,
    ignored: Arc<std::sync::Mutex<HashSet<u16>>>,
    session_cmd: mpsc::UnboundedSender<SessionCmd>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            // Wait for a live epoch.
            let epoch: Arc<ConnectionEpoch> = loop {
                if let Some(e) = epoch_rx.borrow().clone() {
                    break e;
                }
                if epoch_rx.changed().await.is_err() {
                    return;
                }
            };

            let mut method: Option<Method> = None;
            let mut prev: HashSet<u16> = HashSet::new();
            let mut prev_rows: Vec<RemotePort> = Vec::new();
            let mut baseline = true;

            loop {
                let scan = tokio::select! {
                    _ = epoch.cancel.cancelled() => break,
                    r = scan_once(&epoch, &mut method) => r,
                };
                match scan {
                    Ok(rows) => {
                        let deduped = parse::dedupe_by_port(rows);
                        let ports_set: HashSet<u16> = deduped.iter().map(|p| p.port).collect();
                        let ignored_now = ignored.lock().unwrap().clone();

                        let added: Vec<RemotePort> = deduped
                            .iter()
                            .filter(|p| !prev.contains(&p.port) && !ignored_now.contains(&p.port))
                            .cloned()
                            .collect();
                        let removed: Vec<u16> =
                            prev.iter().filter(|p| !ports_set.contains(p)).copied().collect();

                        let changed = deduped != prev_rows;
                        if baseline || changed {
                            emit_ports(
                                &app,
                                &PortsChanged {
                                    all: deduped.clone(),
                                    added: if baseline { Vec::new() } else { added },
                                    removed,
                                    is_baseline: baseline,
                                    unsupported: method == Some(Method::Unsupported),
                                },
                            );
                            let _ = ports_live_tx.send(ports_set.clone());
                            forwards.on_ports_update(&ports_set);
                        }
                        prev = ports_set;
                        prev_rows = deduped;
                        baseline = false;
                    }
                    Err(_) => {
                        // exec failed — the connection is probably dead.
                        let _ = session_cmd.send(SessionCmd::SuspectDead(epoch.id));
                        break;
                    }
                }

                tokio::select! {
                    _ = epoch.cancel.cancelled() => break,
                    _ = tokio::time::sleep(SCAN_INTERVAL) => {}
                }
            }

            // Epoch died; wait for the next one.
            if epoch_rx.changed().await.is_err() {
                return;
            }
        }
    })
}

async fn scan_once(
    epoch: &ConnectionEpoch,
    method: &mut Option<Method>,
) -> crate::error::Result<Vec<RemotePort>> {
    if method.is_none() {
        *method = Some(detect_method(epoch).await?);
    }
    match method.unwrap() {
        Method::Ss => {
            let (out, _) = exec_capture(&epoch.handle, "ss -tlnp 2>/dev/null").await?;
            Ok(parse::parse_ss(&out))
        }
        Method::Netstat => {
            let (out, _) = exec_capture(&epoch.handle, "netstat -tlnp 2>/dev/null").await?;
            Ok(parse::parse_netstat(&out))
        }
        Method::ProcNet => {
            let (out, _) =
                exec_capture(&epoch.handle, "cat /proc/net/tcp /proc/net/tcp6 2>/dev/null").await?;
            Ok(parse::parse_proc_net(&out))
        }
        Method::Unsupported => Ok(Vec::new()),
    }
}

async fn detect_method(epoch: &ConnectionEpoch) -> crate::error::Result<Method> {
    let (out, exit) = exec_capture(&epoch.handle, "ss -tlnp 2>/dev/null").await?;
    if exit == Some(0) && !parse::parse_ss(&out).is_empty() || out.contains("State") {
        return Ok(Method::Ss);
    }
    let (out, exit) = exec_capture(&epoch.handle, "netstat -tlnp 2>/dev/null").await?;
    if exit == Some(0) && (out.contains("Proto") || !parse::parse_netstat(&out).is_empty()) {
        return Ok(Method::Netstat);
    }
    let (out, _) =
        exec_capture(&epoch.handle, "cat /proc/net/tcp /proc/net/tcp6 2>/dev/null").await?;
    if out.contains("local_address") {
        return Ok(Method::ProcNet);
    }
    Ok(Method::Unsupported)
}
