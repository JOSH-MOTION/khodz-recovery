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

// Mirrors crates/khodz-device/src/model.rs
export type BusType =
  | "usb"
  | "sata"
  | "nvme"
  | "pci_express"
  | "sas"
  | "scsi"
  | "ata"
  | "atapi"
  | "sd_card"
  | "mmc"
  | "thunderbolt"
  | "apple_fabric"
  | "firewire"
  | "fibre_channel"
  | "raid"
  | "iscsi"
  | "storage_spaces"
  | "virtual"
  | "disk_image"
  | "ufs"
  | "unknown";

export interface Volume {
  id: string;
  device_path: string;
  mount_points: string[];
  label: string | null;
  filesystem: string | null;
  size_bytes: number | null;
  free_bytes: number | null;
  offset_bytes: number | null;
  encrypted: boolean | null;
  is_system: boolean;
  spans_multiple_disks: boolean;
}

export interface PhysicalDisk {
  id: string;
  display_name: string;
  device_path: string;
  model: string | null;
  vendor: string | null;
  serial: string | null;
  size_bytes: number;
  logical_sector_size: number;
  physical_sector_size: number | null;
  bus: BusType;
  removable: boolean;
  is_system: boolean;
  is_virtual: boolean;
  solid_state: boolean | null;
  partition_scheme: string | null;
  media_present: boolean;
  volumes: Volume[];
}

export type SettingKey = "theme" | "allow_same_disk_recovery" | "last_destination";
