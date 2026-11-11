import { useEffect, useState } from "react";
import type { ScanSource } from "../lib/types";
import { useScan, type ScanModeChoice } from "../lib/scan";
import { Button } from "./ui";
import { Icon } from "./Icon";

const MODES: { value: ScanModeChoice; title: string; text: string }[] = [
  {
    value: "quick",
    title: "Quick scan",
    text: "Reads filesystem metadata: lists existing and recently deleted files with their original names. Takes seconds to minutes.",
  },
  {
    value: "deep",
    title: "Deep scan",
    text: "Quick scan, then searches every sector for known file types (photos, documents, archives) even where the filesystem is damaged. Reads the whole drive.",
  },
];

/** Choose quick/deep and start. The source is opened read-only either way. */
export function ScanDialog({
  source,
  label,
  onClose,
}: {
  source: ScanSource;
  label: string;
  onClose: () => void;
}) {
  const { start, active } = useScan();
  const [mode, setMode] = useState<ScanModeChoice>("quick");

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/50 p-6" onMouseDown={onClose}>
      <div
        role="dialog"
        aria-modal="true"
        aria-label={`Scan ${label}`}
        className="k-enter w-full max-w-lg rounded-xl border border-line bg-surface shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="border-b border-line px-5 py-4">
          <h2 className="text-[15px] font-semibold">Scan {label}</h2>
          <p className="mt-1 flex items-center gap-1.5 text-[12.5px] text-muted">
            <Icon name="shield" size={14} className="text-ok" /> Read-only: nothing on this source will be changed.
          </p>
        </div>
        <div role="radiogroup" aria-label="Scan type" className="flex flex-col gap-2 p-5">
          {MODES.map((m) => (
            <button
              key={m.value}
              type="button"
              role="radio"
              aria-checked={mode === m.value}
              onClick={() => setMode(m.value)}
              className={`rounded-lg border px-4 py-3 text-left transition-colors outline-none focus-visible:ring-2 focus-visible:ring-accent/60 ${
                mode === m.value ? "border-accent bg-accent-soft" : "border-line hover:bg-surface-2"
              }`}
            >
              <div className="text-[13.5px] font-semibold">{m.title}</div>
              <div className="mt-0.5 text-[12.5px] leading-relaxed text-muted">{m.text}</div>
            </button>
          ))}
        </div>
        <div className="flex justify-end gap-2 border-t border-line px-5 py-3">
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            icon="scan"
            disabled={active !== null}
            title={active ? "Another scan is running" : undefined}
            onClick={() => {
              start(source, label, mode);
              onClose();
            }}
          >
            Start scan
          </Button>
        </div>
      </div>
    </div>
  );
}
