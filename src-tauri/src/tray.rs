use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Listener, Manager, Runtime, WindowEvent};

/// Menu-bar / system-tray presence. Closing the window hides it here instead
/// of quitting, so pinned tunnels and the SSH session keep running.
pub fn setup<R: Runtime>(app: &tauri::App<R>) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show nettle", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "Quit nettle", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &sep, &quit])?;

    let mut builder = TrayIconBuilder::with_id("nettle-tray")
        .menu(&menu)
        .tooltip("nettle")
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Left click (Windows/Linux convention) brings the window back.
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

    // Reflect the active tunnel count next to the icon (macOS title,
    // tooltip elsewhere).
    let handle = app.handle().clone();
    app.listen("forwards-changed", move |event| {
        let count = serde_json::from_str::<Vec<serde_json::Value>>(event.payload())
            .map(|v| v.len())
            .unwrap_or(0);
        if let Some(tray) = handle.tray_by_id("nettle-tray") {
            let title = if count > 0 {
                Some(format!("{count}"))
            } else {
                None
            };
            #[cfg(target_os = "macos")]
            let _ = tray.set_title(title.clone());
            let tooltip = if count > 0 {
                format!("nettle — {count} tunnel(s) active")
            } else {
                "nettle".to_string()
            };
            let _ = tray.set_tooltip(Some(tooltip));
            let _ = title;
        }
    });

    Ok(())
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
