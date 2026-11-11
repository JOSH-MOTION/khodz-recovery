import { describe, expect, it } from "vitest";
import { hexRows, isAllZero, looksLikeText, previewKind } from "../lib/preview";

describe("preview helpers", () => {
  it("never renders SVG or HTML: they are text", () => {
    expect(previewKind("svg", 100)).toBe("text");
    expect(previewKind("html", 100)).toBe("text");
    expect(previewKind("png", 100)).toBe("image");
    expect(previewKind("png", 64 * 1024 * 1024)).toBe("binary", );
  });

  it("sniffs text for unknown types", () => {
    const text = new TextEncoder().encode("plain text\nwith lines\tand tabs");
    expect(looksLikeText(text)).toBe(true);
    expect(previewKind(null, 30, text)).toBe("text");
    expect(looksLikeText(new Uint8Array([65, 0, 66]))).toBe(false);
    expect(previewKind("dat", 3, new Uint8Array([1, 2, 3]))).toBe("binary");
    expect(looksLikeText(new Uint8Array())).toBe(false);
  });

  it("detects zeroed data", () => {
    expect(isAllZero(new Uint8Array(100))).toBe(true);
    expect(isAllZero(new Uint8Array([0, 0, 1]))).toBe(false);
    expect(isAllZero(new Uint8Array())).toBe(false);
  });

  it("formats hex dumps", () => {
    const rows = hexRows(new Uint8Array([0x48, 0x69, 0x00, 0xff, ...Array(16).fill(0x41)]), 0x100);
    expect(rows).toHaveLength(2);
    expect(rows[0]).toEqual({
      offset: "00000100",
      hex: "48 69 00 ff 41 41 41 41 41 41 41 41 41 41 41 41",
      ascii: "Hi..AAAAAAAAAAAA",
    });
    expect(rows[1]!.offset).toBe("00000110");
  });
});
