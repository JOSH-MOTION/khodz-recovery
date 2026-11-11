import { invoke } from "@tauri-apps/api/core";
import type { AppInfo, CommandError, Page, PhysicalDisk, Recovery, Scan, SettingKey } from "./types";

/** Typed wrappers over Tauri commands. The only module that calls `invoke`. */
export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  getSetting: (key: SettingKey) => invoke<string | null>("get_setting", { key }),
  setSetting: (key: SettingKey, value: string) => invoke<void>("set_setting", { key, value }),
  listScans: (page?: Page) => invoke<Scan[]>("list_scans", { page }),
  listRecoveries: (page?: Page) => invoke<Recovery[]>("list_recoveries", { page }),
  listDevices: () => invoke<PhysicalDisk[]>("list_devices"),
};

export function errorMessage(err: unknown): string {
  if (err && typeof err === "object" && "message" in err) {
    return String((err as CommandError).message);
  }
  return String(err);
}
