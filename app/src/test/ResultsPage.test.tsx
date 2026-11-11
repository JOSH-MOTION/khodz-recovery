import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Scan, ScanItemRow, ScanProgress } from "../lib/types";

vi.mock("../lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/api")>();
  return {
    ...actual,
    api: {
      listScans: vi.fn(),
      listScanItems: vi.fn(),
      scanItemStats: vi.fn(),
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
    path: "/docs/report.bin",
    name: "report.bin",
    size: 102_400,
    is_dir: false,
    is_deleted: false,
    name_complete: true,
    origin: "metadata",
    extent_quality: "exact",
    filesystem: "NTFS",
    file_type: "bin",
    modified_at: null,
    data_state: "present",
    carve_status: null,
    note: null,
    ...over,
  };
}

const rows = [
  row({ id: 1 }),
  row({ id: 2, path: "/to-delete-1.bin", name: "to-delete-1.bin", is_deleted: true, data_state: "erased", note: "data erased (zeros), likely TRIM" }),
  row({ id: 3, path: "/$Carved/jpg/jpg_000000010000.jpg", name: "jpg_000000010000.jpg", origin: "carved", carve_status: "complete", filesystem: null, is_deleted: true }),
  row({ id: 4, path: "/$Carved/png/png_0000000BE000.png", name: "png_0000000BE000.png", origin: "carved", carve_status: "partial", note: "chunk CRC mismatch", filesystem: null, is_deleted: true }),
];

beforeEach(() => {
  vi.mocked(api.listScans).mockReset().mockResolvedValue([scan]);
  vi.mocked(api.listScanItems).mockReset().mockResolvedValue({ items: rows, total: 4 });
  vi.mocked(api.scanItemStats)
    .mockReset()
    .mockResolvedValue({ files: 4, deleted: 1, erased: 1, carved: 2, carved_complete: 1, total_bytes: 409_600 });
  vi.mocked(api.startScan).mockReset();
});

describe("itemStatus", () => {
  it("labels every state honestly", () => {
    expect(itemStatus(rows[0]!).text).toBe("Existing");
    expect(itemStatus(rows[1]!)).toMatchObject({ text: "Deleted · erased", tone: "danger" });
    expect(itemStatus(rows[2]!)).toMatchObject({ text: "Verified", tone: "ok" });
    expect(itemStatus(rows[3]!)).toMatchObject({ text: "Partial", tone: "warn" });
    expect(itemStatus(row({ origin: "carved", carve_status: "unverified" })).text).toBe("Unverified");
    expect(itemStatus(row({ is_deleted: true, extent_quality: "partial" })).text).toBe("Deleted · partial");
  });
});

describe("ResultsPage", () => {
  it("shows the latest scan's stats and items with statuses", async () => {
    render(
      <ScanProvider>
        <ResultsPage />
      </ScanProvider>,
    );
    expect(await screen.findByText("report.bin")).toBeInTheDocument();
    expect(screen.getByText("Deleted · erased")).toBeInTheDocument();
    expect(screen.getByText("Verified")).toBeInTheDocument();
    expect(screen.getByText("Partial")).toBeInTheDocument();
    expect(screen.getByText("1 verified")).toBeInTheDocument();
    expect(screen.getByText("Showing 4 of 4")).toBeInTheDocument();
    expect(api.listScanItems).toHaveBeenCalledWith(7, { view: "all_files", search: null }, { limit: 200, offset: 0 });
  });

  it("filters by view and debounced search", async () => {
    const user = userEvent.setup();
    render(
      <ScanProvider>
        <ResultsPage />
      </ScanProvider>,
    );
    await screen.findByText("report.bin");
    await user.click(screen.getByRole("tab", { name: "Deleted" }));
    expect(api.listScanItems).toHaveBeenLastCalledWith(7, { view: "deleted", search: null }, expect.anything());
    await user.type(screen.getByRole("searchbox", { name: "Search files" }), "report");
    await vi.waitFor(() =>
      expect(api.listScanItems).toHaveBeenLastCalledWith(7, { view: "deleted", search: "report" }, expect.anything()),
    );
  });

  it("shows an empty state before any scan", async () => {
    vi.mocked(api.listScans).mockResolvedValue([]);
    render(
      <ScanProvider>
        <ResultsPage />
      </ScanProvider>,
    );
    expect(await screen.findByText("No scan results yet")).toBeInTheDocument();
  });

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
    expect(await screen.findByText(/administrator rights are required/)).toBeInTheDocument();
  });
});
