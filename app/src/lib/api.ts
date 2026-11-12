import { Channel, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type {
  AppInfo,
  CommandError,
  FingerprintResult,
  ImageAnalysis,
  ImageInfo,
  DestinationReport,
  FolderListing,
  RecoveryItemRow,
  RecoveryOutcome,
  RecoveryProgress,
  SelectionPart,
  ItemFilter,
  ItemPage,
  ItemStats,
  ItemView,
  ScanItemDetail,
  ScanVolume,
  Scope,
  ScanOutcome,
  ScanProgress,
  ScanSource,
  JobProgress,
  LayoutReport,
  SearchOutcome,
  SearchProgress,
  VolumeSpec,
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
  /** `volumes` restricts the scan to those volumes (e.g. lost partitions); null = the partition table. */
  startScan: (
    source: ScanSource,
    mode: "quick" | "deep",
    jobId: string,
    onProgress: (p: ScanProgress) => void,
    volumes: VolumeSpec[] | null = null,
  ) => {
    const channel = new Channel<ScanProgress>();
    channel.onmessage = onProgress;
    return invoke<ScanOutcome>("start_scan", { source, mode, volumes, jobId, onProgress: channel });
  },
  inspectPartitions: (source: ScanSource) => invoke<LayoutReport>("inspect_partitions", { source }),
  searchPartitions: (source: ScanSource, jobId: string, onProgress: (p: SearchProgress) => void) => {
    const channel = new Channel<SearchProgress>();
    channel.onmessage = onProgress;
    return invoke<SearchOutcome>("search_partitions", { source, jobId, onProgress: channel });
  },
  listScanItems: (scanId: number, filter: ItemFilter, page: Page) =>
    invoke<ItemPage>("list_scan_items", { scanId, filter, page }),
  scanItemStats: (scanId: number) => invoke<ItemStats>("scan_item_stats", { scanId }),
  listVolumes: (scanId: number, view: ItemView) => invoke<ScanVolume[]>("list_volumes", { scanId, view }),
  listFolder: (scanId: number, scope: Scope, dir: string, view: ItemView, page: Page) =>
    invoke<FolderListing>("list_folder", { scanId, scope, dir, view, page }),
  getScanItem: (id: number) => invoke<ScanItemDetail>("get_scan_item", { id }),

  /** Native folder picker for the recovery destination. Null if cancelled. */
  pickFolder: async (): Promise<string | null> => {
    const picked = await open({ title: "Choose where to save recovered files", directory: true, multiple: false });
    return typeof picked === "string" ? picked : null;
  },
  checkRecovery: (scanId: number, selection: SelectionPart[], destination: string | null) =>
    invoke<DestinationReport>("check_recovery", { scanId, selection, destination }),
  startRecovery: (
    scanId: number,
    selection: SelectionPart[],
    destination: string,
    allowSameDisk: boolean,
    jobId: string,
    onProgress: (p: RecoveryProgress) => void,
  ) => {
    const channel = new Channel<RecoveryProgress>();
    channel.onmessage = onProgress;
    return invoke<RecoveryOutcome>("start_recovery", {
      scanId,
      selection,
      destination,
      allowSameDisk,
      jobId,
      onProgress: channel,
    });
  },
  getRecovery: (recoveryId: number) => invoke<Recovery>("get_recovery", { recoveryId }),
  listRecoveryItems: (recoveryId: number, page: Page) =>
    invoke<RecoveryItemRow[]>("list_recovery_items", { recoveryId, page }),
  showInFolder: (path: string) => revealItemInDir(path),
  /** Raw bytes of an item (read-only, from the scanned source), as an ArrayBuffer. */
  readItemData: async (itemId: number, offset: number, len: number): Promise<Uint8Array> => {
    const buf = await invoke<ArrayBuffer | number[]>("read_item_data", { itemId, offset, len });
    return buf instanceof ArrayBuffer ? new Uint8Array(buf) : Uint8Array.from(buf);
  },
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
