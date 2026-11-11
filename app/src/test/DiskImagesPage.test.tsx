import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { FingerprintResult, ImageInfo, JobProgress } from "../lib/types";

vi.mock("../lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/api")>();
  return {
    ...actual,
    api: { pickImageFile: vi.fn(), inspectImage: vi.fn(), fingerprintImage: vi.fn(), cancelJob: vi.fn() },
  };
});

import { api } from "../lib/api";
import { DiskImagesPage } from "../pages/DiskImagesPage";

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

beforeEach(() => {
  vi.mocked(api.pickImageFile).mockReset().mockResolvedValue(info.path);
  vi.mocked(api.inspectImage).mockReset().mockResolvedValue(info);
  vi.mocked(api.fingerprintImage).mockReset();
  vi.mocked(api.cancelJob).mockReset().mockResolvedValue(true);
});

async function openImage() {
  const user = userEvent.setup();
  render(<DiskImagesPage />);
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
    expect(screen.getByRole("button", { name: "Scan image" })).toBeDisabled();
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
