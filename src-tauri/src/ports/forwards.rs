use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use russh::ChannelMsg;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::{ConfigStore, PinnedForward};
use crate::error::{NettleError, Result};
use crate::ipc::types::{ForwardInfo, LogLevel};
use crate::ports::sandbox::{self, SandboxPorts};
use crate::ssh::EpochRx;
use crate::state::UiBridge;
use crate::stats::{ActiveTunnelGuard, HostStats};

/// Grace period for holding an accepted local connection while the remote
/// process (or the SSH link) comes back.
const WAIT_GRACE: Duration = Duration::from_secs(20);
/// After one side of a tunnelled connection hit EOF, how long the other side
/// may keep it open before we tear the whole thing down.
const HALF_CLOSE_LINGER: Duration = Duration::from_secs(30);
/// Pause after a failed accept() before trying again.
const ACCEPT_RETRY: Duration = Duration::from_millis(250);
/// Read chunk for local → remote; matches the usual SSH max packet payload.
const PROXY_BUF: usize = 32 * 1024;

/// Where a tunnel leads: a port on the host itself, or a port inside one of
/// the host's Docker sandboxes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Target {
    pub sandbox: Option<String>,
    pub port: u16,
}

impl Target {
    pub fn host(port: u16) -> Self {
        Self {
            sandbox: None,
            port,
        }
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.sandbox {
            Some(name) => write!(f, "sandbox {name}:{}", self.port),
            None => write!(f, "remote {}", self.port),
        }
    }
}

struct Entry {
    target: Target,
    pinned: bool,
    /// Lifetime of the local listener.
    stop: CancellationToken,
    /// Lifetime of the connections accepted for the *current* target (a child
    /// of `stop`). Switching the target cancels it, so open connections to the
    /// old target end and clients reconnect to the new one.
    conns: CancellationToken,
}

/// Local tunnels to remote ports. The core invariant: a forward's TcpListener
/// is bound once and survives SSH reconnects and remote process restarts —
/// only the user removing the forward tears it down. Entries are keyed by the
/// local port, so pointing a busy local port at another target (e.g. moving
/// localhost:4180 from sandbox A to sandbox B) retargets the listener in place.
pub struct ForwardManager {
    ui: Arc<UiBridge>,
    host_id: Uuid,
    store: ConfigStore,
    epoch_rx: EpochRx,
    ports_live_rx: watch::Receiver<HashSet<u16>>,
    sandbox_live_rx: watch::Receiver<SandboxPorts>,
    entries: StdMutex<HashMap<u16, Entry>>,
}

impl ForwardManager {
    pub fn new(
        ui: Arc<UiBridge>,
        host_id: Uuid,
        store: ConfigStore,
        epoch_rx: EpochRx,
        ports_live_rx: watch::Receiver<HashSet<u16>>,
        sandbox_live_rx: watch::Receiver<SandboxPorts>,
    ) -> Arc<Self> {
        Arc::new(Self {
            ui,
            host_id,
            store,
            epoch_rx,
            ports_live_rx,
            sandbox_live_rx,
            entries: StdMutex::new(HashMap::new()),
        })
    }

    pub fn list(&self) -> Vec<ForwardInfo> {
        let mut infos: Vec<ForwardInfo> = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .map(|(local_port, e)| ForwardInfo {
                port: e.target.port,
                local_port: *local_port,
                pinned: e.pinned,
                live: self.is_live(&e.target),
                sandbox: e.target.sandbox.clone(),
            })
            .collect();
        infos.sort_by(|a, b| {
            a.sandbox
                .cmp(&b.sandbox)
                .then(a.port.cmp(&b.port))
                .then(a.local_port.cmp(&b.local_port))
        });
        infos
    }

    /// The target's process is listening right now (per the last scan).
    fn is_live(&self, target: &Target) -> bool {
        match &target.sandbox {
            None => self.ports_live_rx.borrow().contains(&target.port),
            Some(name) => self
                .sandbox_live_rx
                .borrow()
                .get(name)
                .is_some_and(|ports| ports.contains_key(&target.port)),
        }
    }

    /// `target` is forwarded (to any local port).
    pub fn has_target(&self, target: &Target) -> bool {
        self.entries
            .lock()
            .unwrap()
            .values()
            .any(|e| &e.target == target)
    }

    /// What the listener on `local_port` currently tunnels to, if any.
    pub fn local_target(&self, local_port: u16) -> Option<Target> {
        self.entries
            .lock()
            .unwrap()
            .get(&local_port)
            .map(|e| e.target.clone())
    }

    /// Current target and connection token of the listener on `local_port`.
    fn current(&self, local_port: u16) -> Option<(Target, CancellationToken)> {
        self.entries
            .lock()
            .unwrap()
            .get(&local_port)
            .map(|e| (e.target.clone(), e.conns.clone()))
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
        self.set_target(Target::host(port), local_port, enabled, pinned)
            .await
    }

    /// Forward `target` to `local_port` (0 = same as the target port), or
    /// remove its forward. A local port already tunnelling somewhere else is
    /// switched over to `target` without rebinding; the pin follows it.
    pub async fn set_target(
        self: &Arc<Self>,
        target: Target,
        local_port: u16,
        enabled: bool,
        pinned: bool,
    ) -> Result<()> {
        let local_port = if local_port == 0 {
            target.port
        } else {
            local_port
        };
        // This target's existing forward, wherever it is bound.
        let existing = self
            .entries
            .lock()
            .unwrap()
            .iter()
            .find(|(_, e)| e.target == target)
            .map(|(lp, _)| *lp);

        if !enabled {
            let entry = existing.and_then(|lp| self.entries.lock().unwrap().remove(&lp));
            if let Some(entry) = &entry {
                entry.stop.cancel();
                self.ui.log(
                    LogLevel::Info,
                    Some(self.host_id),
                    "forward",
                    format!(
                        "removed forward {target} → localhost:{}",
                        existing.unwrap_or(0)
                    ),
                );
            }
            self.persist_pin(&target, local_port, false).await;
            self.broadcast();
            return Ok(());
        }

        // Already forwarded to the requested local port: just update the pin.
        if existing == Some(local_port) {
            if let Some(entry) = self.entries.lock().unwrap().get_mut(&local_port) {
                entry.pinned = pinned;
            }
            self.persist_pin(&target, local_port, pinned).await;
            self.broadcast();
            return Ok(());
        }

        // The local port changed → drop the old binding before re-binding.
        if let Some(old) = existing.and_then(|lp| self.entries.lock().unwrap().remove(&lp)) {
            old.stop.cancel();
        }

        // The local port tunnels to another target: switch it over in place.
        let switched = {
            let mut entries = self.entries.lock().unwrap();
            entries.get_mut(&local_port).map(|entry| {
                let from = std::mem::replace(&mut entry.target, target.clone());
                entry.pinned |= pinned;
                entry.conns.cancel();
                entry.conns = entry.stop.child_token();
                (from, entry.pinned)
            })
        };
        if let Some((from, pinned)) = switched {
            self.persist_pin(&from, local_port, false).await;
            self.persist_pin(&target, local_port, pinned).await;
            self.ui.log(
                LogLevel::Info,
                Some(self.host_id),
                "forward",
                format!("switched localhost:{local_port} from {from} to {target}"),
            );
            self.broadcast();
            return Ok(());
        }

        // Bind the IPv4 loopback (mandatory) and the IPv6 loopback
        // (best-effort) so clients hitting either resolution of `localhost`
        // land in the tunnel.
        let listener_v4 = match TcpListener::bind(("127.0.0.1", local_port)).await {
            Ok(l) => l,
            Err(e) => {
                let err = if e.kind() == std::io::ErrorKind::AddrInUse {
                    NettleError::PortInUse {
                        port: local_port,
                        hint: free_local_port().await,
                    }
                } else {
                    NettleError::Msg(format!("cannot bind localhost:{local_port}: {e}"))
                };
                self.ui.log(
                    LogLevel::Error,
                    Some(self.host_id),
                    "forward",
                    format!("cannot forward {target}: {err}"),
                );
                return Err(err);
            }
        };
        let listener_v6 = match TcpListener::bind(("::1", local_port)).await {
            Ok(l) => Some(l),
            Err(e) => {
                self.ui.log(
                    LogLevel::Warn,
                    Some(self.host_id),
                    "forward",
                    format!("forward {target}: IPv6 loopback [::1]:{local_port} unavailable ({e}); tunnel is IPv4-only"),
                );
                None
            }
        };

        let stop = CancellationToken::new();
        self.entries.lock().unwrap().insert(
            local_port,
            Entry {
                target: target.clone(),
                pinned,
                stop: stop.clone(),
                conns: stop.child_token(),
            },
        );
        self.persist_pin(&target, local_port, pinned).await;

        self.spawn_accept_loop(listener_v4, local_port, stop.clone());
        if let Some(listener) = listener_v6 {
            self.spawn_accept_loop(listener, local_port, stop);
        }
        self.ui.log(
            LogLevel::Info,
            Some(self.host_id),
            "forward",
            format!(
                "forwarding {target} → localhost:{local_port}{}",
                if pinned { " (pinned)" } else { "" }
            ),
        );

        self.broadcast();
        Ok(())
    }

    fn spawn_accept_loop(
        self: &Arc<Self>,
        listener: TcpListener,
        local_port: u16,
        stop: CancellationToken,
    ) {
        let mgr = self.clone();
        tokio::spawn(async move {
            // accept() errors are usually transient (EMFILE, ECONNABORTED,
            // ENOBUFS); giving up would silently kill the tunnel. Back off a
            // little and keep serving, logging the first failure only.
            let mut logged_error = false;
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
                    accepted = listener.accept() => match accepted {
                        Ok((sock, peer)) => {
                            // Resolve the target per connection: it may have
                            // been switched since the listener was bound.
                            let Some((target, conns)) = mgr.current(local_port) else {
                                break;
                            };
                            let conn_stop = conns.child_token();
                            let mgr = mgr.clone();
                            tokio::spawn(async move {
                                let _ = proxy(&mgr, sock, peer, target, conn_stop).await;
                            });
                        }
                        Err(e) => {
                            if !logged_error {
                                logged_error = true;
                                mgr.ui.log(
                                    LogLevel::Warn,
                                    Some(mgr.host_id),
                                    "forward",
                                    format!("forward localhost:{local_port}: accept failed ({e}); retrying"),
                                );
                            }
                            tokio::select! {
                                _ = stop.cancelled() => break,
                                _ = tokio::time::sleep(ACCEPT_RETRY) => {}
                            }
                        }
                    }
                }
            }
        });
    }

    pub fn shutdown(&self) {
        let mut entries = self.entries.lock().unwrap();
        for (_, entry) in entries.drain() {
            entry.stop.cancel();
        }
    }

    /// Drop the target's pin (and any other pin on the same local port of
    /// this host), then record the new one if `pinned`.
    async fn persist_pin(&self, target: &Target, local_port: u16, pinned: bool) {
        let host_id = self.host_id;
        let target = target.clone();
        let _ = self
            .store
            .update_state(move |s| {
                s.pinned_forwards.retain(|p| {
                    p.host_id != host_id
                        || !(p.port == target.port && p.sandbox == target.sandbox
                            || pinned && p.local_port() == local_port)
                });
                if pinned {
                    s.pinned_forwards.push(PinnedForward {
                        host_id,
                        port: target.port,
                        local_port,
                        sandbox: target.sandbox,
                    });
                }
            })
            .await;
    }
}

/// A free ephemeral local port, offered as a "try this instead" hint when the
/// requested one is taken.
async fn free_local_port() -> Option<u16> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.ok()?;
    listener.local_addr().ok().map(|a| a.port())
}

/// Proxy one accepted local connection: through a direct-tcpip channel for a
/// host port, through an `sbx exec` stdio bridge for a sandbox port.
async fn proxy(
    mgr: &ForwardManager,
    mut sock: tokio::net::TcpStream,
    peer: std::net::SocketAddr,
    target: Target,
    stop: CancellationToken,
) -> Result<()> {
    let stats = mgr.ui.stats.get(mgr.host_id);
    let mut epoch_rx = mgr.epoch_rx.clone();
    let mut live_rx = mgr.ports_live_rx.clone();
    let mut sandbox_rx = mgr.sandbox_live_rx.clone();
    // Wait (bounded) for a live epoch AND the target port to be listening.
    let deadline = tokio::time::Instant::now() + WAIT_GRACE;
    let epoch = loop {
        let ready_epoch = epoch_rx.borrow().clone();
        if let Some(e) = ready_epoch {
            if mgr.is_live(&target) {
                break e;
            }
        }
        tokio::select! {
            _ = stop.cancelled() => return Ok(()),
            _ = tokio::time::sleep_until(deadline) => {
                stats.tunnel_wait_timeouts.fetch_add(1, Ordering::Relaxed);
                stats.touch();
                return Ok(());
            }
            r = epoch_rx.changed() => { if r.is_err() { return Ok(()); } }
            r = live_rx.changed() => { if r.is_err() { return Ok(()); } }
            r = sandbox_rx.changed() => { if r.is_err() { return Ok(()); } }
        }
    };

    let opened = match &target.sandbox {
        None => {
            crate::ssh::channel_open_loopback(
                &epoch.handle,
                target.port,
                &peer.ip().to_string(),
                peer.port() as u32,
            )
            .await
        }
        Some(name) => {
            let bind = mgr
                .sandbox_live_rx
                .borrow()
                .get(name)
                .and_then(|ports| ports.get(&target.port).cloned());
            open_sandbox_bridge(&epoch.handle, name, target.port, bind.as_deref()).await
        }
    };
    let channel = match opened {
        Ok(c) => c,
        Err(e) => {
            stats.tunnel_refused.fetch_add(1, Ordering::Relaxed);
            stats.touch();
            mgr.ui.log(
                LogLevel::Warn,
                Some(mgr.host_id),
                "forward",
                format!("forward {target}: remote refused the connection ({e})"),
            );
            return Err(e);
        }
    };

    let _slot = ActiveTunnelGuard::acquire(stats.clone());
    let outcome = pump(&mut sock, channel, &stats, &stop, &epoch.cancel).await;
    // A bridge that exits non-zero without a single byte back never reached
    // the service (sandbox gone, port closed, no socat/nc in the image).
    if target.sandbox.is_some() && outcome.bytes_down == 0 && outcome.exit.is_some_and(|c| c != 0) {
        stats.tunnel_refused.fetch_add(1, Ordering::Relaxed);
        stats.touch();
        mgr.ui.log(
            LogLevel::Warn,
            Some(mgr.host_id),
            "forward",
            format!(
                "forward {target}: the sandbox bridge failed (exit {})",
                outcome.exit.unwrap_or_default()
            ),
        );
    }
    Ok(())
}

/// An exec channel whose stdio is the sandbox port (see
/// [`sandbox::bridge_command`]).
pub async fn open_sandbox_bridge(
    handle: &russh::client::Handle<crate::ssh::handler::ClientHandler>,
    name: &str,
    port: u16,
    bind: Option<&str>,
) -> Result<russh::Channel<russh::client::Msg>> {
    if !sandbox::valid_name(name) {
        return Err(NettleError::Msg(format!("invalid sandbox name {name:?}")));
    }
    let channel = handle.channel_open_session().await?;
    channel
        .exec(true, sandbox::bridge_command(name, port, bind))
        .await?;
    Ok(channel)
}

/// How a pumped connection ended.
struct PumpOutcome {
    bytes_down: u64,
    /// Exit status of the remote command (exec bridges only).
    exit: Option<u32>,
}

/// Shovel bytes between a local socket and an SSH channel until both sides
/// are done, the forward is removed, or the link dies.
///
/// This drives the channel directly (`wait()` / `data()`) instead of going
/// through `Channel::into_stream()` + `copy_bidirectional`: it gives exact
/// byte accounting for the connection stats, treats a remote `Close` as the
/// end of the connection (a closed channel can never carry data again, so
/// there is nothing to wait for), and bounds how long a half-closed
/// connection may linger — keep-alive clients that never close their end
/// used to pin a task and a socket per connection indefinitely.
async fn pump(
    sock: &mut tokio::net::TcpStream,
    mut channel: russh::Channel<russh::client::Msg>,
    stats: &HostStats,
    stop: &CancellationToken,
    link_down: &CancellationToken,
) -> PumpOutcome {
    let (mut rd, mut wr) = sock.split();
    let mut buf = vec![0u8; PROXY_BUF];
    // Direction state: once a side signalled EOF we stop reading from it.
    let mut local_eof = false;
    let mut remote_eof = false;
    let far_future = tokio::time::Instant::now() + Duration::from_secs(365 * 24 * 3600);
    let mut linger_until = far_future;
    let mut outcome = PumpOutcome {
        bytes_down: 0,
        exit: None,
    };

    loop {
        if local_eof && remote_eof {
            break;
        }
        tokio::select! {
            _ = stop.cancelled() => break,
            _ = link_down.cancelled() => break,
            _ = tokio::time::sleep_until(linger_until) => break,
            r = rd.read(&mut buf), if !local_eof => match r {
                Ok(0) | Err(_) => {
                    local_eof = true;
                    linger_until = tokio::time::Instant::now() + HALF_CLOSE_LINGER;
                    let _ = channel.eof().await;
                }
                Ok(n) => {
                    // `data()` honours the SSH window; race it against the
                    // link so a stalled window on a dead link can't hang us.
                    let sent = tokio::select! {
                        _ = stop.cancelled() => false,
                        _ = link_down.cancelled() => false,
                        r = channel.data(&buf[..n]) => r.is_ok(),
                    };
                    if !sent {
                        break;
                    }
                    stats.add_tunnel_bytes(n as u64, 0);
                }
            },
            // Keep reading after the remote EOF: an exec bridge reports its
            // exit status and then `Close` once the process is gone.
            msg = channel.wait() => match msg {
                Some(ChannelMsg::Data { .. }) if remote_eof => {}
                Some(ChannelMsg::Data { data }) => {
                    let written = tokio::select! {
                        _ = stop.cancelled() => false,
                        _ = link_down.cancelled() => false,
                        r = wr.write_all(&data[..]) => r.is_ok(),
                    };
                    if !written {
                        break;
                    }
                    stats.add_tunnel_bytes(0, data.len() as u64);
                    outcome.bytes_down += data.len() as u64;
                }
                // stderr of an exec bridge (direct-tcpip never sends any):
                // diagnostics, not payload.
                Some(ChannelMsg::ExtendedData { .. }) => {}
                Some(ChannelMsg::ExitStatus { exit_status }) => outcome.exit = Some(exit_status),
                Some(ChannelMsg::Eof) => {
                    remote_eof = true;
                    linger_until = tokio::time::Instant::now() + HALF_CLOSE_LINGER;
                    let _ = wr.shutdown().await;
                }
                // Channel gone (or the session dropped it): nothing can flow
                // in either direction any more.
                Some(ChannelMsg::Close) | None => break,
                Some(_) => {}
            },
        }
    }
    // Best effort: tell the remote we're done. Errors (link already gone)
    // don't matter; dropping the channel closes it anyway.
    let _ = channel.close().await;
    outcome
}
