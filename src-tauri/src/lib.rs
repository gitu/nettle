pub mod config;
pub mod error;
pub mod ipc;
pub mod local_fs;
pub mod ports;
pub mod sftp;
pub mod ssh;
pub mod state;
pub mod terminal;

use tauri::Manager;

use config::ConfigStore;
use ipc::commands;
use state::{AppState, UiBridge};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .setup(|app| {
            let config_dir = app.path().app_config_dir()?;
            let ui = UiBridge::new(Box::new(app.handle().clone()));
            app.manage(AppState::new(ConfigStore::new(config_dir), ui));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_hosts,
            commands::save_host,
            commands::delete_host,
            commands::connect,
            commands::disconnect,
            commands::get_connection_state,
            commands::host_key_decision,
            commands::provide_secret,
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
            commands::port_ignore,
            commands::window_control,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
