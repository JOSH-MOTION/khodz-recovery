import type { ScanItemRow } from "./types";

export type Tone = "ok" | "warn" | "danger" | "muted" | "accent";

/** One honest label for what the scan knows about an item. */
export function itemStatus(r: ScanItemRow): { text: string; tone: Tone; hint?: string } {
  if (r.origin === "carved") {
    if (r.carve_status === "complete")
      return { text: "Verified", tone: "ok", hint: r.note ?? "File structure checked end to end" };
    if (r.carve_status === "unverified")
      return { text: "Unverified", tone: "muted", hint: r.note ?? "Contents of this format cannot be verified" };
    return { text: "Partial", tone: "warn", hint: r.note ?? "Only part of the file was found" };
  }
  if (!r.is_deleted) return { text: "Existing", tone: "accent" };
  if (r.data_state === "erased") return { text: "Deleted · erased", tone: "danger", hint: r.note ?? "Data reads back as zeros" };
  if (r.content_check === "verified")
    return {
      text: "Deleted · verified",
      tone: "ok",
      hint: r.note ?? "The data at its location was checked against its format and is intact",
    };
  if (r.overwritten_pct === 100)
    return {
      text: "Deleted · overwritten",
      tone: "danger",
      hint: "Other files have since been saved over all of it; nothing of it is left to recover",
    };
  if (r.content_check === "damaged")
    return {
      text: "Deleted · damaged",
      tone: "warn",
      hint: r.note ?? "The data at its location is not all this file; it will probably not open, or only partly",
    };
  if (r.overwritten_pct !== null && r.overwritten_pct > 0)
    return {
      text: `Deleted · ${r.overwritten_pct}% overwritten`,
      tone: "warn",
      hint: "Part of it now belongs to other files; a recovered copy will be incomplete",
    };
  if (r.extent_quality === "partial") return { text: "Deleted · partial", tone: "warn", hint: "Location only partly known" };
  return {
    text: "Deleted",
    tone: "warn",
    hint: r.extent_quality === "contiguity_assumed" ? "Contiguous storage assumed" : undefined,
  };
}

export const TONES: Record<Tone, string> = {
  ok: "border-ok/40 bg-ok/10 text-ok",
  warn: "border-warn/40 bg-warn/10 text-warn",
  danger: "border-danger/40 bg-danger/10 text-danger",
  muted: "border-line text-muted",
  accent: "border-accent/40 bg-accent-soft text-accent",
};
