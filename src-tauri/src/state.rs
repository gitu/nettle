use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;

use crate::config::{ConfigStore, HostConfig};
use crate::ipc::types::{
    AuthRequest, ConnState, ForwardInfo, HostKeyPrompt, PortsChanged, TransferMeta,
};
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
    pub conn_state: StdMutex<ConnState>,
}

impl UiBridge {
    pub fn new(sink: Box<dyn EventSink>) -> Arc<Self> {
        Arc::new(Self {
            sink,
            prompts: Prompts::default(),
            conn_state: StdMutex::new(ConnState::Disconnected),
        })
    }

    fn emit<T: Serialize>(&self, event: &str, payload: &T) {
        let value = serde_json::to_value(payload).unwrap_or(serde_json::Value::Null);
        self.sink.emit_json(event, value);
    }

    pub fn emit_conn(&self, state: ConnState) {
        *self.conn_state.lock().unwrap() = state.clone();
        self.emit("connection-state", &state);
    }

    pub fn emit_ports(&self, payload: &PortsChanged) {
        self.emit("ports-changed", payload);
    }

    pub fn emit_forwards(&self, payload: &Vec<ForwardInfo>) {
        self.emit("forwards-changed", payload);
    }

    pub fn emit_transfer(&self, payload: &TransferMeta) {
        self.emit("transfer-updated", payload);
    }

    pub fn emit_term_closed(&self) {
        self.emit("term-closed", &());
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

pub struct AppState {
    pub store: ConfigStore,
    pub session: Mutex<Option<Arc<ActiveSession>>>,
    pub ui: Arc<UiBridge>,
}

impl AppState {
    pub fn new(store: ConfigStore, ui: Arc<UiBridge>) -> Self {
        Self {
            store,
            session: Mutex::new(None),
            ui,
        }
    }
}
