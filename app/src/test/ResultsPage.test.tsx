import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { FolderListing, Scan, ScanItemDetail, ScanItemRow, ScanProgress, Scope } from "../lib/types";

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
    },
  };
});

import { api } from "../lib/api";
import { ScanProvider, useScan } from "../lib/scan";
import { ResultsPage, itemStatus } from "../pages/ResultsPage";

const scan: Scan = {
  id: 7,
  source_kind: "image",
  source_path: "D:\\mbr-multi.img",
  source_label: "mbr-multi.img",
  source_size: 167_772_160,
  scan_mode: "deep",
  status: "completed",
  error: null,
  started_at: 1_700_000_000_000,
  finished_at: 1_700_000_005_000,
  items_found: 4,
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
    filesystem: "NTFS",
    file_type: "txt",
    modified_at: null,
    data_state: "present",
    carve_status: null,
    note: null,
    ...over,
  };
}

const notes = row({ id: 1 });
const erased = row({ id: 2, path: "/to-delete-1.bin", name: "to-delete-1.bin", file_type: "bin", size: 4096, is_deleted: true, data_state: "erased" });
const photo = row({ id: 3, path: "/photo.jpg", name: "photo.jpg", file_type: "jpg", size: 2048 });
const blob = row({ id: 4, path: "/data.bin", name: "data.bin", file_type: "bin", size: 64 });
const report = row({ id: 5, path: "/docs/report.bin", name: "report.bin", file_type: "bin", size: 20 });

const content: Record<number, Uint8Array> = {
  1: new TextEncoder().encode("hello world"),
  2: new Uint8Array(4096),
  3: new Uint8Array([0xff, 0xd8, 0xff, 0xe0, 1, 2, 3]),
  4: Uint8Array.from({ length: 64 }, (_, i) => (i * 7) & 0xff),
  5: Uint8Array.from({ length: 20 }, (_, i) => i),
};

function listing(scope: Scope, dir: string): FolderListing {
  if (scope.kind === "carved") return { folders: [{ name: "$Carved", path: "/$Carved", files: 1, deleted: 1 }], files: [], total_files: 0 };
  if (dir === "/docs") return { folders: [], files: [report], total_files: 1 };
  return { folders: [{ name: "docs", path: "/docs", files: 1, deleted: 0 }], files: [blob, notes, photo, erased], total_files: 4 };
}

beforeEach(() => {
  vi.mocked(api.listScans).mockReset().mockResolvedValue([scan]);
  vi.mocked(api.scanItemStats)
    .mockReset()
    .mockResolvedValue({ files: 5, deleted: 1, erased: 1, carved: 1, carved_complete: 1, total_bytes: 10_000 });
  vi.mocked(api.listVolumes)
    .mockReset()
    .mockResolvedValue([
      { offset: 65_536, size: 41_943_040, filesystem: "NTFS", label: "LNTFS", error: null, files: 5, deleted: 1 },
    ]);
  vi.mocked(api.listFolder).mockReset().mockImplementation(async (_id, scope, dir) => listing(scope, dir));
  vi.mocked(api.getScanItem)
    .mockReset()
    .mockImplementation(async (id) => {
      const r = [notes, erased, photo, blob, report].find((x) => x.id === id)!;
      return { ...r, scan_id: 7, volume_offset: 65_536, extents: [[1_048_576, r.size]], resident: null, created_at: null } as ScanItemDetail;
    });
  vi.mocked(api.readItemData).mockReset().mockImplementation(async (id) => content[id]!);
  vi.mocked(api.listScanItems).mockReset().mockResolvedValue({ items: [report], total: 1 });
  vi.mocked(api.startScan).mockReset();
  URL.createObjectURL = vi.fn(() => "blob:preview");
  URL.revokeObjectURL = vi.fn();
});

function renderPage() {
  return render(
    <ScanProvider>
      <ResultsPage />
    </ScanProvider>,
  );
}

async function selectFile(user: ReturnType<typeof userEvent.setup>, name: string) {
  const list = await screen.findByRole("listbox", { name: "Files and folders" });
  await user.click(await within(list).findByText(name));
}

describe("itemStatus", () => {
  it("labels every state honestly", () => {
    expect(itemStatus(notes).text).toBe("Existing");
    expect(itemStatus(erased)).toMatchObject({ text: "Deleted · erased", tone: "danger" });
    expect(itemStatus(row({ origin: "carved", carve_status: "complete" }))).toMatchObject({ text: "Verified", tone: "ok" });
    expect(itemStatus(row({ origin: "carved", carve_status: "partial" }))).toMatchObject({ text: "Partial", tone: "warn" });
    expect(itemStatus(row({ origin: "carved", carve_status: "unverified" })).text).toBe("Unverified");
    expect(itemStatus(row({ is_deleted: true, extent_quality: "partial" })).text).toBe("Deleted · partial");
  });
});

describe("Results explorer", () => {
  it("shows locations, folders and files of the first volume", async () => {
    renderPage();
    const nav = await screen.findByRole("navigation", { name: "Locations" });
    expect(await within(nav).findByText("LNTFS (NTFS)")).toBeInTheDocument();
    expect(within(nav).getByText("Found by signature")).toBeInTheDocument();
    const list = await screen.findByRole("listbox", { name: "Files and folders" });
    expect(await within(list).findByText("docs")).toBeInTheDocument();
    expect(within(list).getByText("notes.txt")).toBeInTheDocument();
    expect(within(list).getByText("Deleted · erased")).toBeInTheDocument();
    expect(api.listFolder).toHaveBeenCalledWith(7, { kind: "volume", offset: 65_536 }, "/", "all_files", { limit: 200, offset: 0 });
  });

  it("navigates into folders and back via the breadcrumb and keyboard", async () => {
    const user = userEvent.setup();
    renderPage();
    await user.click(await screen.findByRole("button", { name: "Open docs" }));
    const list = screen.getByRole("listbox", { name: "Files and folders" });
    expect(await within(list).findByText("report.bin")).toBeInTheDocument();
    expect(api.listFolder).toHaveBeenLastCalledWith(7, { kind: "volume", offset: 65_536 }, "/docs", "all_files", expect.anything());
    list.focus();
    await user.keyboard("{Backspace}");
    expect(await within(list).findByText("notes.txt")).toBeInTheDocument();
  });

  it("switches to the carved location", async () => {
    const user = userEvent.setup();
    renderPage();
    const nav = await screen.findByRole("navigation", { name: "Locations" });
    await user.click(await within(nav).findByText("Found by signature"));
    expect(api.listFolder).toHaveBeenLastCalledWith(7, { kind: "carved" }, "/", "all_files", expect.anything());
  });

  it("uses a flat list while searching", async () => {
    const user = userEvent.setup();
    renderPage();
    await screen.findByRole("listbox", { name: "Files and folders" });
    await user.type(screen.getByRole("searchbox", { name: "Search files" }), "report");
    const results = await screen.findByRole("listbox", { name: "Search results" });
    expect(await within(results).findByText("report.bin")).toBeInTheDocument();
    expect(api.listScanItems).toHaveBeenLastCalledWith(7, { view: "all_files", search: "report" }, { limit: 200, offset: 0 });
  });
});

describe("Preview pane", () => {
  it("previews text, hex and details", async () => {
    const user = userEvent.setup();
    renderPage();
    await selectFile(user, "notes.txt");
    const pane = screen.getByRole("complementary", { name: "Preview" });
    expect(await within(pane).findByText("hello world")).toBeInTheDocument();
    await user.click(within(pane).getByRole("tab", { name: "hex" }));
    expect(within(pane).getByRole("table", { name: "Hex view" })).toHaveTextContent("68 65 6c 6c 6f");
    await user.click(within(pane).getByRole("tab", { name: "details" }));
    expect(within(pane).getByText(/@ 1,048,576 · 11 bytes/)).toBeInTheDocument();
    expect(within(pane).getByText(/Exact — taken from intact filesystem records/)).toBeInTheDocument();
  });

  it("says plainly when data was erased", async () => {
    const user = userEvent.setup();
    renderPage();
    await selectFile(user, "to-delete-1.bin");
    const pane = screen.getByRole("complementary", { name: "Preview" });
    expect(await within(pane).findByText(/reads back as zeros/)).toBeInTheDocument();
  });

  it("decodes images through the browser and reports decode failures", async () => {
    const user = userEvent.setup();
    renderPage();
    await selectFile(user, "photo.jpg");
    const pane = screen.getByRole("complementary", { name: "Preview" });
    const img = await within(pane).findByRole("img", { name: "Preview" });
    expect(img).toHaveAttribute("src", "blob:preview");
    act(() => {
      fireEvent.error(img);
    });
    expect(within(pane).getByText(/could not be decoded/)).toBeInTheDocument();
  });

  it("does not claim a partial image decoded successfully", async () => {
    const partial = { ...photo, extent_quality: "partial" as const };
    vi.mocked(api.listFolder).mockImplementation(async () => ({ folders: [], files: [partial], total_files: 1 }));
    const user = userEvent.setup();
    renderPage();
    await selectFile(user, "photo.jpg");
    const pane = screen.getByRole("complementary", { name: "Preview" });
    const img = await within(pane).findByRole("img", { name: "Preview" });
    Object.defineProperty(img, "naturalWidth", { value: 800 });
    Object.defineProperty(img, "naturalHeight", { value: 600 });
    act(() => {
      fireEvent.load(img);
    });
    expect(within(pane).getByText(/missing parts of the image are shown blank/)).toBeInTheDocument();
    expect(within(pane).queryByText(/decoded successfully/)).toBeNull();
    expect(within(pane).getByText(/preview may show damaged content/)).toBeInTheDocument();
  });

  it("falls back to hex for unknown binary data", async () => {
    const user = userEvent.setup();
    renderPage();
    await selectFile(user, "data.bin");
    const pane = screen.getByRole("complementary", { name: "Preview" });
    expect(await within(pane).findByText(/No visual preview/)).toBeInTheDocument();
    expect(within(pane).getByRole("table", { name: "Hex view" })).toBeInTheDocument();
  });

  it("shows read errors (e.g. source missing)", async () => {
    vi.mocked(api.readItemData).mockRejectedValue({ kind: "invalid_data", message: "D:\\mbr-multi.img does not exist" });
    const user = userEvent.setup();
    renderPage();
    await selectFile(user, "notes.txt");
    expect(await screen.findByRole("alert")).toHaveTextContent("does not exist");
  });
});

describe("Scan progress on the Results page", () => {
  it("shows live progress for a running scan and can cancel it", async () => {
    vi.mocked(api.listScans).mockResolvedValue([]);
    let report: (p: ScanProgress) => void = () => {};
    vi.mocked(api.startScan).mockImplementation((_s, _m, _id, onProgress) => {
      report = onProgress;
      return new Promise(() => {});
    });
    function Starter() {
      const { start } = useScan();
      return <button onClick={() => start({ kind: "image", path: "x.img" }, "x.img", "deep")}>go</button>;
    }
    const user = userEvent.setup();
    render(
      <ScanProvider>
        <Starter />
        <ResultsPage />
      </ScanProvider>,
    );
    await user.click(screen.getByRole("button", { name: "go" }));
    act(() => report({ phase: "carving", bytes_done: 50, bytes_total: 200, items_found: 12, detail: null }));
    const panel = screen.getByText("Deep scan of x.img").closest("section")!;
    expect(within(panel).getByRole("progressbar")).toHaveAttribute("aria-valuenow", "25");
    expect(within(panel).getByText("12 items found")).toBeInTheDocument();
    await user.click(within(panel).getByRole("button", { name: "Cancel" }));
    expect(api.cancelJob).toHaveBeenCalled();
  });

  it("reports a scan that could not start (e.g. no administrator rights)", async () => {
    vi.mocked(api.startScan).mockRejectedValue({
      kind: "access_denied",
      message: "access denied: administrator rights are required to read \\\\.\\PhysicalDrive2 directly",
    });
    function Starter() {
      const { start } = useScan();
      return <button onClick={() => start({ kind: "device", path: "\\\\.\\PhysicalDrive2", label: null }, "USB", "quick")}>go</button>;
    }
    const user = userEvent.setup();
    render(
      <ScanProvider>
        <Starter />
        <ResultsPage />
      </ScanProvider>,
    );
    await user.click(screen.getByRole("button", { name: "go" }));
    expect(await screen.findByText(/administrator rights are required/)).toHaveTextContent(/Run as administrator/);
  });

  it("opens signature results when no volume has files", async () => {
    vi.mocked(api.listVolumes).mockResolvedValue([
      { offset: 0, size: 995_328, filesystem: "Unknown", label: null, error: null, files: 0, deleted: 0 },
    ]);
    renderPage();
    await vi.waitFor(() =>
      expect(api.listFolder).toHaveBeenCalledWith(7, { kind: "carved" }, "/", "all_files", expect.anything()),
    );
    const nav = await screen.findByRole("navigation", { name: "Locations" });
    expect(within(nav).queryByText("Unknown volume")).toBeNull();
  });
});
