// Mirrors of Rust types serialized over Tauri IPC. Keep in sync with crates/khodz-db/src/models.rs
// and app/src-tauri/src/commands.rs.

export type SourceKind = "physical_disk" | "volume" | "image";
export type ScanMode = "quick" | "deep";
export type ScanStatus = "running" | "completed" | "cancelled" | "failed" | "device_lost";
export type RecoveryStatus = "running" | "completed" | "cancelled" | "failed";

export interface Scan {
  id: number;
  source_kind: SourceKind;
  source_path: string;
  source_label: string | null;
  source_size: number;
  scan_mode: ScanMode;
  status: ScanStatus;
  error: string | null;
  started_at: number;
  finished_at: number | null;
  items_found: number;
}

export interface Recovery {
  id: number;
  scan_id: number | null;
  destination: string;
  status: RecoveryStatus;
  error: string | null;
  started_at: number;
  finished_at: number | null;
  files_total: number;
  files_verified: number;
  files_failed: number;
  bytes_written: number;
}

export interface Page {
  limit: number;
  offset: number;
}

export interface AppInfo {
  name: string;
  version: string;
  os: string;
  arch: string;
  db_path: string;
  schema_version: number;
}

export interface CommandError {
  kind: string;
  message: string;
}

export type SettingKey = "theme" | "allow_same_disk_recovery" | "last_destination";
