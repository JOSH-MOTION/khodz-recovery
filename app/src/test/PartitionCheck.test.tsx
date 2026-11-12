import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { AnalyzedVolume, LayoutReport, SearchOutcome, SearchProgress } from "../lib/types";

vi.mock("../lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/api")>();
  return {
    ...actual,
    api: { inspectPartitions: vi.fn(), searchPartitions: vi.fn(), cancelJob: vi.fn(), startScan: vi.fn() },
  };
});

import { api } from "../lib/api";
import { ScanProvider } from "../lib/scan";
import { PartitionCheck, bytesToSearch } from "../components/PartitionCheck";

const MIB = 1 << 20;
const source = { kind: "image" as const, path: "D:\\lost.img" };

function volume(over: Partial<AnalyzedVolume>): AnalyzedVolume {
  return {
    origin: "search",
    partition: null,
    type_name: null,
    offset: 65_536,
    size: 40 * MIB,
    filesystem: "fat32",
    label: "PFAT",
    verified: true,
    opens: true,
    boot_backup: null,
    notes: [],
    problems: [],
    ...over,
  };
}

// Shaped like the real analysis of fixtures/out/mbr-multi.img with its MBR erased.
const erased: LayoutReport = {
  disk_size: 160 * MIB,
  table: { kind: "unknown", sector_size: 512, disk_signature: null, disk_guid: null, partitions: [], warnings: ["no 0x55AA boot signature in sector 0"] },
  volumes: [],
  issues: [
    "No partition table and no filesystem at the start of the disk. The table may have been erased — search for lost partitions.",
  ],
  unallocated: [[0, 160 * MIB]],
};

const found: AnalyzedVolume[] = [
  volume({}),
  volume({ offset: 42_074_112, filesystem: "ntfs", label: "LNTFS", boot_backup: 84_016_640, notes: ["Found through its backup boot record; the primary boot sector is damaged."] }),
  volume({ offset: 84_082_688, filesystem: "apfs", label: null, opens: false, notes: ["APFS volumes are detected only; their files cannot be listed yet."] }),
];

beforeEach(() => {
  vi.mocked(api.inspectPartitions).mockReset().mockResolvedValue(erased);
  vi.mocked(api.searchPartitions).mockReset();
  vi.mocked(api.cancelJob).mockReset().mockResolvedValue(true);
  vi.mocked(api.startScan).mockReset().mockReturnValue(new Promise(() => {}));
});

function renderCheck() {
  return render(
    <ScanProvider>
      <PartitionCheck source={source} label="lost.img" />
    </ScanProvider>,
  );
}

describe("bytesToSearch", () => {
  it("counts only space not covered by verified volumes", () => {
    const layout: LayoutReport = {
      ...erased,
      volumes: [volume({ origin: "table", offset: 0, size: 10 * MIB }), volume({ origin: "table", offset: 50 * MIB, size: 10 * MIB, verified: false })],
    };
    expect(bytesToSearch(layout)).toBe(150 * MIB);
    expect(bytesToSearch({ ...layout, volumes: [volume({ offset: 0, size: 160 * MIB })] })).toBe(0);
  });
});

describe("PartitionCheck", () => {
  it("says so when the table matches the disk", async () => {
    vi.mocked(api.inspectPartitions).mockResolvedValue({
      ...erased,
      table: { ...erased.table, kind: "mbr", warnings: [] },
      volumes: [volume({ origin: "table", partition: 1, offset: 0, size: 160 * MIB })],
      issues: [],
      unallocated: [],
    });
    renderCheck();
    expect(await screen.findByText("The partition table matches what is on the disk.")).toBeInTheDocument();
    expect(screen.getByText(/there is nowhere for a lost partition to be/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Search for lost partitions/ })).toBeNull();
  });

  it("lists table problems and unpartitioned space", async () => {
    renderCheck();
    const list = await screen.findByRole("list", { name: "Partition problems" });
    expect(within(list).getByText(/table may have been erased/)).toBeInTheDocument();
    expect(screen.getByText("160.0 MiB of this disk is not in any partition.")).toBeInTheDocument();
    expect(screen.getByText(/Searches 160\.0 MiB not covered/)).toBeInTheDocument();
  });

  it("searches with progress and cancel, then scans only the chosen partitions", async () => {
    let report: (p: SearchProgress) => void = () => {};
    let finish: (o: SearchOutcome) => void = () => {};
    vi.mocked(api.searchPartitions).mockImplementation((_s, _id, onProgress) => {
      report = onProgress;
      return new Promise((r) => (finish = r));
    });
    const user = userEvent.setup();
    renderCheck();
    await user.click(await screen.findByRole("button", { name: "Search for lost partitions" }));
    act(() => report({ bytes_done: 40 * MIB, bytes_total: 160 * MIB, found: 1 }));
    expect(screen.getByRole("progressbar", { name: "Partition search progress" })).toHaveAttribute("aria-valuenow", "25");
    expect(screen.getByText(/40\.0 MiB of 160\.0 MiB · 1 found/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(api.cancelJob).toHaveBeenCalledWith(vi.mocked(api.searchPartitions).mock.calls[0]![1]);

    await act(async () => finish({ layout: erased, found, searched_bytes: 160 * MIB }));
    const list = screen.getByRole("list", { name: "Found partitions" });
    expect(within(list).getByText("PFAT (FAT32)")).toBeInTheDocument();
    expect(within(list).getByText(/Found through its backup boot record/)).toBeInTheDocument();
    expect(within(list).getByText("Detected only — files cannot be listed")).toBeInTheDocument();
    // Readable volumes are pre-selected; detected-only ones are not.
    expect(within(list).getByRole("checkbox", { name: /Include PFAT/ })).toBeChecked();
    expect(within(list).getByRole("checkbox", { name: /Include APFS volume/ })).not.toBeChecked();

    await user.click(screen.getByRole("button", { name: "Scan 2 partitions…" }));
    const dialog = screen.getByRole("dialog", { name: "Scan lost.img (found partitions)" });
    expect(within(dialog).getByText(/Only the 2 partitions you selected/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("button", { name: "Start scan" }));
    expect(api.startScan).toHaveBeenCalledWith(source, "quick", expect.any(String), expect.any(Function), [
      { offset: 65_536, size: 40 * MIB, boot_backup: null },
      { offset: 42_074_112, size: 40 * MIB, boot_backup: 84_016_640 },
    ]);
  });

  it("reports a search that found nothing, and cancellations", async () => {
    vi.mocked(api.searchPartitions).mockResolvedValueOnce({ layout: erased, found: [], searched_bytes: 160 * MIB });
    const user = userEvent.setup();
    renderCheck();
    await user.click(await screen.findByRole("button", { name: "Search for lost partitions" }));
    expect(await screen.findByText(/No lost partitions found in 160\.0 MiB/)).toBeInTheDocument();
    vi.mocked(api.searchPartitions).mockRejectedValueOnce({ kind: "cancelled", message: "operation cancelled" });
    await user.click(screen.getByRole("button", { name: "Search again" }));
    expect(await screen.findByText("Search cancelled.")).toBeInTheDocument();
  });

  it("explains that drives need administrator rights", async () => {
    vi.mocked(api.inspectPartitions).mockRejectedValue({ kind: "access_denied", message: "access denied" });
    render(
      <ScanProvider>
        <PartitionCheck source={{ kind: "device", path: "\\\\.\\PhysicalDrive2", label: "USB" }} label="USB" />
      </ScanProvider>,
    );
    expect(await screen.findByRole("alert")).toHaveTextContent(/needs Khodz Recovery to run as administrator/);
  });
});
