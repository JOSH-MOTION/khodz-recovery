import { Channel, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import type {
  AppInfo,
  CommandError,
  FingerprintResult,
  ImageAnalysis,
  ImageInfo,
  ItemFilter,
  ItemPage,
  ItemStats,
  ScanOutcome,
  ScanProgress,
  ScanSource,
  JobProgress,
  Page,
  PhysicalDisk,
  Recovery,
  Scan,
  SettingKey,
} from "./types";

/** Raw (sector-for-sector) image extensions. Compressed/container formats are not supported. */
export const IMAGE_EXTENSIONS = ["img", "dd", "raw", "bin", "ima", "001"];

/** Typed wrappers over Tauri commands. The only module that calls `invoke`. */
export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  getSetting: (key: SettingKey) => invoke<string | null>("get_setting", { key }),
  setSetting: (key: SettingKey, value: string) => invoke<void>("set_setting", { key, value }),
  listScans: (page?: Page) => invoke<Scan[]>("list_scans", { page }),
  listRecoveries: (page?: Page) => invoke<Recovery[]>("list_recoveries", { page }),
  listDevices: () => invoke<PhysicalDisk[]>("list_devices"),

  /** Native "open file" dialog. Resolves to null if the user cancels. */
  pickImageFile: async (): Promise<string | null> => {
    const picked = await open({
      title: "Open disk image",
      multiple: false,
      directory: false,
      filters: [
        { name: "Raw disk images", extensions: IMAGE_EXTENSIONS },
        { name: "All files", extensions: ["*"] },
      ],
    });
    return typeof picked === "string" ? picked : null;
  },
  inspectImage: (path: string) => invoke<ImageInfo>("inspect_image", { path }),
  analyzeImage: (path: string) => invoke<ImageAnalysis>("analyze_image", { path }),
  fingerprintImage: (path: string, jobId: string, onProgress: (p: JobProgress) => void) => {
    const channel = new Channel<JobProgress>();
    channel.onmessage = onProgress;
    return invoke<FingerprintResult>("fingerprint_image", { path, jobId, onProgress: channel });
  },
  cancelJob: (jobId: string) => invoke<boolean>("cancel_job", { jobId }),
  startScan: (source: ScanSource, mode: "quick" | "deep", jobId: string, onProgress: (p: ScanProgress) => void) => {
    const channel = new Channel<ScanProgress>();
    channel.onmessage = onProgress;
    return invoke<ScanOutcome>("start_scan", { source, mode, jobId, onProgress: channel });
  },
  listScanItems: (scanId: number, filter: ItemFilter, page: Page) =>
    invoke<ItemPage>("list_scan_items", { scanId, filter, page }),
  scanItemStats: (scanId: number) => invoke<ItemStats>("scan_item_stats", { scanId }),
};

export function errorMessage(err: unknown): string {
  if (err && typeof err === "object" && "message" in err) {
    return String((err as CommandError).message);
  }
  return String(err);
}

export function errorKind(err: unknown): string | null {
  return err && typeof err === "object" && "kind" in err ? String((err as CommandError).kind) : null;
}
