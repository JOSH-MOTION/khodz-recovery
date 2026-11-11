import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Scan } from "../lib/types";

// The IPC boundary is the only thing mocked; every component above it is real.
const settings = new Map<string, string>();
const scans: Scan[] = [];

vi.mock("../lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../lib/api")>();
  return {
    ...actual,
    api: {
      appInfo: vi.fn(async () => ({
        name: "Khodz Recovery",
        version: "0.1.0",
        os: "windows",
        arch: "x86_64",
        db_path: "C:\\data\\khodz.db",
        schema_version: 1,
      })),
      getSetting: vi.fn(async (k: string) => settings.get(k) ?? null),
      setSetting: vi.fn(async (k: string, v: string) => void settings.set(k, v)),
      listScans: vi.fn(async () => scans),
      listRecoveries: vi.fn(async () => []),
    },
  };
});

import App from "../App";
import { api } from "../lib/api";

beforeEach(() => {
  settings.clear();
  scans.length = 0;
  vi.clearAllMocks();
});

describe("App shell", () => {
  it("starts on Drives and shows no fabricated devices", () => {
    render(<App />);
    expect(screen.getByRole("heading", { level: 1, name: "Drives" })).toBeInTheDocument();
    expect(screen.getByText("Drive detection isn't available yet")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Refresh" })).toBeDisabled();
  });

  it("navigates between every section", async () => {
    const user = userEvent.setup();
    render(<App />);
    const nav = screen.getByRole("navigation", { name: "Main" });
    for (const [label, heading] of [
      ["Disk Images", "Disk Images"],
      ["Results", "Results"],
      ["Recovery", "Recovery"],
      ["History", "History"],
      ["Drives", "Drives"],
    ] as const) {
      await user.click(within(nav).getByRole("button", { name: label }));
      expect(screen.getByRole("heading", { level: 1, name: heading })).toBeInTheDocument();
      expect(within(nav).getByRole("button", { name: label })).toHaveAttribute("aria-current", "page");
    }
    await user.click(screen.getByRole("button", { name: "Settings" }));
    expect(screen.getByRole("heading", { level: 1, name: "Settings" })).toBeInTheDocument();
  });

  it("applies the stored theme on startup", async () => {
    settings.set("theme", "light");
    render(<App />);
    await waitFor(() => expect(document.documentElement.dataset.theme).toBe("light"));
  });

  it("persists theme changes through the backend", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.click(screen.getByRole("button", { name: "Settings" }));
    await user.click(screen.getByRole("radio", { name: "Light" }));
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(api.setSetting).toHaveBeenCalledWith("theme", "light");
    expect(screen.getByRole("radio", { name: "Light" })).toHaveAttribute("aria-checked", "true");
  });

  it("shows real app info from the backend", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.click(screen.getByRole("button", { name: "Settings" }));
    expect(await screen.findByText("C:\\data\\khodz.db")).toBeInTheDocument();
    expect(screen.getByText("v1")).toBeInTheDocument();
  });
});

describe("History", () => {
  it("shows an empty state when there are no scans", async () => {
    const user = userEvent.setup();
    render(<App />);
    await user.click(screen.getByRole("button", { name: "History" }));
    expect(await screen.findByText("No scans yet")).toBeInTheDocument();
  });

  it("lists scans returned by the backend", async () => {
    scans.push({
      id: 1,
      source_kind: "image",
      source_path: "D:\\images\\usb.img",
      source_label: "USB stick image",
      source_size: 8 * 1024 ** 3,
      scan_mode: "deep",
      status: "completed",
      error: null,
      started_at: 1_700_000_000_000,
      finished_at: 1_700_000_125_000,
      items_found: 1234,
    });
    const user = userEvent.setup();
    render(<App />);
    await user.click(screen.getByRole("button", { name: "History" }));
    expect(await screen.findByText("USB stick image")).toBeInTheDocument();
    expect(screen.getByText("8.0 GiB")).toBeInTheDocument();
    expect(screen.getByText("completed")).toBeInTheDocument();
    expect(screen.getByText("2m 5s")).toBeInTheDocument();
  });

  it("surfaces backend errors with a retry", async () => {
    vi.mocked(api.listScans).mockRejectedValueOnce({ kind: "database", message: "database error: disk I/O" });
    const user = userEvent.setup();
    render(<App />);
    await user.click(screen.getByRole("button", { name: "History" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("database error: disk I/O");
    await user.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("No scans yet")).toBeInTheDocument();
  });
});
