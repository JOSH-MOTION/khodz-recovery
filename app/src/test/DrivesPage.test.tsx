import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PhysicalDisk } from "../lib/types";

vi.mock("../lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/api")>();
  return { ...actual, api: { listDevices: vi.fn(), startScan: vi.fn(), cancelJob: vi.fn() } };
});

import { api } from "../lib/api";
import { ScanProvider } from "../lib/scan";
import { DrivesPage, isExternal } from "../pages/DrivesPage";

// Test fixtures shaped like real output from this project's Windows dev machine.
const systemDisk: PhysicalDisk = {
  id: "PhysicalDrive0",
  display_name: "SanDisk SD9TN8W-256G-1006",
  device_path: "\\\\.\\PhysicalDrive0",
  model: "SanDisk SD9TN8W-256G-1006",
  vendor: null,
  serial: null,
  size_bytes: 256_060_514_304,
  logical_sector_size: 512,
  physical_sector_size: 4096,
  bus: "sata",
  removable: false,
  is_system: true,
  is_virtual: false,
  solid_state: true,
  partition_scheme: null,
  media_present: true,
  volumes: [
    {
      id: "\\\\?\\Volume{a}\\",
      device_path: "\\\\.\\C:",
      mount_points: ["C:\\"],
      label: null,
      filesystem: "NTFS",
      size_bytes: 222_830_784_512,
      free_bytes: 24_034_578_432,
      offset_bytes: 1_048_576,
      encrypted: null,
      is_system: true,
      spans_multiple_disks: false,
    },
  ],
};

const usbStick: PhysicalDisk = {
  ...systemDisk,
  id: "PhysicalDrive2",
  display_name: "SanDisk 3.2Gen1",
  device_path: "\\\\.\\PhysicalDrive2",
  size_bytes: 30_765_219_840,
  bus: "usb",
  removable: true,
  is_system: false,
  solid_state: null,
  volumes: [
    {
      ...systemDisk.volumes[0]!,
      id: "\\\\?\\Volume{f}\\",
      mount_points: ["F:\\"],
      label: "JOSH MOTION",
      filesystem: "FAT32",
      size_bytes: 30_748_409_856,
      free_bytes: 2_484_617_216,
      offset_bytes: 32_256,
      is_system: false,
      encrypted: true,
    },
  ],
};

beforeEach(() => vi.mocked(api.listDevices).mockReset());

describe("DrivesPage", () => {
  it("renders a card per drive with real attributes", async () => {
    vi.mocked(api.listDevices).mockResolvedValue([systemDisk, usbStick]);
    render(<DrivesPage />);

    const sys = await screen.findByRole("button", { name: /SanDisk SD9TN8W-256G-1006/ });
    const sysCard = sys.closest("section")!;
    expect(within(sysCard).getByText("System")).toBeInTheDocument();
    expect(within(sysCard).getByText("SATA")).toBeInTheDocument();
    expect(within(sysCard).getByText("SSD")).toBeInTheDocument();
    expect(within(sysCard).getByText("C:")).toBeInTheDocument();
    expect(within(sysCard).getByText("Local disk")).toBeInTheDocument();
    expect(within(sysCard).getByText("185.1 GiB of 207.5 GiB")).toBeInTheDocument();

    const usbCard = screen.getByRole("button", { name: /SanDisk 3\.2Gen1/ }).closest("section")!;
    expect(within(usbCard).getByText("Removable")).toBeInTheDocument();
    expect(within(usbCard).queryByText("System")).toBeNull();
    expect(within(usbCard).getByText("JOSH MOTION")).toBeInTheDocument();
    expect(within(usbCard).getByText("FAT32")).toBeInTheDocument();

    expect(screen.getByText(/2 drives · 267\.1 GiB total/)).toBeInTheDocument();
  });

  it("starts a read-only device scan through the scan dialog", async () => {
    vi.mocked(api.listDevices).mockResolvedValue([usbStick]);
    vi.mocked(api.startScan).mockReturnValue(new Promise(() => {}));
    const user = userEvent.setup();
    render(
      <ScanProvider>
        <DrivesPage />
      </ScanProvider>,
    );
    await screen.findByText("SanDisk 3.2Gen1");
    await user.click(screen.getByRole("button", { name: "Scan" }));
    const dialog = screen.getByRole("dialog", { name: "Scan SanDisk 3.2Gen1" });
    expect(within(dialog).getByText(/Read-only/)).toBeInTheDocument();
    await user.click(within(dialog).getByRole("radio", { name: /Deep scan/ }));
    await user.click(within(dialog).getByRole("button", { name: "Start scan" }));
    expect(api.startScan).toHaveBeenCalledWith(
      { kind: "device", path: usbStick.device_path, label: "SanDisk 3.2Gen1" },
      "deep",
      expect.any(String),
      expect.any(Function),
    );
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("cannot scan a drive without media", async () => {
    vi.mocked(api.listDevices).mockResolvedValue([{ ...usbStick, media_present: false, volumes: [] }]);
    render(
      <ScanProvider>
        <DrivesPage />
      </ScanProvider>,
    );
    await screen.findByText("No media");
    expect(screen.getByRole("button", { name: "Scan" })).toBeDisabled();
  });

  it("selects a drive on click", async () => {
    vi.mocked(api.listDevices).mockResolvedValue([systemDisk, usbStick]);
    const user = userEvent.setup();
    render(<DrivesPage />);
    const card = await screen.findByRole("button", { name: /SanDisk 3\.2Gen1/ });
    expect(card).toHaveAttribute("aria-pressed", "false");
    await user.click(card);
    expect(card).toHaveAttribute("aria-pressed", "true");
  });

  it("shows an empty state when no drives are reported", async () => {
    vi.mocked(api.listDevices).mockResolvedValue([]);
    render(<DrivesPage />);
    expect(await screen.findByText("No drives found")).toBeInTheDocument();
  });

  it("shows backend errors and recovers on retry", async () => {
    vi.mocked(api.listDevices)
      .mockRejectedValueOnce({ kind: "access_denied", message: "access denied: Full Disk Access required" })
      .mockResolvedValueOnce([systemDisk]);
    const user = userEvent.setup();
    render(<DrivesPage />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Full Disk Access required");
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("SanDisk SD9TN8W-256G-1006")).toBeInTheDocument();
  });

  it("re-enumerates when the window regains focus (hot-plug)", async () => {
    vi.mocked(api.listDevices).mockResolvedValue([systemDisk]);
    render(<DrivesPage />);
    await screen.findByText("SanDisk SD9TN8W-256G-1006");
    vi.mocked(api.listDevices).mockResolvedValue([systemDisk, usbStick]);
    window.dispatchEvent(new Event("focus"));
    expect(await screen.findByText("SanDisk 3.2Gen1")).toBeInTheDocument();
  });

  it("classifies external drives", () => {
    expect(isExternal(usbStick)).toBe(true);
    expect(isExternal(systemDisk)).toBe(false);
    expect(isExternal({ ...systemDisk, bus: "thunderbolt" })).toBe(true);
  });
});
