import { describe, expect, it } from "vitest";
import { formatBytes, formatDuration } from "../lib/format";
import { resolveTheme, isThemePreference } from "../lib/theme";

describe("formatBytes (must match khodz_core::ByteSize)", () => {
  it("formats like the Rust implementation", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1023)).toBe("1023 B");
    expect(formatBytes(1024)).toBe("1.0 KiB");
    expect(formatBytes(1536)).toBe("1.5 KiB");
    expect(formatBytes(500 * 1024 ** 3)).toBe("500.0 GiB");
  });
  it("rejects invalid input", () => {
    expect(formatBytes(-1)).toBe("—");
    expect(formatBytes(Number.NaN)).toBe("—");
  });
});

describe("formatDuration", () => {
  it("handles running and finished jobs", () => {
    expect(formatDuration(0, null)).toBe("—");
    expect(formatDuration(0, 42_000)).toBe("42s");
    expect(formatDuration(0, 125_000)).toBe("2m 5s");
    expect(formatDuration(0, 3_720_000)).toBe("1h 2m");
    expect(formatDuration(10, 5)).toBe("—");
  });
});

describe("theme", () => {
  it("resolves system preference", () => {
    expect(resolveTheme("system", true)).toBe("dark");
    expect(resolveTheme("system", false)).toBe("light");
    expect(resolveTheme("light", true)).toBe("light");
  });
  it("validates stored values", () => {
    expect(isThemePreference("dark")).toBe(true);
    expect(isThemePreference("purple")).toBe(false);
    expect(isThemePreference(null)).toBe(false);
  });
});
