//! IPC commands. Each `#[tauri::command]` is a thin wrapper over a plain function taking
//! `&AppState`, so the logic is unit-testable without a running webview.

use serde::Serialize;
use tauri::State;

use khodz_db::{DbError, Page, Recovery, Scan};

use crate::state::AppState;

/// Error shape returned to the frontend: `{ kind, message }`.
#[derive(Debug, Serialize, PartialEq)]
pub struct CommandError {
    pub kind: &'static str,
    pub message: String,
}

impl CommandError {
    pub fn internal(msg: impl Into<String>) -> Self {
        Self { kind: "internal", message: msg.into() }
    }
}

impl From<DbError> for CommandError {
    fn from(e: DbError) -> Self {
        let kind = match e {
            DbError::NotFound(..) => "not_found",
            DbError::InvalidState(_) => "invalid_request",
            DbError::SchemaTooNew { .. } => "schema_too_new",
            DbError::Sqlite(_) => "database",
        };
        Self { kind, message: e.to_string() }
    }
}

type CmdResult<T> = Result<T, CommandError>;

#[derive(Debug, Serialize)]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    pub db_path: String,
    pub schema_version: u32,
}

pub fn app_info_impl(state: &AppState) -> CmdResult<AppInfo> {
    Ok(AppInfo {
        name: "Khodz Recovery",
        version: env!("CARGO_PKG_VERSION"),
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        db_path: state.db_path.display().to_string(),
        schema_version: state.db()?.schema_version()?,
    })
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> CmdResult<AppInfo> {
    app_info_impl(&state)
}

#[tauri::command]
pub fn get_setting(state: State<'_, AppState>, key: String) -> CmdResult<Option<String>> {
    Ok(state.db()?.get_setting(&key)?)
}

#[tauri::command]
pub fn set_setting(state: State<'_, AppState>, key: String, value: String) -> CmdResult<()> {
    Ok(state.db()?.set_setting(&key, &value)?)
}

#[tauri::command]
pub fn list_scans(state: State<'_, AppState>, page: Option<Page>) -> CmdResult<Vec<Scan>> {
    Ok(state.db()?.list_scans(page.unwrap_or_default())?)
}

#[tauri::command]
pub fn list_recoveries(state: State<'_, AppState>, page: Option<Page>) -> CmdResult<Vec<Recovery>> {
    Ok(state.db()?.list_recoveries(page.unwrap_or_default())?)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn state() -> AppState {
        AppState { db: Mutex::new(khodz_db::Database::open_in_memory().unwrap()), db_path: "memory".into() }
    }

    #[test]
    fn app_info_reports_real_build_values() {
        let info = app_info_impl(&state()).unwrap();
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.os, std::env::consts::OS);
        assert!(info.schema_version >= 1);
    }

    #[test]
    fn db_errors_map_to_stable_kinds() {
        let s = state();
        let err: CommandError = s.db().unwrap().set_setting("bogus", "x").unwrap_err().into();
        assert_eq!(err.kind, "invalid_request");
        let err: CommandError = s.db().unwrap().get_scan(7).unwrap_err().into();
        assert_eq!(err.kind, "not_found");
    }

    #[test]
    fn command_error_serializes_as_kind_and_message() {
        let json = serde_json::to_string(&CommandError::internal("x")).unwrap();
        assert_eq!(json, r#"{"kind":"internal","message":"x"}"#);
    }
}
