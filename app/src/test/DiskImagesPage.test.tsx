import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { FingerprintResult, FsKind, ImageAnalysis, ImageInfo, JobProgress } from "../lib/types";

vi.mock("../lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/api")>();
  return {
    ...actual,
    api: {
      pickImageFile: vi.fn(),
      inspectImage: vi.fn(),
      analyzeImage: vi.fn(),
      fingerprintImage: vi.fn(),
      cancelJob: vi.fn(),
      startScan: vi.fn(),
    },
  };
});

import { api } from "../lib/api";
import { DiskImagesPage } from "../pages/DiskImagesPage";
import { ScanProvider } from "../lib/scan";

const info: ImageInfo = {
  path: "D:\\images\\usb-stick.img",
  file_name: "usb-stick.img",
  size_bytes: 30_765_219_840,
  sector_size: 512,
  modified_ms: 1_700_000_000_000,
  file_read_only: true,
};

const result: FingerprintResult = {
  sha256: "a".repeat(64),
  bytes: info.size_bytes,
  unreadable_bytes: 0,
  unreadable_ranges: [],
  elapsed_ms: 12_300,
};

// Shaped like the real analysis of fixtures/out/mbr-multi.img.
const analysis: ImageAnalysis = {
  table: { kind: "mbr", sector_size: 512, disk_signature: 0x1a2b3c4d, disk_guid: null, partitions: [], warnings: [] },
  volumes: [
    vol(1, 65_536, 41_943_040, "FAT32", "fat32", "PFAT", true),
    vol(5, 42_074_112, 41_943_040, "NTFS / exFAT", "ntfs", "LNTFS", true),
    vol(6, 84_148_224, 41_943_040, "NTFS / exFAT", "ex_fat", "LEXFAT", true),
  ],
};

function vol(n: number, offset: number, size: number, type: string, kind: FsKind, label: string, walkable: boolean) {
  return {
    partition: n,
    offset,
    size,
    type_name: type,
    walkable,
    fs: { kind, label, serial: null, cluster_size: 4096, sector_size: 512, volume_size: size, encrypted: false, notes: [] },
  };
}

beforeEach(() => {
  vi.mocked(api.pickImageFile).mockReset().mockResolvedValue(info.path);
  vi.mocked(api.inspectImage).mockReset().mockResolvedValue(info);
  vi.mocked(api.analyzeImage).mockReset().mockResolvedValue(analysis);
  vi.mocked(api.fingerprintImage).mockReset();
  vi.mocked(api.cancelJob).mockReset().mockResolvedValue(true);
});

async function openImage() {
  const user = userEvent.setup();
  render(
    <ScanProvider>
      <DiskImagesPage />
    </ScanProvider>,
  );
  await user.click(screen.getByRole("button", { name: "Open image" }));
  await screen.findByText("usb-stick.img");
  return user;
}

describe("DiskImagesPage", () => {
  it("starts empty and explains supported formats", () => {
    render(<DiskImagesPage />);
    expect(screen.getByText("No image open")).toBeInTheDocument();
    expect(screen.getByText(/\.img, \.dd/)).toBeInTheDocument();
  });

  it("opens an image and shows its real properties", async () => {
    await openImage();
    expect(api.inspectImage).toHaveBeenCalledWith(info.path);
    expect(screen.getByText("28.7 GiB")).toBeInTheDocument();
    expect(screen.getByText("30,765,219,840 bytes")).toBeInTheDocument();
    expect(screen.getByText("Opened read-only")).toBeInTheDocument();
    expect(screen.getByText("File marked read-only")).toBeInTheDocument();
  });

  it("starts a quick scan of the open image", async () => {
    vi.mocked(api.startScan).mockReturnValue(new Promise(() => {}));
    const user = await openImage();
    await user.click(screen.getByRole("button", { name: "Scan image" }));
    await user.click(screen.getByRole("button", { name: "Start scan" }));
    expect(api.startScan).toHaveBeenCalledWith({ kind: "image", path: info.path }, "quick", expect.any(String), expect.any(Function));
  });

  it("does nothing when the file dialog is cancelled", async () => {
    vi.mocked(api.pickImageFile).mockResolvedValue(null);
    const user = userEvent.setup();
    render(<DiskImagesPage />);
    await user.click(screen.getByRole("button", { name: "Open image" }));
    expect(api.inspectImage).not.toHaveBeenCalled();
    expect(screen.getByText("No image open")).toBeInTheDocument();
  });

  it("shows open errors", async () => {
    vi.mocked(api.inspectImage).mockRejectedValue({ kind: "access_denied", message: "access denied: locked file" });
    const user = userEvent.setup();
    render(<DiskImagesPage />);
    await user.click(screen.getByRole("button", { name: "Open image" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("locked file");
  });

  it("streams fingerprint progress then shows the hash", async () => {
    let report: (p: JobProgress) => void = () => {};
    let finish: (r: FingerprintResult) => void = () => {};
    vi.mocked(api.fingerprintImage).mockImplementation((_path, _id, onProgress) => {
      report = onProgress;
      return new Promise((resolve) => (finish = resolve));
    });
    const user = await openImage();
    await user.click(screen.getByRole("button", { name: "Compute SHA-256" }));

    report({ bytes_done: info.size_bytes / 2, bytes_total: info.size_bytes });
    const bar = await screen.findByRole("progressbar");
    expect(bar).toHaveAttribute("aria-valuenow", "50");
    expect(screen.getByRole("button", { name: "Open image" })).toBeDisabled();

    finish(result);
    expect(await screen.findByLabelText("SHA-256")).toHaveTextContent(result.sha256);
    expect(screen.getByText(/hashed in 12\.3s/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open image" })).toBeEnabled();
  });

  it("cancels a running fingerprint", async () => {
    let fail: (e: unknown) => void = () => {};
    vi.mocked(api.fingerprintImage).mockImplementation(() => new Promise((_, reject) => (fail = reject)));
    const user = await openImage();
    await user.click(screen.getByRole("button", { name: "Compute SHA-256" }));
    await user.click(await screen.findByRole("button", { name: "Cancel" }));
    expect(api.cancelJob).toHaveBeenCalledWith(vi.mocked(api.fingerprintImage).mock.calls[0]![1]);
    fail({ kind: "cancelled", message: "operation cancelled" });
    expect(await screen.findByText("Cancelled.")).toBeInTheDocument();
  });

  it("shows the partition table and every filesystem", async () => {
    await openImage();
    expect(await screen.findByText("MBR partition table")).toBeInTheDocument();
    expect(screen.getByText("Signature 1A2B3C4D")).toBeInTheDocument();
    for (const label of ["PFAT", "LNTFS", "LEXFAT"]) expect(screen.getByText(label)).toBeInTheDocument();
    expect(screen.getAllByText("Readable")).toHaveLength(3);
    expect(screen.getByRole("img", { name: "Partition map with 3 volumes" })).toBeInTheDocument();
  });

  it("states plainly when a filesystem is only detected or encrypted", async () => {
    vi.mocked(api.analyzeImage).mockResolvedValue({
      table: { ...analysis.table, kind: "gpt", disk_guid: "11111111-2222-3333-4444-555555555555", warnings: ["backup GPT header problem: header CRC mismatch"] },
      volumes: [
        vol(1, 1_048_576, 1e9, "Apple APFS", "apfs", "", false),
        { ...vol(2, 2e9, 1e9, "Microsoft basic data", "bit_locker", "", false), fs: { ...vol(2, 0, 0, "", "bit_locker", "", false).fs, label: null, encrypted: true } },
      ],
    });
    await openImage();
    expect(await screen.findByText("Detected only")).toBeInTheDocument();
    expect(screen.getByText("Encrypted — unlock in the OS first")).toBeInTheDocument();
    expect(screen.getByRole("alert")).toHaveTextContent("backup GPT header problem");
  });

  it("never hides unreadable data in a fingerprint", async () => {
    vi.mocked(api.fingerprintImage).mockResolvedValue({
      ...result,
      unreadable_bytes: 4096,
      unreadable_ranges: [[512, 4608]],
    });
    const user = await openImage();
    await user.click(screen.getByRole("button", { name: "Compute SHA-256" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("4.0 KiB could not be read");
  });
});
