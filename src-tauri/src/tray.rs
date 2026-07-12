use tauri::menu::{CheckMenuItemBuilder, Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Listener, Manager, Runtime, WindowEvent};
use uuid::Uuid;

use crate::ipc::commands;
use crate::ipc::types::ConnState;
use crate::state::AppState;

/// Menu-bar / system-tray presence. Closing the window hides it here instead
/// of quitting, so pinned tunnels and the SSH session keep running. The menu is
/// rebuilt live to show each host's state and offer quick actions (connect /
/// disconnect / forward a discovered port) without opening the window.
pub fn setup<R: Runtime>(app: &tauri::App<R>) -> tauri::Result<()> {
    let menu = Menu::with_items(app, &[])?; // replaced immediately by rebuild()

    let mut builder = TrayIconBuilder::with_id("nettle-tray")
        .menu(&menu)
        .tooltip("nettle")
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { .. } = event {
                show_main(tray.app_handle());
            }
        });
    // macOS menu bar: monochrome template image so the system recolors it
    // for light/dark mode and selection. Elsewhere: the colored app icon.
    #[cfg(target_os = "macos")]
    {
        let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
        builder = builder.icon(icon).icon_as_template(true);
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;

    // Populate the menu now, and rebuild it whenever hosts, connection state,
    // ports, or forwards change.
    rebuild(app.handle());
    for event in [
        "connection-state",
        "ports-changed",
        "forwards-changed",
        "hosts-changed",
    ] {
        let handle = app.handle().clone();
        app.listen(event, move |_| rebuild(&handle));
    }

    Ok(())
}

// ---------- dynamic menu ----------

/// Send-safe snapshot of one host for building the menu on the main thread.
struct HostRow {
    id: Uuid,
    name: String,
    status: &'static str,
    connected: bool,
    ports: Vec<PortRow>,
    forward_count: usize,
}

struct PortRow {
    port: u16,
    process: Option<String>,
    forwarded: bool,
}

/// Gather host state synchronously from the sync snapshots on `UiBridge`
/// (plus the persisted host list), then rebuild + apply the menu on the main
/// thread.
fn rebuild<R: Runtime>(app: &AppHandle<R>) {
    let state = app.state::<AppState>().inner().clone();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let hosts = state.store.load_hosts().await;
        let conn = state.ui.conn_states.lock().unwrap().clone();
        let ports = state.ui.ports.lock().unwrap().clone();
        let forwards = state.ui.forwards.lock().unwrap().clone();

        let rows: Vec<HostRow> = hosts
            .into_iter()
            .map(|h| {
                let (status, connected) = match conn.get(&h.id) {
                    Some(ConnState::Connected { .. }) => ("connected", true),
                    Some(ConnState::Connecting { .. }) => ("connecting", false),
                    Some(ConnState::Authenticating { .. }) => ("authenticating", false),
                    Some(ConnState::Reconnecting { .. }) => ("reconnecting", false),
                    Some(ConnState::Failed { .. }) => ("failed", false),
                    _ => ("disconnected", false),
                };
                let fwd = forwards.get(&h.id);
                let forward_count = fwd.map(|v| v.len()).unwrap_or(0);
                let fwd_ports: std::collections::HashSet<u16> = fwd
                    .map(|v| v.iter().map(|f| f.port).collect())
                    .unwrap_or_default();
                let prows = if connected {
                    ports
                        .get(&h.id)
                        .map(|v| {
                            v.iter()
                                .map(|p| PortRow {
                                    port: p.port,
                                    process: p.process.clone(),
                                    forwarded: fwd_ports.contains(&p.port),
                                })
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };
                HostRow {
                    id: h.id,
                    name: h.name,
                    status,
                    connected,
                    ports: prows,
                    forward_count,
                }
            })
            .collect();

        let tunnels: usize = rows.iter().map(|r| r.forward_count).sum();
        let call_handle = handle.clone();
        let _ = call_handle.run_on_main_thread(move || {
            if let Ok(menu) = build_menu(&handle, &rows) {
                if let Some(tray) = handle.tray_by_id("nettle-tray") {
                    let _ = tray.set_menu(Some(menu));
                    #[cfg(target_os = "macos")]
                    let _ = tray.set_title(if tunnels > 0 {
                        Some(format!("{tunnels}"))
                    } else {
                        None
                    });
                    let tip = if tunnels > 0 {
                        format!("nettle — {tunnels} tunnel(s) active")
                    } else {
                        "nettle".to_string()
                    };
                    let _ = tray.set_tooltip(Some(tip));
                }
            }
        });
    });
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, rows: &[HostRow]) -> tauri::Result<Menu<R>> {
    let mut mb = MenuBuilder::new(app)
        .text("show", "Show nettle")
        .text("about", "About nettle")
        .separator();

    if rows.is_empty() {
        let empty = MenuItemBuilder::with_id("nohosts", "No hosts yet")
            .enabled(false)
            .build(app)?;
        mb = mb.item(&empty);
    } else {
        for row in rows {
            let dot = match row.status {
                "connected" => "●",
                "connecting" | "authenticating" | "reconnecting" => "◐",
                "failed" => "✕",
                _ => "○",
            };
            let mut sb = SubmenuBuilder::new(app, format!("{dot}  {}", row.name));
            if row.connected {
                sb = sb
                    .text(format!("hs:{}", row.id), "Show")
                    .text(format!("hd:{}", row.id), "Disconnect")
                    .separator();
                if row.ports.is_empty() {
                    let scanning = MenuItemBuilder::with_id("noports", "scanning ports…")
                        .enabled(false)
                        .build(app)?;
                    sb = sb.item(&scanning);
                } else {
                    let hdr = MenuItemBuilder::with_id("pfhdr", "Forward a port")
                        .enabled(false)
                        .build(app)?;
                    sb = sb.item(&hdr);
                    for p in &row.ports {
                        let label = match &p.process {
                            Some(proc) => format!("{}   {}", p.port, proc),
                            None => format!("{}", p.port),
                        };
                        let item = CheckMenuItemBuilder::with_id(
                            format!("pf:{}:{}", row.id, p.port),
                            label,
                        )
                        .checked(p.forwarded)
                        .build(app)?;
                        sb = sb.item(&item);
                    }
                }
            } else if row.status == "disconnected" || row.status == "failed" {
                sb = sb
                    .text(format!("hc:{}", row.id), "Connect")
                    .text(format!("hs:{}", row.id), "Show");
            } else {
                let busy = MenuItemBuilder::with_id("busy", format!("{}…", row.status))
                    .enabled(false)
                    .build(app)?;
                sb = sb.item(&busy).text(format!("hs:{}", row.id), "Show");
            }
            let sub = sb.build()?;
            mb = mb.item(&sub);
        }
    }

    mb.separator().text("quit", "Quit nettle").build()
}

// ---------- menu events ----------

fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: tauri::menu::MenuEvent) {
    let id = event.id.as_ref();
    match id {
        "show" => show_main(app),
        "about" => {
            show_main(app);
            let _ = app.emit("open-about", ());
        }
        "quit" => app.exit(0),
        _ => handle_host_action(app, id),
    }
}

fn handle_host_action<R: Runtime>(app: &AppHandle<R>, id: &str) {
    if let Some(uuid) = id.strip_prefix("hs:").and_then(parse_uuid) {
        show_main(app);
        let _ = app.emit("tray-focus-host", uuid.to_string());
    } else if let Some(uuid) = id.strip_prefix("hc:").and_then(parse_uuid) {
        let state = app.state::<AppState>().inner().clone();
        tauri::async_runtime::spawn(async move {
            let _ = commands::connect_host(&state, uuid).await;
        });
    } else if let Some(uuid) = id.strip_prefix("hd:").and_then(parse_uuid) {
        let state = app.state::<AppState>().inner().clone();
        tauri::async_runtime::spawn(async move {
            commands::disconnect_host(&state, uuid).await;
        });
    } else if let Some(rest) = id.strip_prefix("pf:") {
        if let Some((uuid, port)) = parse_host_port(rest) {
            let state = app.state::<AppState>().inner().clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(session) = commands::with_session(&state, uuid).await {
                    // Toggle: forward if not already, otherwise remove it.
                    let on = session.forwards.list().iter().any(|f| f.port == port);
                    let _ = session.forwards.set_with_local(port, 0, !on, false).await;
                }
            });
        }
    }
}

fn parse_uuid(s: &str) -> Option<Uuid> {
    Uuid::parse_str(s).ok()
}

/// "<uuid>:<port>" → (uuid, port). The uuid contains hyphens, not colons, so
/// split on the last colon.
fn parse_host_port(s: &str) -> Option<(Uuid, u16)> {
    let (uuid, port) = s.rsplit_once(':')?;
    Some((Uuid::parse_str(uuid).ok()?, port.parse().ok()?))
}

/// Hide instead of close, so background tunnels survive.
pub fn on_window_event<R: Runtime>(window: &tauri::Window<R>, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let _ = window.hide();
    }
}

fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
