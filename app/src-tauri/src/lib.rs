//! Tauri shell. This layer only adapts engine/database APIs to IPC commands; it contains no
//! disk-access or recovery logic.

mod commands;
mod images;
mod state;

use tauri::Manager;

use crate::state::AppState;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let db_path = dir.join("khodz.db");
            let db = khodz_db::Database::open(&db_path)?;
            // Jobs left "running" by a previous process can never complete; record that honestly.
            db.fail_interrupted_scans()?;
            db.fail_interrupted_recoveries()?;
            app.manage(AppState::new(db, db_path));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            commands::get_setting,
            commands::set_setting,
            commands::list_scans,
            commands::list_recoveries,
            commands::list_devices,
            images::inspect_image,
            images::analyze_image,
            images::fingerprint_image,
            images::cancel_job,
        ])
        .run(tauri::generate_context!())
        .expect("failed to start Khodz Recovery");
}
