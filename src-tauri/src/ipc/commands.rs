use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};

use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::State;
use tokio::sync::watch;
use uuid::Uuid;

use crate::config::{ConnectionSet, HostConfig, HostPort, Settings, WebConfig};
use crate::error::{NettleError, Result};
use crate::ipc::types::{
    ConnState, DirListing, ForwardInfo, HostForward, SessionInfo, TransferDirection, TransferMeta,
    TransferProgress,
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
        Some(existing) => {
            // Cached runtime secrets belong to a connection identity; if that
            // changed, they no longer apply — drop them.
            let identity_changed = existing.hostname != host.hostname
                || existing.port != host.port
                || existing.username != host.username
                || existing.key_path != host.key_path;
            if identity_changed {
                state.vault.forget(Some(host.id));
            }
            *existing = host.clone();
        }
        None => hosts.push(host.clone()),
    }
    state.store.save_hosts(hosts).await?;
    state.ui.notify_hosts_changed();
    Ok(host)
}

#[tauri::command]
pub async fn delete_host(state: State<'_, AppState>, host_id: Uuid) -> Result<()> {
    teardown(&state, host_id).await;
    state
        .ui
        .emit_conn(host_id, ConnState::Disconnected { host_id });
    state.vault.forget(Some(host_id));
    let mut hosts = state.store.load_hosts().await;
    hosts.retain(|h| h.id != host_id);
    state.store.save_hosts(hosts).await?;
    state.ui.notify_hosts_changed();
    Ok(())
}

// ---------- connection ----------

#[tauri::command]
pub async fn connect(state: State<'_, AppState>, host_id: Uuid) -> Result<()> {
    connect_host(&state, host_id).await
}

/// Connect one host, honouring the "keep connections" setting (when off,
/// connecting to one host tears down every other live session first). Shared by
/// the Tauri command and the web-control server.
pub(crate) async fn connect_host(state: &AppState, host_id: Uuid) -> Result<()> {
    let keep = state.store.load_state().await.settings.keep_connections;
    if !keep {
        let others: Vec<Uuid> = {
            let sessions = state.sessions.lock().await;
            sessions
                .keys()
                .copied()
                .filter(|id| *id != host_id)
                .collect()
        };
        for id in others {
            teardown(state, id).await;
        }
    }
    open_session(state, host_id).await
}

/// Build and register a live session for one host (idempotent — reconnecting an
/// already-open host tears the old one down first).
pub(crate) async fn open_session(state: &AppState, host_id: Uuid) -> Result<()> {
    let host = state
        .store
        .load_hosts()
        .await
        .into_iter()
        .find(|h| h.id == host_id)
        .ok_or_else(|| NettleError::Msg("unknown host".into()))?;

    teardown(state, host_id).await;

    let persisted = state.store.load_state().await;
    let pins: Vec<(u16, u16)> = persisted
        .pinned_forwards
        .iter()
        .filter(|p| p.host_id == host_id)
        .map(|p| (p.port, p.local_port()))
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
        host_id,
        epoch_rx.clone(),
        forwards.clone(),
        ports_live_tx,
        ignored_shared,
        cmd_tx.clone(),
    );

    for (port, local_port) in pins {
        let _ = forwards.set_with_local(port, local_port, true, true).await;
    }

    let session = Arc::new(ActiveSession {
        browser: SftpBrowser::new(epoch_rx.clone()),
        transfers: TransferManager::new(ui, host_id, epoch_rx.clone()),
        terminal: StdMutex::new(None),
        host,
        cmd_tx,
        epoch_rx,
        forwards,
        actor_task: StdMutex::new(Some(actor_task)),
        scanner_task: StdMutex::new(Some(scanner_task)),
    });
    state.sessions.lock().await.insert(host_id, session);
    Ok(())
}

pub(crate) async fn teardown(state: &AppState, host_id: Uuid) {
    let old = state.sessions.lock().await.remove(&host_id);
    if let Some(old) = old {
        if let Some(term) = old.terminal.lock().unwrap().take() {
            term.close();
        }
        old.forwards.shutdown();
        let _ = old.cmd_tx.send(SessionCmd::Disconnect);
        let actor = old.actor_task.lock().unwrap().take();
        if let Some(actor) = actor {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), actor).await;
        }
        let scanner = old.scanner_task.lock().unwrap().take();
        if let Some(scanner) = scanner {
            scanner.abort();
        }
    }
    // Note: teardown is intentionally silent. It's used internally by
    // open_session to clear a stale session before reconnecting, so emitting a
    // `disconnected` here would wipe the freshly-created session on the UI. The
    // user-facing disconnect commands emit it explicitly.
}

#[tauri::command]
pub async fn disconnect(state: State<'_, AppState>, host_id: Uuid) -> Result<()> {
    disconnect_host(&state, host_id).await;
    Ok(())
}

/// Tear a session down and emit the user-facing `disconnected` event. Shared by
/// the Tauri command and the web-control server.
pub(crate) async fn disconnect_host(state: &AppState, host_id: Uuid) {
    teardown(state, host_id).await;
    state
        .ui
        .emit_conn(host_id, ConnState::Disconnected { host_id });
}

#[tauri::command]
pub async fn disconnect_all(state: State<'_, AppState>) -> Result<()> {
    let ids: Vec<Uuid> = state.sessions.lock().await.keys().copied().collect();
    for id in ids {
        teardown(&state, id).await;
        state
            .ui
            .emit_conn(id, ConnState::Disconnected { host_id: id });
    }
    Ok(())
}

/// Snapshot of every live session, for frontend hydration after a reload.
#[tauri::command]
pub fn list_sessions(state: State<'_, AppState>) -> Vec<SessionInfo> {
    state
        .ui
        .conn_states
        .lock()
        .unwrap()
        .iter()
        .map(|(host_id, conn)| SessionInfo {
            host_id: *host_id,
            conn: conn.clone(),
        })
        .collect()
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

// ---------- settings ----------

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Settings> {
    Ok(state.store.load_state().await.settings)
}

#[tauri::command]
pub async fn set_settings(state: State<'_, AppState>, settings: Settings) -> Result<()> {
    state
        .store
        .update_state(move |s| s.settings = settings)
        .await
}

// ---------- connection sets ----------

#[tauri::command]
pub async fn list_sets(state: State<'_, AppState>) -> Result<Vec<ConnectionSet>> {
    Ok(state.store.load_state().await.sets)
}

#[tauri::command]
pub async fn save_set(state: State<'_, AppState>, mut set: ConnectionSet) -> Result<ConnectionSet> {
    if set.id.is_nil() {
        set.id = Uuid::new_v4();
    }
    let saved = set.clone();
    state
        .store
        .update_state(move |s| match s.sets.iter_mut().find(|x| x.id == set.id) {
            Some(existing) => *existing = set,
            None => s.sets.push(set),
        })
        .await?;
    Ok(saved)
}

#[tauri::command]
pub async fn delete_set(state: State<'_, AppState>, set_id: Uuid) -> Result<()> {
    state
        .store
        .update_state(move |s| s.sets.retain(|x| x.id != set_id))
        .await
}

/// Connect every host in a set. With "keep connections" off, all hosts NOT in
/// the set are disconnected first so the set becomes the live working set.
#[tauri::command]
pub async fn connect_set(state: State<'_, AppState>, set_id: Uuid) -> Result<()> {
    let persisted = state.store.load_state().await;
    let set = persisted
        .sets
        .iter()
        .find(|s| s.id == set_id)
        .ok_or_else(|| NettleError::Msg("unknown connection set".into()))?
        .clone();

    if !persisted.settings.keep_connections {
        let drop_ids: Vec<Uuid> = {
            let sessions = state.sessions.lock().await;
            sessions
                .keys()
                .copied()
                .filter(|id| !set.host_ids.contains(id))
                .collect()
        };
        for id in drop_ids {
            teardown(&state, id).await;
        }
    }
    for host_id in set.host_ids {
        let _ = open_session(&state, host_id).await;
    }
    Ok(())
}

// ---------- per-host session access ----------

pub(crate) async fn with_session(state: &AppState, host_id: Uuid) -> Result<Arc<ActiveSession>> {
    state
        .sessions
        .lock()
        .await
        .get(&host_id)
        .cloned()
        .ok_or(NettleError::NotConnected)
}

// ---------- terminal ----------

#[tauri::command]
pub async fn term_open(
    state: State<'_, AppState>,
    host_id: Uuid,
    cols: u32,
    rows: u32,
    on_data: Channel<InvokeResponseBody>,
) -> Result<()> {
    let session = with_session(&state, host_id).await?;
    let handle = terminal::open(
        state.ui.clone(),
        host_id,
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
pub async fn term_write(state: State<'_, AppState>, host_id: Uuid, data: Vec<u8>) -> Result<()> {
    let session = with_session(&state, host_id).await?;
    let term = session.terminal.lock().unwrap();
    if let Some(term) = term.as_ref() {
        term.write(data);
    }
    Ok(())
}

#[tauri::command]
pub async fn term_resize(
    state: State<'_, AppState>,
    host_id: Uuid,
    cols: u32,
    rows: u32,
) -> Result<()> {
    let session = with_session(&state, host_id).await?;
    let term = session.terminal.lock().unwrap();
    if let Some(term) = term.as_ref() {
        term.resize(cols, rows);
    }
    Ok(())
}

#[tauri::command]
pub async fn term_close(state: State<'_, AppState>, host_id: Uuid) -> Result<()> {
    let session = with_session(&state, host_id).await?;
    if let Some(term) = session.terminal.lock().unwrap().take() {
        term.close();
    }
    Ok(())
}

// ---------- files ----------

#[tauri::command]
pub async fn sftp_list(
    state: State<'_, AppState>,
    host_id: Uuid,
    path: String,
) -> Result<DirListing> {
    let session = with_session(&state, host_id).await?;
    session.browser.list(&path).await
}

#[tauri::command]
pub async fn sftp_home(state: State<'_, AppState>, host_id: Uuid) -> Result<String> {
    let session = with_session(&state, host_id).await?;
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
    host_id: Uuid,
    direction: TransferDirection,
    remote_path: String,
    local_path: String,
    on_progress: Channel<TransferProgress>,
) -> Result<Uuid> {
    let session = with_session(&state, host_id).await?;
    Ok(session
        .transfers
        .start(direction, remote_path, local_path, on_progress))
}

#[tauri::command]
pub async fn transfer_cancel(state: State<'_, AppState>, host_id: Uuid, id: Uuid) -> Result<()> {
    let session = with_session(&state, host_id).await?;
    session.transfers.cancel(id);
    Ok(())
}

#[tauri::command]
pub async fn transfer_list(state: State<'_, AppState>, host_id: Uuid) -> Result<Vec<TransferMeta>> {
    let session = with_session(&state, host_id).await?;
    Ok(session.transfers.list())
}

#[tauri::command]
pub async fn transfer_clear_finished(state: State<'_, AppState>, host_id: Uuid) -> Result<()> {
    let session = with_session(&state, host_id).await?;
    session.transfers.clear_finished();
    Ok(())
}

// ---------- ports & forwards ----------

#[tauri::command]
pub async fn forward_set(
    state: State<'_, AppState>,
    host_id: Uuid,
    port: u16,
    enabled: bool,
    pinned: bool,
    local_port: Option<u16>,
) -> Result<()> {
    let session = with_session(&state, host_id).await?;
    session
        .forwards
        .set_with_local(port, local_port.unwrap_or(0), enabled, pinned)
        .await
}

/// Best-effort probe over the SSH connection: is the remote loopback port
/// serving `http` or `https`? Powers the "open in browser" action so the URL
/// gets the right scheme. Falls back to `http` when it can't tell.
#[tauri::command]
pub async fn probe_port_scheme(
    state: State<'_, AppState>,
    host_id: Uuid,
    port: u16,
) -> Result<String> {
    let session = with_session(&state, host_id).await?;
    let epoch = crate::ssh::current_epoch(&session.epoch_rx).ok_or(NettleError::NotConnected)?;
    let scheme = crate::ssh::probe_http_scheme(&epoch.handle, port).await?;
    Ok(scheme.to_string())
}

#[tauri::command]
pub async fn forward_list(state: State<'_, AppState>, host_id: Uuid) -> Result<Vec<ForwardInfo>> {
    let session = with_session(&state, host_id).await?;
    Ok(session.forwards.list())
}

/// Every forward across every live session — powers the dashboard.
#[tauri::command]
pub async fn all_forwards(state: State<'_, AppState>) -> Result<Vec<HostForward>> {
    let sessions: Vec<Arc<ActiveSession>> = state.sessions.lock().await.values().cloned().collect();
    let mut out = Vec::new();
    for session in sessions {
        for f in session.forwards.list() {
            out.push(HostForward {
                host_id: session.host.id,
                host_name: session.host.name.clone(),
                forward: f,
            });
        }
    }
    out.sort_by(|a, b| {
        a.host_name
            .cmp(&b.host_name)
            .then(a.forward.port.cmp(&b.forward.port))
    });
    Ok(out)
}

#[tauri::command]
pub async fn port_ignore(state: State<'_, AppState>, host_id: Uuid, port: u16) -> Result<()> {
    let key = HostPort { host_id, port };
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

// ---------- web control server ----------

/// Stop any running server, then (re)start it if the config says enabled.
/// Binding is validated before the caller persists the config.
async fn restart_web(state: &AppState, cfg: &WebConfig) -> Result<()> {
    let old = state.web.lock().unwrap().take();
    if let Some(handle) = old {
        handle.stop().await;
    }
    if cfg.enabled {
        let handle = crate::web::start(state.clone(), cfg).await?;
        *state.web.lock().unwrap() = Some(handle);
    }
    Ok(())
}

#[tauri::command]
pub async fn get_web_config(state: State<'_, AppState>) -> Result<WebConfig> {
    Ok(state.store.load_state().await.web)
}

#[tauri::command]
pub async fn set_web_config(state: State<'_, AppState>, config: WebConfig) -> Result<WebConfig> {
    let mut cfg = config;
    // A token is minted the first time the server is switched on.
    if cfg.enabled && cfg.token.is_empty() {
        cfg.token = crate::config::new_web_token();
    }
    restart_web(&state, &cfg).await?;
    let to_save = cfg.clone();
    state.store.update_state(move |s| s.web = to_save).await?;
    Ok(cfg)
}

#[tauri::command]
pub async fn web_regenerate_token(state: State<'_, AppState>) -> Result<WebConfig> {
    let mut cfg = state.store.load_state().await.web;
    cfg.token = crate::config::new_web_token();
    restart_web(&state, &cfg).await?;
    let to_save = cfg.clone();
    state.store.update_state(move |s| s.web = to_save).await?;
    Ok(cfg)
}

/// The shareable link (with the token in the URL fragment).
#[tauri::command]
pub async fn web_link(state: State<'_, AppState>) -> Result<String> {
    Ok(crate::web::link(&state.store.load_state().await.web))
}

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
