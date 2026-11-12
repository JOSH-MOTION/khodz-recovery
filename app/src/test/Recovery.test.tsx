import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type {
  DestinationReport,
  FolderListing,
  Recovery,
  RecoveryItemRow,
  RecoveryOutcome,
  RecoveryProgress,
  Scan,
  ScanItemRow,
  Scope,
  SelectionPart,
} from "../lib/types";

vi.mock("../lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/api")>();
  return {
    ...actual,
    api: {
      listScans: vi.fn(),
      listScanItems: vi.fn(),
      scanItemStats: vi.fn(),
      listVolumes: vi.fn(),
      listFolder: vi.fn(),
      getScanItem: vi.fn(),
      readItemData: vi.fn(),
      startScan: vi.fn(),
      cancelJob: vi.fn(),
      pickFolder: vi.fn(),
      checkRecovery: vi.fn(),
      startRecovery: vi.fn(),
      listRecoveries: vi.fn(),
      listRecoveryItems: vi.fn(),
      showInFolder: vi.fn(),
    },
  };
});

import { api } from "../lib/api";
import { RecoveryProvider } from "../lib/recovery";
import { ScanProvider } from "../lib/scan";
import { RecoveryPage } from "../pages/RecoveryPage";
import { ResultsPage } from "../pages/ResultsPage";

const scan: Scan = {
  id: 7,
  source_kind: "physical_disk",
  source_path: "\\\\.\\PhysicalDrive2",
  source_label: "USB stick",
  source_size: 30_765_219_840,
  scan_mode: "quick",
  status: "completed",
  error: null,
  started_at: 1_700_000_000_000,
  finished_at: 1_700_000_005_000,
  items_found: 3,
};

function row(over: Partial<ScanItemRow>): ScanItemRow {
  return {
    id: 1,
    path: "/notes.txt",
    name: "notes.txt",
    size: 11,
    is_dir: false,
    is_deleted: false,
    name_complete: true,
    origin: "metadata",
    extent_quality: "exact",
    filesystem: "FAT32",
    file_type: "txt",
    modified_at: null,
    data_state: "present",
    carve_status: null,
    note: null,
    ...over,
  };
}

const notes = row({ id: 1 });
const photo = row({ id: 3, path: "/photo.jpg", name: "photo.jpg", file_type: "jpg", size: 2048 });
const report = row({ id: 5, path: "/docs/report.bin", name: "report.bin", file_type: "bin", size: 20 });

function listing(_scope: Scope, dir: string): FolderListing {
  if (dir === "/docs") return { folders: [], files: [report], total_files: 1 };
  return { folders: [{ name: "docs", path: "/docs", files: 1, deleted: 0 }], files: [notes, photo], total_files: 2 };
}

const okCheck = {
  destination: "E:\\Recovered",
  dest_disks: ["PhysicalDrive1"],
  source_disks: ["PhysicalDrive2"],
  same_disk: false,
  free_bytes: 500_000_000_000,
  required_bytes: 67_110_000,
  problems: [],
  warnings: [],
};

function report_(files: number, check: DestinationReport["check"]): DestinationReport {
  return { summary: { files, bytes: 2079, erased: 0, partial: 0 }, check };
}

const done: Recovery = {
  id: 3,
  scan_id: 7,
  destination: "E:\\Recovered\\Khodz Recovery 2026-11-12 01-00-00",
  status: "completed",
  error: null,
  started_at: 1_700_000_010_000,
  finished_at: 1_700_000_012_000,
  files_total: 3,
  files_verified: 2,
  files_failed: 0,
  bytes_written: 2079,
  files_partial: 1,
  files_mismatch: 0,
  same_disk_override: false,
};

const items: RecoveryItemRow[] = [
  { id: 1, scan_item_id: 9, source_path: "/$Carved/x.jpg", output_path: "Found by signature/x.jpg", size: 2048, sha256: "ab", outcome: "partial_source", error: "2 of 4 sectors could not be read; they were written as zeros" },
  { id: 2, scan_item_id: 1, source_path: "/notes.txt", output_path: "USB (FAT32)/notes.txt", size: 11, sha256: "cd", outcome: "verified", error: null },
];

beforeEach(() => {
  vi.mocked(api.listScans).mockReset().mockResolvedValue([scan]);
  vi.mocked(api.scanItemStats)
    .mockReset()
    .mockResolvedValue({ files: 3, deleted: 0, erased: 0, carved: 0, carved_complete: 0, total_bytes: 2079 });
  vi.mocked(api.listVolumes)
    .mockReset()
    .mockResolvedValue([{ offset: 1_048_576, size: 30_000_000_000, filesystem: "FAT32", label: "USB", error: null, files: 3, deleted: 0 }]);
  vi.mocked(api.listFolder).mockReset().mockImplementation(async (_id, scope, dir) => listing(scope, dir));
  vi.mocked(api.listScanItems).mockReset().mockResolvedValue({ items: [], total: 0 });
  vi.mocked(api.getScanItem).mockReset().mockRejectedValue(new Error("not needed"));
  vi.mocked(api.readItemData).mockReset().mockResolvedValue(new Uint8Array());
  vi.mocked(api.pickFolder).mockReset().mockResolvedValue("E:\\Recovered");
  vi.mocked(api.checkRecovery)
    .mockReset()
    .mockImplementation(async (_scan, sel: SelectionPart[], dest) => report_(sel.length === 0 ? 0 : 3, dest ? { ...okCheck, destination: dest } : null));
  vi.mocked(api.startRecovery).mockReset();
  vi.mocked(api.cancelJob).mockReset().mockResolvedValue(true);
  vi.mocked(api.listRecoveries).mockReset().mockResolvedValue([]);
  vi.mocked(api.listRecoveryItems).mockReset().mockResolvedValue(items);
  vi.mocked(api.showInFolder).mockReset().mockResolvedValue(undefined);
});

function renderResults(onStarted = vi.fn()) {
  render(
    <ScanProvider>
      <RecoveryProvider onStarted={onStarted}>
        <ResultsPage />
        <RecoveryPage />
      </RecoveryProvider>
    </ScanProvider>,
  );
  return onStarted;
}

async function list() {
  return screen.findByRole("listbox", { name: "Files and folders" });
}

describe("Selecting files to recover", () => {
  it("selects files and whole folders, and a folder covers its contents", async () => {
    const user = userEvent.setup();
    renderResults();
    const l = await list();
    const bar = screen.getByRole("region", { name: "Selection" });
    expect(within(bar).getByRole("button", { name: /Recover/ })).toBeDisabled();

    await user.click(await within(l).findByRole("checkbox", { name: "Select notes.txt" }));
    await user.click(within(l).getByRole("checkbox", { name: "Select folder docs" }));
    expect(bar).toHaveTextContent("1 file and 1 folder selected");

    // Inside the selected folder, files are covered by it and can't be unticked one by one.
    await user.click(within(l).getByRole("button", { name: "Open docs" }));
    const inner = await within(l).findByRole("checkbox", { name: "Select report.bin" });
    expect(inner).toBeChecked();
    expect(inner).toBeDisabled();

    await user.click(within(bar).getByRole("button", { name: "Clear" }));
    expect(bar).toHaveTextContent("Tick files or folders to recover them.");
  });

  it("selects everything in the current folder at once", async () => {
    const user = userEvent.setup();
    renderResults();
    await list();
    await user.click(await screen.findByRole("checkbox", { name: "Select everything in this folder" }));
    expect(screen.getByRole("region", { name: "Selection" })).toHaveTextContent("0 files and 1 folder selected");
  });
});

describe("Recover dialog", () => {
  async function openDialog(user: ReturnType<typeof userEvent.setup>) {
    const l = await list();
    await user.click(await within(l).findByRole("checkbox", { name: "Select notes.txt" }));
    await user.click(within(l).getByRole("checkbox", { name: "Select folder docs" }));
    await user.click(within(screen.getByRole("region", { name: "Selection" })).getByRole("button", { name: /Recover/ }));
    return screen.getByRole("dialog", { name: "Recover files" });
  }

  it("sends the exact selection, needs a checked destination, then starts", async () => {
    vi.mocked(api.startRecovery).mockReturnValue(new Promise(() => {}));
    const user = userEvent.setup();
    const onStarted = renderResults();
    const dlg = await openDialog(user);

    const parts: SelectionPart[] = [
      { kind: "items", ids: [1] },
      { kind: "folder", scope: { kind: "volume", offset: 1_048_576 }, dir: "/docs", view: "all_files" },
    ];
    expect(api.checkRecovery).toHaveBeenCalledWith(7, parts, null);
    const go = await within(dlg).findByRole("button", { name: "Recover 3 files" });
    expect(go).toBeDisabled();

    await user.click(within(dlg).getByRole("button", { name: "Choose folder…" }));
    expect(await within(dlg).findByText("Destination checks passed.")).toBeInTheDocument();
    expect(api.checkRecovery).toHaveBeenLastCalledWith(7, parts, "E:\\Recovered");
    expect(go).toBeEnabled();

    await user.click(go);
    expect(api.startRecovery).toHaveBeenCalledWith(7, parts, "E:\\Recovered", false, expect.any(String), expect.any(Function));
    expect(onStarted).toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(screen.getByRole("region", { name: "Selection" })).toHaveTextContent("Tick files or folders");
  });

  it("blocks a destination with problems", async () => {
    vi.mocked(api.checkRecovery).mockImplementation(async (_s, _sel, dest) =>
      report_(3, dest ? { ...okCheck, free_bytes: 1000, problems: ["Not enough free space: 1000 B free, 64 MB needed"] } : null),
    );
    const user = userEvent.setup();
    renderResults();
    const dlg = await openDialog(user);
    await user.click(within(dlg).getByRole("button", { name: "Choose folder…" }));
    expect(await within(dlg).findByRole("alert")).toHaveTextContent("Not enough free space");
    expect(within(dlg).getByRole("button", { name: "Recover 3 files" })).toBeDisabled();
  });

  it("refuses the source's own disk unless the risk is explicitly accepted", async () => {
    vi.mocked(api.checkRecovery).mockImplementation(async (_s, _sel, dest) =>
      report_(3, dest ? { ...okCheck, dest_disks: ["PhysicalDrive2"], same_disk: true } : null),
    );
    vi.mocked(api.startRecovery).mockReturnValue(new Promise(() => {}));
    const user = userEvent.setup();
    renderResults();
    const dlg = await openDialog(user);
    await user.click(within(dlg).getByRole("button", { name: "Choose folder…" }));
    expect(await within(dlg).findByText(/same physical disk as the drive you are recovering from/)).toBeInTheDocument();
    expect(within(dlg).queryByText("Destination checks passed.")).toBeNull();
    const go = within(dlg).getByRole("button", { name: "Recover 3 files" });
    expect(go).toBeDisabled();
    await user.click(within(dlg).getByRole("checkbox", { name: /I understand the risk/ }));
    expect(go).toBeEnabled();
    await user.click(go);
    expect(api.startRecovery).toHaveBeenCalledWith(7, expect.anything(), "E:\\Recovered", true, expect.any(String), expect.any(Function));
  });

  it("explains erased files will not be written", async () => {
    vi.mocked(api.checkRecovery).mockResolvedValue({ summary: { files: 3, bytes: 2079, erased: 1, partial: 0 }, check: null });
    const user = userEvent.setup();
    renderResults();
    const dlg = await openDialog(user);
    expect(await within(dlg).findByText(/1 selected file was erased/)).toBeInTheDocument();
    expect(within(dlg).getByRole("button", { name: "Recover 2 files" })).toBeDisabled();
  });
});

describe("Recovery page", () => {
  it("shows an honest empty state", async () => {
    render(
      <RecoveryProvider>
        <RecoveryPage />
      </RecoveryProvider>,
    );
    expect(await screen.findByText("Nothing recovered yet")).toBeInTheDocument();
  });

  it("follows a running recovery, can cancel it, then shows the report", async () => {
    let progress: (p: RecoveryProgress) => void = () => {};
    let finish: (o: RecoveryOutcome) => void = () => {};
    vi.mocked(api.startRecovery).mockImplementation((_s, _sel, _d, _o, _j, onProgress) => {
      progress = onProgress;
      return new Promise((r) => (finish = r));
    });
    const user = userEvent.setup();
    renderResults();
    const l = await list();
    await user.click(await within(l).findByRole("checkbox", { name: "Select notes.txt" }));
    await user.click(within(screen.getByRole("region", { name: "Selection" })).getByRole("button", { name: /Recover/ }));
    const dlg = screen.getByRole("dialog", { name: "Recover files" });
    await user.click(within(dlg).getByRole("button", { name: "Choose folder…" }));
    await user.click(await within(dlg).findByRole("button", { name: "Recover 3 files" }));

    act(() => progress({ files_done: 1, files_total: 3, bytes_done: 1000, bytes_total: 4000, current: "USB (FAT32)/notes.txt" }));
    expect(screen.getByRole("progressbar", { name: "Recovery progress" })).toHaveAttribute("aria-valuenow", "25");
    expect(screen.getByText(/Writing USB \(FAT32\)\/notes.txt/)).toBeInTheDocument();
    expect(screen.getByText("1 of 3 files")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(api.cancelJob).toHaveBeenCalled();

    vi.mocked(api.listRecoveries).mockResolvedValue([done]);
    await act(async () =>
      finish({
        recovery_id: 3,
        status: "completed",
        session_dir: done.destination,
        summary: { files_total: 3, verified: 2, mismatched: 0, partial: 1, failed: 0, bytes_written: 2079, stopped: null },
        error: null,
      }),
    );
    expect(screen.queryByRole("progressbar", { name: "Recovery progress" })).toBeNull();
    expect(await screen.findByText("Recovery report")).toBeInTheDocument();
    expect(await screen.findByText("Found by signature/x.jpg")).toBeInTheDocument();
    expect(screen.getByText(/2 of 4 sectors could not be read/)).toBeInTheDocument();
    expect(screen.getAllByText("Incomplete").length).toBeGreaterThan(0);
  });

  it("reports outcomes per file and opens the folder", async () => {
    vi.mocked(api.listRecoveries).mockResolvedValue([{ ...done, same_disk_override: true }]);
    const user = userEvent.setup();
    render(
      <RecoveryProvider>
        <RecoveryPage />
      </RecoveryProvider>,
    );
    expect(await screen.findByText("Recovery report")).toBeInTheDocument();
    expect(screen.getByText(/same physical disk as the source, by explicit choice/)).toBeInTheDocument();
    const table = await screen.findByRole("table");
    const rows = within(table).getAllByRole("row");
    expect(rows[1]).toHaveTextContent("Found by signature/x.jpg");
    expect(rows[1]).toHaveTextContent("Incomplete");
    expect(rows[2]).toHaveTextContent("Verified");
    await user.click(screen.getByRole("button", { name: "Show in folder" }));
    expect(api.showInFolder).toHaveBeenCalledWith(done.destination);
  });

  it("shows why a recovery could not start", async () => {
    vi.mocked(api.startRecovery).mockRejectedValue({ kind: "unsafe_destination", message: "E:\\Recovered is on the same physical disk as the source" });
    const user = userEvent.setup();
    renderResults();
    const l = await list();
    await user.click(await within(l).findByRole("checkbox", { name: "Select notes.txt" }));
    await user.click(within(screen.getByRole("region", { name: "Selection" })).getByRole("button", { name: /Recover/ }));
    const dlg = screen.getByRole("dialog", { name: "Recover files" });
    await user.click(within(dlg).getByRole("button", { name: "Choose folder…" }));
    await user.click(await within(dlg).findByRole("button", { name: "Recover 3 files" }));
    expect(await screen.findByText(/same physical disk as the source/)).toBeInTheDocument();
  });
});
