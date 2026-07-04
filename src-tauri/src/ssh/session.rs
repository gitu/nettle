use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use russh::client;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::config::HostConfig;
use crate::error::{NettleError, Result};
use crate::ipc::types::ConnState;
use crate::ssh::auth::{self, SecretCache};
use crate::ssh::handler::ClientHandler;
use crate::ssh::{dns, now_ms, ConnectionEpoch, EpochRx, EpochTx};
use crate::state::UiBridge;

#[derive(Debug)]
pub enum SessionCmd {
    Disconnect,
    /// A subsystem hit an I/O error on this epoch — verify and reconnect.
    SuspectDead(u64),
}

pub struct SessionActor;

impl SessionActor {
    pub fn spawn(
        ui: Arc<UiBridge>,
        host: HostConfig,
        known_hosts_path: PathBuf,
    ) -> (mpsc::UnboundedSender<SessionCmd>, EpochRx, JoinHandle<()>) {
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let (epoch_tx, epoch_rx) = watch::channel(None);
        let task = tokio::spawn(Self::run(ui, host, known_hosts_path, cmd_rx, epoch_tx));
        (cmd_tx, epoch_rx, task)
    }

    async fn run(
        ui: Arc<UiBridge>,
        host: HostConfig,
        known_hosts_path: PathBuf,
        mut cmd_rx: mpsc::UnboundedReceiver<SessionCmd>,
        epoch_tx: EpochTx,
    ) {
        let mut cache = SecretCache::default();
        let mut epoch_id: u64 = 0;
        let mut ever_connected = false;
        let mut attempt: u32 = 0;

        let config = Arc::new(client::Config {
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 3,
            ..Default::default()
        });

        'main: loop {
            ui.emit_conn(if ever_connected {
                ConnState::Reconnecting {
                    host_id: host.id,
                    attempt,
                    next_retry_ms: None,
                }
            } else {
                ConnState::Connecting { host_id: host.id }
            });

            let (death_tx, mut death_rx) = mpsc::unbounded_channel();
            let interactive = !ever_connected;

            match Self::try_connect(
                &ui,
                &host,
                &known_hosts_path,
                config.clone(),
                &mut cache,
                interactive,
                death_tx,
            )
            .await
            {
                Ok((handle, ip)) => {
                    ever_connected = true;
                    attempt = 0;
                    epoch_id += 1;
                    let cancel = CancellationToken::new();
                    let epoch = Arc::new(ConnectionEpoch {
                        id: epoch_id,
                        handle,
                        cancel: cancel.clone(),
                        resolved_ip: ip,
                        connected_at_ms: now_ms(),
                    });
                    let _ = epoch_tx.send(Some(epoch.clone()));
                    ui.emit_conn(ConnState::Connected {
                        host_id: host.id,
                        ip: ip.to_string(),
                        since_ms: epoch.connected_at_ms,
                        epoch: epoch_id,
                    });

                    // Supervise until the connection dies or the user disconnects.
                    loop {
                        tokio::select! {
                            cmd = cmd_rx.recv() => match cmd {
                                Some(SessionCmd::Disconnect) | None => {
                                    cancel.cancel();
                                    let _ = epoch_tx.send(None);
                                    let _ = epoch
                                        .handle
                                        .disconnect(russh::Disconnect::ByApplication, "", "en")
                                        .await;
                                    ui.emit_conn(ConnState::Disconnected);
                                    break 'main;
                                }
                                Some(SessionCmd::SuspectDead(id)) if id == epoch_id => break,
                                Some(SessionCmd::SuspectDead(_)) => {}
                            },
                            _ = death_rx.recv() => break,
                        }
                    }
                    // Connection died — tear down this epoch, fall through to reconnect.
                    cancel.cancel();
                    let _ = epoch_tx.send(None);
                }
                Err(err) => {
                    if !ever_connected {
                        // Initial connect failed: report and stop; the user retries explicitly.
                        ui.emit_conn(ConnState::Failed {
                            host_id: host.id,
                            error: err.to_string(),
                        });
                        break 'main;
                    }
                }
            }

            attempt += 1;
            let delay = dns::backoff_delay(attempt);
            ui.emit_conn(ConnState::Reconnecting {
                host_id: host.id,
                attempt,
                next_retry_ms: Some(delay.as_millis() as u64),
            });
            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                cmd = cmd_rx.recv() => {
                    if matches!(cmd, Some(SessionCmd::Disconnect) | None) {
                        ui.emit_conn(ConnState::Disconnected);
                        break 'main;
                    }
                }
            }
        }
    }

    async fn try_connect(
        ui: &Arc<UiBridge>,
        host: &HostConfig,
        known_hosts_path: &std::path::Path,
        config: Arc<client::Config>,
        cache: &mut SecretCache,
        interactive: bool,
        death_tx: mpsc::UnboundedSender<String>,
    ) -> Result<(client::Handle<ClientHandler>, IpAddr)> {
        let addrs = dns::resolve(&host.hostname, host.port).await?;
        let mut last_err = NettleError::Timeout;
        for addr in addrs {
            let handler = ClientHandler {
                ui: ui.clone(),
                hostname: host.hostname.clone(),
                port: host.port,
                known_hosts_path: known_hosts_path.to_path_buf(),
                prompt_allowed: interactive,
                death_tx: death_tx.clone(),
            };
            // Interactive connects may park on the host-key prompt (60s budget).
            let timeout = if interactive { 75 } else { 10 };
            match tokio::time::timeout(
                Duration::from_secs(timeout),
                client::connect(config.clone(), addr, handler),
            )
            .await
            {
                Ok(Ok(mut handle)) => {
                    if interactive {
                        ui.emit_conn(ConnState::Authenticating { host_id: host.id });
                    }
                    auth::authenticate(&mut handle, host, ui, cache, interactive).await?;
                    return Ok((handle, addr.ip()));
                }
                Ok(Err(e)) => last_err = e.into(),
                Err(_) => last_err = NettleError::Timeout,
            }
        }
        Err(last_err)
    }
}
