// Pure helpers for previewing untrusted recovered data. Nothing here executes content: images
// go to the browser's image decoder via blob URLs, everything else is shown as text or hex.

export const IMAGE_MIME: Record<string, string> = {
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  png: "image/png",
  gif: "image/gif",
  bmp: "image/bmp",
  webp: "image/webp",
};

const TEXT_TYPES = new Set([
  "txt", "csv", "tsv", "log", "md", "json", "xml", "html", "htm", "css", "js", "ts", "ini", "cfg", "conf",
  "yaml", "yml", "rtf", "sql", "py", "rs", "c", "h", "cpp", "java", "bat", "cmd", "ps1", "sh", "svg",
]);

export const IMAGE_PREVIEW_MAX = 32 * 1024 * 1024;
export const TEXT_PREVIEW_MAX = 256 * 1024;
export const HEX_PREVIEW_MAX = 16 * 1024;

export type PreviewKind = "image" | "text" | "binary";

/** Decide how to preview from the type and (for unknown types) a sniff of the first bytes. */
export function previewKind(fileType: string | null, size: number, head?: Uint8Array): PreviewKind {
  const t = (fileType ?? "").toLowerCase();
  // SVG is XML that can carry scripts: always shown as text, never rendered.
  if (t in IMAGE_MIME && size <= IMAGE_PREVIEW_MAX) return "image";
  if (TEXT_TYPES.has(t)) return "text";
  if (head && looksLikeText(head)) return "text";
  return "binary";
}

export function looksLikeText(bytes: Uint8Array): boolean {
  const n = Math.min(bytes.length, 4096);
  if (n === 0) return false;
  let printable = 0;
  for (let i = 0; i < n; i++) {
    const b = bytes[i]!;
    if (b === 0) return false;
    if (b === 9 || b === 10 || b === 13 || (b >= 32 && b !== 127)) printable++;
  }
  return printable / n > 0.95;
}

export function isAllZero(bytes: Uint8Array): boolean {
  for (let i = 0; i < bytes.length; i++) if (bytes[i] !== 0) return false;
  return bytes.length > 0;
}

/** Classic hex dump rows: offset, 16 hex bytes, printable ASCII. */
export function hexRows(bytes: Uint8Array, base = 0): { offset: string; hex: string; ascii: string }[] {
  const rows = [];
  for (let i = 0; i < bytes.length; i += 16) {
    const chunk = bytes.subarray(i, i + 16);
    const hex = Array.from(chunk, (b) => b.toString(16).padStart(2, "0")).join(" ");
    const ascii = Array.from(chunk, (b) => (b >= 32 && b < 127 ? String.fromCharCode(b) : ".")).join("");
    rows.push({ offset: (base + i).toString(16).padStart(8, "0"), hex, ascii });
  }
  return rows;
}

export const QUALITY_TEXT: Record<string, string> = {
  exact: "Exact — taken from intact filesystem records or a fully verified file structure.",
  contiguity_assumed:
    "Assumed contiguous — only the start of the file is known; if it was fragmented, the end may contain other data.",
  partial: "Partial — only part of the file's location is known.",
  unknown: "Unknown.",
};
