use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::{ConfigStore, PinnedForward};
use crate::error::{NettleError, Result};
use crate::ipc::types::ForwardInfo;
use crate::ssh::EpochRx;
use crate::state::UiBridge;

/// Grace period for holding an accepted local connection while the remote
/// process (or the SSH link) comes back.
const WAIT_GRACE: Duration = Duration::from_secs(20);

struct Entry {
    local_port: u16,
    pinned: bool,
    stop: CancellationToken,
}

/// Local tunnels to remote ports. The core invariant: a forward's TcpListener
/// is bound once and survives SSH reconnects and remote process restarts —
/// only the user removing the forward tears it down.
pub struct ForwardManager {
    ui: Arc<UiBridge>,
    host_id: Uuid,
    store: ConfigStore,
    epoch_rx: EpochRx,
    ports_live_rx: watch::Receiver<HashSet<u16>>,
    entries: StdMutex<HashMap<u16, Entry>>,
}

impl ForwardManager {
    pub fn new(
        ui: Arc<UiBridge>,
        host_id: Uuid,
        store: ConfigStore,
        epoch_rx: EpochRx,
        ports_live_rx: watch::Receiver<HashSet<u16>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            ui,
            host_id,
            store,
            epoch_rx,
            ports_live_rx,
            entries: StdMutex::new(HashMap::new()),
        })
    }

    pub fn list(&self) -> Vec<ForwardInfo> {
        let live = self.ports_live_rx.borrow().clone();
        let mut infos: Vec<ForwardInfo> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .map(|(port, e)| ForwardInfo {
                port: *port,
                local_port: e.local_port,
                pinned: e.pinned,
                live: live.contains(port),
            })
            .collect();
        infos.sort_by_key(|f| f.port);
        infos
    }

    fn broadcast(&self) {
        self.ui.emit_forwards(&crate::ipc::types::ForwardsChanged {
            host_id: self.host_id,
            forwards: self.list(),
        });
    }

    /// Called by the scanner after each scan.
    pub fn on_ports_update(&self, _live: &HashSet<u16>) {
        self.broadcast();
    }

    pub async fn set(self: &Arc<Self>, port: u16, enabled: bool, pinned: bool) -> Result<()> {
        self.set_with_local(port, port, enabled, pinned).await
    }

    /// Like `set`, but with an explicit local bind port — lets the user tunnel
    /// a remote port to a *different* local port (e.g. remote 8080 →
    /// localhost:9090). A `local_port` of 0 means "same as the remote port".
    pub async fn set_with_local(
        self: &Arc<Self>,
        port: u16,
        local_port: u16,
        enabled: bool,
        pinned: bool,
    ) -> Result<()> {
        let local_port = if local_port == 0 { port } else { local_port };

        if !enabled {
            let entry = self.entries.lock().unwrap().remove(&port);
            if let Some(entry) = entry {
                entry.stop.cancel();
            }
            self.persist_pin(port, 0, false).await;
            self.broadcast();
            return Ok(());
        }

        // If the forward already exists on the same local port, just update the
        // pin flag. If the local port changed, fall through to rebind it.
        let rebind = {
            let mut entries = self.entries.lock().unwrap();
            match entries.get_mut(&port) {
                Some(entry) if entry.local_port == local_port => {
                    entry.pinned = pinned;
                    false
                }
                Some(_) => true, // local port changed → tear down and rebind
                None => true,    // new forward
            }
        };
        if !rebind {
            self.persist_pin(port, local_port, pinned).await;
            self.broadcast();
            return Ok(());
        }

        // Drop any existing binding for this remote port before re-binding.
        if let Some(old) = self.entries.lock().unwrap().remove(&port) {
            old.stop.cancel();
        }

        let listener = TcpListener::bind(("127.0.0.1", local_port))
            .await
            .map_err(|e| NettleError::Msg(format!("cannot bind localhost:{local_port}: {e}")))?;
        let stop = CancellationToken::new();
        self.entries.lock().unwrap().insert(
            port,
            Entry {
                local_port,
                pinned,
                stop: stop.clone(),
            },
        );
        self.persist_pin(port, local_port, pinned).await;

        let mgr = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    accepted = listener.accept() => {
                        let Ok((sock, peer)) = accepted else { break };
                        let epoch_rx = mgr.epoch_rx.clone();
                        let live_rx = mgr.ports_live_rx.clone();
                        let conn_stop = stop.child_token();
                        tokio::spawn(async move {
                            let _ = proxy(sock, peer, port, epoch_rx, live_rx, conn_stop).await;
                        });
                    }
                }
            }
        });

        self.broadcast();
        Ok(())
    }

    pub fn shutdown(&self) {
        let mut entries = self.entries.lock().unwrap();
        for (_, entry) in entries.drain() {
            entry.stop.cancel();
        }
    }

    async fn persist_pin(&self, port: u16, local_port: u16, pinned: bool) {
        let host_id = self.host_id;
        let _ = self
            .store
            .update_state(move |s| {
                s.pinned_forwards
                    .retain(|p| !(p.host_id == host_id && p.port == port));
                if pinned {
                    s.pinned_forwards.push(PinnedForward {
                        host_id,
                        port,
                        local_port,
                    });
                }
            })
            .await;
    }
}

/// Proxy one accepted local connection through a direct-tcpip channel.
async fn proxy(
    mut sock: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    port: u16,
    mut epoch_rx: EpochRx,
    mut live_rx: watch::Receiver<HashSet<u16>>,
    stop: CancellationToken,
) -> Result<()> {
    // Wait (bounded) for a live epoch AND the remote port to be listening.
    let deadline = tokio::time::Instant::now() + WAIT_GRACE;
    let epoch = loop {
        let ready_epoch = epoch_rx.borrow().clone();
        let port_live = live_rx.borrow().contains(&port);
        if let Some(e) = ready_epoch {
            if port_live {
                break e;
            }
        }
        tokio::select! {
            _ = stop.cancelled() => return Ok(()),
            _ = tokio::time::sleep_until(deadline) => return Ok(()),
            r = epoch_rx.changed() => { if r.is_err() { return Ok(()); } }
            r = live_rx.changed() => { if r.is_err() { return Ok(()); } }
        }
    };

    let channel = epoch
        .handle
        .channel_open_direct_tcpip(
            "127.0.0.1",
            port as u32,
            peer.ip().to_string(),
            peer.port() as u32,
        )
        .await?;
    let mut stream = channel.into_stream();

    tokio::select! {
        _ = stop.cancelled() => {}
        _ = epoch.cancel.cancelled() => {}
        _ = tokio::io::copy_bidirectional(&mut sock, &mut stream) => {}
    }
    Ok(())
}
