pub mod config;
pub mod error;
pub mod ipc;
pub mod local_fs;
pub mod ports;
pub mod sftp;
pub mod ssh;
pub mod state;
pub mod terminal;
pub mod tray;
pub mod web;

use tauri::Manager;

use config::ConfigStore;
use ipc::commands;
use state::{AppState, UiBridge};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let ui = UiBridge::new(Box::new(app.handle().clone()));
            app.manage(AppState::new(ConfigStore::new(config_dir), ui));
            tray::setup(app)?;

            // Bring the web-control server up if it was left enabled.
            let app_state = app.state::<AppState>().inner().clone();
            tauri::async_runtime::spawn(async move {
                let cfg = app_state.store.load_state().await.web;
                if cfg.enabled && !cfg.token.is_empty() {
                    match web::start(app_state.clone(), &cfg).await {
                        Ok(handle) => *app_state.web.lock().unwrap() = Some(handle),
                        Err(e) => eprintln!("nettle: web control server failed to start: {e}"),
                    }
                }
            });
            Ok(())
        })
        .on_window_event(tray::on_window_event)
        .invoke_handler(tauri::generate_handler![
            commands::list_hosts,
            commands::save_host,
            commands::delete_host,
            commands::connect,
            commands::disconnect,
            commands::disconnect_all,
            commands::list_sessions,
            commands::host_key_decision,
            commands::provide_secret,
            commands::forget_secrets,
            commands::get_settings,
            commands::set_settings,
            commands::list_sets,
            commands::save_set,
            commands::delete_set,
            commands::connect_set,
            commands::term_open,
            commands::term_write,
            commands::term_resize,
            commands::term_close,
            commands::sftp_list,
            commands::sftp_home,
            commands::local_list,
            commands::local_home_dir,
            commands::transfer_start,
            commands::transfer_cancel,
            commands::transfer_list,
            commands::transfer_clear_finished,
            commands::forward_set,
            commands::forward_list,
            commands::probe_port_scheme,
            commands::all_forwards,
            commands::port_ignore,
            commands::window_control,
            commands::get_web_config,
            commands::set_web_config,
            commands::web_regenerate_token,
            commands::web_link,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
