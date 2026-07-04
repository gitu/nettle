use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use tauri::{AppHandle, Emitter};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio::task::JoinHandle;

use crate::config::{ConfigStore, HostConfig};
use crate::ipc::types::{AuthRequest, ConnState, HostKeyPrompt};
use crate::ports::forwards::ForwardManager;
use crate::sftp::browse::SftpBrowser;
use crate::sftp::transfers::TransferManager;
use crate::ssh::session::SessionCmd;
use crate::ssh::EpochRx;
use crate::terminal::TerminalHandle;

/// One-shot UI prompts (host key confirmation, password/passphrase entry).
/// The backend parks on a oneshot receiver; the frontend answers via a command.
#[derive(Default)]
pub struct Prompts {
    host_key: StdMutex<Option<oneshot::Sender<bool>>>,
    secret: StdMutex<Option<oneshot::Sender<Option<String>>>>,
}

impl Prompts {
    pub async fn ask_host_key(&self, app: &AppHandle, payload: HostKeyPrompt) -> bool {
        let (tx, rx) = oneshot::channel();
        *self.host_key.lock().unwrap() = Some(tx);
        let _ = app.emit("host-key-prompt", &payload);
        match tokio::time::timeout(Duration::from_secs(60), rx).await {
            Ok(Ok(accept)) => accept,
            _ => false,
        }
    }

    pub fn answer_host_key(&self, accept: bool) {
        if let Some(tx) = self.host_key.lock().unwrap().take() {
            let _ = tx.send(accept);
        }
    }

    pub async fn ask_secret(&self, app: &AppHandle, payload: AuthRequest) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        *self.secret.lock().unwrap() = Some(tx);
        let _ = app.emit("auth-request", &payload);
        match tokio::time::timeout(Duration::from_secs(180), rx).await {
            Ok(Ok(secret)) => secret,
            _ => None,
        }
    }

    pub fn answer_secret(&self, secret: Option<String>) {
        if let Some(tx) = self.secret.lock().unwrap().take() {
            let _ = tx.send(secret);
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
    pub prompts: Arc<Prompts>,
    pub conn_state: StdMutex<ConnState>,
}

impl AppState {
    pub fn new(store: ConfigStore) -> Self {
        Self {
            store,
            session: Mutex::new(None),
            prompts: Arc::new(Prompts::default()),
            conn_state: StdMutex::new(ConnState::Disconnected),
        }
    }
}
