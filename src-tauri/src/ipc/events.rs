use tauri::{AppHandle, Emitter, Manager};

use super::types::{ConnState, ForwardInfo, PortsChanged, TransferMeta};
use crate::state::AppState;

pub fn emit_conn(app: &AppHandle, state: ConnState) {
    if let Some(app_state) = app.try_state::<AppState>() {
        *app_state.conn_state.lock().unwrap() = state.clone();
    }
    let _ = app.emit("connection-state", &state);
}

pub fn emit_ports(app: &AppHandle, payload: &PortsChanged) {
    let _ = app.emit("ports-changed", payload);
}

pub fn emit_forwards(app: &AppHandle, payload: &Vec<ForwardInfo>) {
    let _ = app.emit("forwards-changed", payload);
}

pub fn emit_transfer(app: &AppHandle, payload: &TransferMeta) {
    let _ = app.emit("transfer-updated", payload);
}

pub fn emit_term_closed(app: &AppHandle) {
    let _ = app.emit("term-closed", ());
}
