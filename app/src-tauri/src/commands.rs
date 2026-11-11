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

impl From<khodz_core::Error> for CommandError {
    fn from(e: khodz_core::Error) -> Self {
        use khodz_core::Error as E;
        let kind = match e {
            E::Io(_) => "io",
            E::Cancelled => "cancelled",
            E::DeviceLost(_) => "device_lost",
            E::MediaError { .. } => "media_error",
            E::AccessDenied(_) => "access_denied",
            E::InvalidData(_) => "invalid_data",
            E::Unsupported(_) => "unsupported",
            E::UnsafeDestination(_) => "unsafe_destination",
        };
        Self { kind, message: e.to_string() }
    }
}

type CmdResult<T> = Result<T, CommandError>;

/// Enumerate disks and volumes. Runs on a blocking worker thread: on macOS it spawns
/// `diskutil`, and on Windows slow USB bridges can take a moment to answer IOCTLs.
#[tauri::command]
pub async fn list_devices() -> CmdResult<Vec<khodz_device::PhysicalDisk>> {
    tauri::async_runtime::spawn_blocking(khodz_device::list_disks)
        .await
        .map_err(|e| CommandError::internal(format!("device enumeration task failed: {e}")))?
        .map_err(CommandError::from)
}

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
    use super::*;

    fn state() -> AppState {
        AppState::new(khodz_db::Database::open_in_memory().unwrap(), "memory".into())
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
    fn engine_errors_map_to_stable_kinds() {
        let e: CommandError = khodz_core::Error::AccessDenied("x".into()).into();
        assert_eq!(e.kind, "access_denied");
        let e: CommandError = khodz_core::Error::Unsupported("x".into()).into();
        assert_eq!(e.kind, "unsupported");
    }

    #[test]
    fn command_error_serializes_as_kind_and_message() {
        let json = serde_json::to_string(&CommandError::internal("x")).unwrap();
        assert_eq!(json, r#"{"kind":"internal","message":"x"}"#);
    }
}
