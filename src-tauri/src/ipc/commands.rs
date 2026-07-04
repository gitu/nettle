use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;
use tokio::sync::watch;
use uuid::Uuid;

use crate::config::{HostConfig, HostPort};
use crate::error::{NettleError, Result};
use crate::ipc::types::{
    ConnState, DirListing, ForwardInfo, TransferDirection, TransferMeta, TransferProgress,
};
use crate::local_fs;
use crate::ports::forwards::ForwardManager;
use crate::ports::scanner;
use crate::sftp::browse::SftpBrowser;
use crate::sftp::transfers::TransferManager;
use crate::ssh::session::{SessionActor, SessionCmd};
use crate::state::{ActiveSession, AppState};
use crate::terminal;

// ---------- hosts ----------

#[tauri::command]
pub async fn list_hosts(state: State<'_, AppState>) -> Result<Vec<HostConfig>> {
    Ok(state.store.load_hosts().await)
}

#[tauri::command]
pub async fn save_host(state: State<'_, AppState>, mut host: HostConfig) -> Result<HostConfig> {
    let mut hosts = state.store.load_hosts().await;
    if host.id.is_nil() {
        host.id = Uuid::new_v4();
    }
    match hosts.iter_mut().find(|h| h.id == host.id) {
        Some(existing) => *existing = host.clone(),
        None => hosts.push(host.clone()),
    }
    state.store.save_hosts(hosts).await?;
    Ok(host)
}

#[tauri::command]
pub async fn delete_host(state: State<'_, AppState>, host_id: Uuid) -> Result<()> {
    let mut hosts = state.store.load_hosts().await;
    hosts.retain(|h| h.id != host_id);
    state.store.save_hosts(hosts).await
}

// ---------- connection ----------

#[tauri::command]
pub async fn connect(state: State<'_, AppState>, host_id: Uuid) -> Result<()> {
    let host = state
        .store
        .load_hosts()
        .await
        .into_iter()
        .find(|h| h.id == host_id)
        .ok_or_else(|| NettleError::Msg("unknown host".into()))?;

    // Tear down any existing session first.
    teardown(&state).await;

    let persisted = state.store.load_state().await;
    let pins: Vec<u16> = persisted
        .pinned_forwards
        .iter()
        .filter(|p| p.host_id == host_id)
        .map(|p| p.port)
        .collect();
    let ignored: HashSet<u16> = persisted
        .ignored_ports
        .iter()
        .filter(|p| p.host_id == host_id)
        .map(|p| p.port)
        .collect();

    let ui = state.ui.clone();
    let (cmd_tx, epoch_rx, actor_task) = SessionActor::spawn(
        ui.clone(),
        host.clone(),
        state.store.known_hosts_path(),
        state.vault.clone(),
    );

    let (ports_live_tx, ports_live_rx) = watch::channel(HashSet::new());
    let forwards = ForwardManager::new(
        ui.clone(),
        host_id,
        state.store.clone(),
        epoch_rx.clone(),
        ports_live_rx.clone(),
    );
    let ignored_shared = Arc::new(StdMutex::new(ignored));
    let scanner_task = scanner::spawn(
        ui.clone(),
        epoch_rx.clone(),
        forwards.clone(),
        ports_live_tx,
        ignored_shared,
        cmd_tx.clone(),
    );

    // Re-establish pinned forwards immediately: listeners come up before the
    // first connection attempt even completes.
    for port in pins {
        let _ = forwards.set(port, true, true).await;
    }

    let session = Arc::new(ActiveSession {
        browser: SftpBrowser::new(epoch_rx.clone()),
        transfers: TransferManager::new(ui, epoch_rx.clone()),
        terminal: StdMutex::new(None),
        host,
        cmd_tx,
        epoch_rx,
        forwards,
        actor_task: StdMutex::new(Some(actor_task)),
        scanner_task: StdMutex::new(Some(scanner_task)),
    });
    *state.session.lock().await = Some(session);
    Ok(())
}

async fn teardown(state: &State<'_, AppState>) {
    let old = state.session.lock().await.take();
    if let Some(old) = old {
        if let Some(term) = old.terminal.lock().unwrap().take() {
            term.close();
        }
        old.forwards.shutdown();
        let _ = old.cmd_tx.send(SessionCmd::Disconnect);
        let actor = old.actor_task.lock().unwrap().take();
        if let Some(actor) = actor {
            if tokio::time::timeout(std::time::Duration::from_secs(3), actor)
                .await
                .is_err()
            {
                // actor hung; it will be dropped
            }
        }
        let scanner = old.scanner_task.lock().unwrap().take();
        if let Some(scanner) = scanner {
            scanner.abort();
        }
    }
}

#[tauri::command]
pub async fn disconnect(state: State<'_, AppState>) -> Result<()> {
    teardown(&state).await;
    Ok(())
}

#[tauri::command]
pub fn get_connection_state(state: State<'_, AppState>) -> ConnState {
    state.ui.conn_state.lock().unwrap().clone()
}

#[tauri::command]
pub fn host_key_decision(state: State<'_, AppState>, accept: bool) {
    state.ui.prompts.answer_host_key(accept);
}

#[tauri::command]
pub fn provide_secret(state: State<'_, AppState>, secret: Option<String>) {
    state.ui.prompts.answer_secret(secret);
}

/// Drop runtime-cached passwords/passphrases (one host, or all).
#[tauri::command]
pub fn forget_secrets(state: State<'_, AppState>, host_id: Option<Uuid>) {
    state.vault.forget(host_id);
}

// ---------- terminal ----------

async fn with_session(state: &State<'_, AppState>) -> Result<Arc<ActiveSession>> {
    state
        .session
        .lock()
        .await
        .clone()
        .ok_or(NettleError::NotConnected)
}

#[tauri::command]
pub async fn term_open(
    state: State<'_, AppState>,
    cols: u32,
    rows: u32,
    on_data: Channel<InvokeResponseBody>,
) -> Result<()> {
    let session = with_session(&state).await?;
    let handle = terminal::open(
        state.ui.clone(),
        session.epoch_rx.clone(),
        session.cmd_tx.clone(),
        cols,
        rows,
        on_data,
    );
    if let Some(old) = session.terminal.lock().unwrap().replace(handle) {
        old.close();
    }
    Ok(())
}

#[tauri::command]
pub async fn term_write(state: State<'_, AppState>, data: Vec<u8>) -> Result<()> {
    let session = with_session(&state).await?;
    let term = session.terminal.lock().unwrap();
    if let Some(term) = term.as_ref() {
        term.write(data);
    }
    Ok(())
}

#[tauri::command]
pub async fn term_resize(state: State<'_, AppState>, cols: u32, rows: u32) -> Result<()> {
    let session = with_session(&state).await?;
    let term = session.terminal.lock().unwrap();
    if let Some(term) = term.as_ref() {
        term.resize(cols, rows);
    }
    Ok(())
}

#[tauri::command]
pub async fn term_close(state: State<'_, AppState>) -> Result<()> {
    let session = with_session(&state).await?;
    if let Some(term) = session.terminal.lock().unwrap().take() {
        term.close();
    }
    Ok(())
}

// ---------- files ----------

#[tauri::command]
pub async fn sftp_list(state: State<'_, AppState>, path: String) -> Result<DirListing> {
    let session = with_session(&state).await?;
    session.browser.list(&path).await
}

#[tauri::command]
pub async fn sftp_home(state: State<'_, AppState>) -> Result<String> {
    let session = with_session(&state).await?;
    session.browser.home().await
}

#[tauri::command]
pub async fn local_list(path: String) -> Result<DirListing> {
    local_fs::list(&path).await
}

#[tauri::command]
pub fn local_home_dir() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/".to_string())
}

// ---------- transfers ----------

#[tauri::command]
pub async fn transfer_start(
    state: State<'_, AppState>,
    direction: TransferDirection,
    remote_path: String,
    local_path: String,
    on_progress: Channel<TransferProgress>,
) -> Result<Uuid> {
    let session = with_session(&state).await?;
    Ok(session
        .transfers
        .start(direction, remote_path, local_path, on_progress))
}

#[tauri::command]
pub async fn transfer_cancel(state: State<'_, AppState>, id: Uuid) -> Result<()> {
    let session = with_session(&state).await?;
    session.transfers.cancel(id);
    Ok(())
}

#[tauri::command]
pub async fn transfer_list(state: State<'_, AppState>) -> Result<Vec<TransferMeta>> {
    let session = with_session(&state).await?;
    Ok(session.transfers.list())
}

#[tauri::command]
pub async fn transfer_clear_finished(state: State<'_, AppState>) -> Result<()> {
    let session = with_session(&state).await?;
    session.transfers.clear_finished();
    Ok(())
}

// ---------- ports & forwards ----------

#[tauri::command]
pub async fn forward_set(
    state: State<'_, AppState>,
    port: u16,
    enabled: bool,
    pinned: bool,
) -> Result<()> {
    let session = with_session(&state).await?;
    session.forwards.set(port, enabled, pinned).await
}

#[tauri::command]
pub async fn forward_list(state: State<'_, AppState>) -> Result<Vec<ForwardInfo>> {
    let session = with_session(&state).await?;
    Ok(session.forwards.list())
}

#[tauri::command]
pub async fn port_ignore(state: State<'_, AppState>, port: u16) -> Result<()> {
    let session = with_session(&state).await?;
    let key = HostPort {
        host_id: session.host.id,
        port,
    };
    state
        .store
        .update_state(move |s| {
            if !s.ignored_ports.contains(&key) {
                s.ignored_ports.push(key);
            }
        })
        .await
}

// ---------- window controls (custom titlebar) ----------

#[tauri::command]
pub fn window_control(window: tauri::Window, action: String) {
    match action.as_str() {
        "close" => {
            let _ = window.close();
        }
        "minimize" => {
            let _ = window.minimize();
        }
        "maximize" => {
            if window.is_maximized().unwrap_or(false) {
                let _ = window.unmaximize();
            } else {
                let _ = window.maximize();
            }
        }
        _ => {}
    }
}
