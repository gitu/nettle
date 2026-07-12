use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;

use crate::config::{ConfigStore, HostConfig};
use crate::ipc::types::{AuthRequest, ConnState, HostKeyPrompt, PortsChanged, TransferMeta};
use crate::ports::forwards::ForwardManager;
use crate::sftp::browse::SftpBrowser;
use crate::sftp::transfers::TransferManager;
use crate::ssh::session::SessionCmd;
use crate::ssh::EpochRx;
use crate::terminal::TerminalHandle;

/// Where backend events go. In the app this is the Tauri event system;
/// in tests it's a capture channel — the whole backend is headless-testable.
pub trait EventSink: Send + Sync + 'static {
    fn emit_json(&self, event: &str, payload: serde_json::Value);
}

impl<R: tauri::Runtime> EventSink for tauri::AppHandle<R> {
    fn emit_json(&self, event: &str, payload: serde_json::Value) {
        use tauri::Emitter;
        let _ = self.emit(event, payload);
    }
}

/// One-shot UI prompts (host key confirmation, password/passphrase entry).
/// The backend parks on a oneshot receiver; the frontend answers via a command.
#[derive(Default)]
pub struct Prompts {
    host_key: StdMutex<Option<oneshot::Sender<bool>>>,
    secret: StdMutex<Option<oneshot::Sender<Option<String>>>>,
}

impl Prompts {
    pub fn answer_host_key(&self, accept: bool) {
        if let Some(tx) = self.host_key.lock().unwrap().take() {
            let _ = tx.send(accept);
        }
    }

    pub fn answer_secret(&self, secret: Option<String>) {
        if let Some(tx) = self.secret.lock().unwrap().take() {
            let _ = tx.send(secret);
        }
    }
}

/// The backend's line to the UI: typed event emission + blocking prompts +
/// the connection-state cache used for frontend hydration.
pub struct UiBridge {
    sink: Box<dyn EventSink>,
    pub prompts: Prompts,
    /// Last connection state per host, for frontend hydration.
    pub conn_states: StdMutex<std::collections::HashMap<uuid::Uuid, ConnState>>,
    /// Latest listening-port snapshot per host — lets the tray build a menu
    /// synchronously without touching the async session map.
    pub ports: StdMutex<std::collections::HashMap<uuid::Uuid, Vec<crate::ipc::types::RemotePort>>>,
    /// Latest forward set per host (same rationale as `ports`).
    pub forwards:
        StdMutex<std::collections::HashMap<uuid::Uuid, Vec<crate::ipc::types::ForwardInfo>>>,
}

impl UiBridge {
    pub fn new(sink: Box<dyn EventSink>) -> Arc<Self> {
        Arc::new(Self {
            sink,
            prompts: Prompts::default(),
            conn_states: StdMutex::new(std::collections::HashMap::new()),
            ports: StdMutex::new(std::collections::HashMap::new()),
            forwards: StdMutex::new(std::collections::HashMap::new()),
        })
    }

    fn emit<T: Serialize>(&self, event: &str, payload: &T) {
        let value = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        self.sink.emit_json(event, value);
    }

    pub fn emit_conn(&self, host_id: uuid::Uuid, state: ConnState) {
        let mut map = self.conn_states.lock().unwrap();
        if matches!(state, ConnState::Disconnected { .. }) {
            map.remove(&host_id);
            // A gone session has no ports or forwards to show.
            self.ports.lock().unwrap().remove(&host_id);
            self.forwards.lock().unwrap().remove(&host_id);
        } else {
            map.insert(host_id, state.clone());
        }
        drop(map);
        self.emit("connection-state", &state);
    }

    pub fn emit_ports(&self, payload: &PortsChanged) {
        self.ports
            .lock()
            .unwrap()
            .insert(payload.host_id, payload.all.clone());
        self.emit("ports-changed", payload);
    }

    pub fn emit_forwards(&self, payload: &crate::ipc::types::ForwardsChanged) {
        self.forwards
            .lock()
            .unwrap()
            .insert(payload.host_id, payload.forwards.clone());
        self.emit("forwards-changed", payload);
    }

    pub fn emit_transfer(&self, payload: &TransferMeta) {
        self.emit("transfer-updated", payload);
    }

    pub fn emit_term_closed(&self, host_id: uuid::Uuid) {
        self.emit("term-closed", &serde_json::json!({ "hostId": host_id }));
    }

    /// Notify listeners (the tray) that the host list changed.
    pub fn notify_hosts_changed(&self) {
        self.emit("hosts-changed", &serde_json::json!({}));
    }

    pub fn emit_host_key_mismatch(&self, payload: &HostKeyPrompt) {
        self.emit("host-key-mismatch", payload);
    }

    pub async fn ask_host_key(&self, payload: HostKeyPrompt) -> bool {
        let (tx, rx) = oneshot::channel();
        *self.prompts.host_key.lock().unwrap() = Some(tx);
        self.emit("host-key-prompt", &payload);
        match tokio::time::timeout(Duration::from_secs(60), rx).await {
            Ok(Ok(accept)) => accept,
            _ => false,
        }
    }

    pub async fn ask_secret(&self, payload: AuthRequest) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        *self.prompts.secret.lock().unwrap() = Some(tx);
        self.emit("auth-request", &payload);
        match tokio::time::timeout(Duration::from_secs(180), rx).await {
            Ok(Ok(secret)) => secret,
            _ => None,
        }
    }
}

/// In-memory secrets per host, alive for the app's runtime only. Lets a user
/// disconnect/reconnect or hop between hosts without re-typing passwords;
/// nothing ever touches disk, and quitting the app forgets everything.
#[derive(Default)]
pub struct SecretVault {
    inner: StdMutex<std::collections::HashMap<uuid::Uuid, crate::ssh::auth::SecretCache>>,
}

impl SecretVault {
    pub fn get(&self, host_id: uuid::Uuid) -> crate::ssh::auth::SecretCache {
        self.inner
            .lock()
            .unwrap()
            .get(&host_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn store(&self, host_id: uuid::Uuid, cache: &crate::ssh::auth::SecretCache) {
        self.inner.lock().unwrap().insert(host_id, cache.clone());
    }

    pub fn forget(&self, host_id: Option<uuid::Uuid>) {
        let mut inner = self.inner.lock().unwrap();
        match host_id {
            Some(id) => {
                inner.remove(&id);
            }
            None => inner.clear(),
        }
    }
}

/// Everything belonging to the currently connected host.
pub struct ActiveSession {
    pub host: HostConfig,
    pub cmd_tx: mpsc::UnboundedSender<SessionCmd>,
    pub epoch_rx: EpochRx,
    pub terminal: StdMutex<Option<TerminalHandle>>,
    pub browser: SftpBrowser,
    pub transfers: Arc<TransferManager>,
    pub forwards: Arc<ForwardManager>,
    pub actor_task: StdMutex<Option<JoinHandle<()>>>,
    pub scanner_task: StdMutex<Option<JoinHandle<()>>>,
}

pub type Sessions = std::collections::HashMap<uuid::Uuid, Arc<ActiveSession>>;

/// Shared application state. Every field is cheap to clone (Arc / Clone), so the
/// whole thing can be handed to the embedded web-control server, which drives
/// the same session machinery as the Tauri commands.
#[derive(Clone)]
pub struct AppState {
    pub store: ConfigStore,
    /// One live session per connected host. Multiple hosts stay connected at
    /// once when the "keep connections" setting is on.
    pub sessions: Arc<Mutex<Sessions>>,
    pub ui: Arc<UiBridge>,
    pub vault: Arc<SecretVault>,
    /// Handle to the running web-control server, if any.
    pub web: Arc<StdMutex<Option<crate::web::WebHandle>>>,
}

impl AppState {
    pub fn new(store: ConfigStore, ui: Arc<UiBridge>) -> Self {
        Self {
            store,
            sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
            ui,
            vault: Arc::new(SecretVault::default()),
            web: Arc::new(StdMutex::new(None)),
        }
    }
}

#[cfg(test)]
mod vault_tests {
    use super::SecretVault;
    use crate::ssh::auth::SecretCache;
    use uuid::Uuid;

    fn pw(s: &str) -> SecretCache {
        SecretCache {
            password: Some(s.into()),
            key_passphrase: None,
        }
    }

    #[test]
    fn unknown_host_returns_empty_cache() {
        let vault = SecretVault::default();
        assert!(vault.get(Uuid::new_v4()).password.is_none());
    }

    #[test]
    fn stored_secret_is_returned_and_hosts_are_isolated() {
        let vault = SecretVault::default();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        vault.store(a, &pw("hunter2"));
        assert_eq!(vault.get(a).password.as_deref(), Some("hunter2"));
        // Another host must not see host A's password.
        assert!(vault.get(b).password.is_none());
    }

    #[test]
    fn forget_one_host_leaves_others() {
        let vault = SecretVault::default();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        vault.store(a, &pw("a-secret"));
        vault.store(b, &pw("b-secret"));
        // Invalidation on host edit forgets only that host.
        vault.forget(Some(a));
        assert!(vault.get(a).password.is_none());
        assert_eq!(vault.get(b).password.as_deref(), Some("b-secret"));
    }

    #[test]
    fn forget_all_clears_everything() {
        let vault = SecretVault::default();
        let a = Uuid::new_v4();
        vault.store(a, &pw("x"));
        vault.forget(None);
        assert!(vault.get(a).password.is_none());
    }
}
