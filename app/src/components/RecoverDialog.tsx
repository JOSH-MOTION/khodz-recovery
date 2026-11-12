import { useEffect, useState } from "react";
import { api, errorMessage } from "../lib/api";
import { formatBytes } from "../lib/format";
import { useRecovery } from "../lib/recovery";
import type { DestinationReport, SelectionPart } from "../lib/types";
import { Button, ErrorBanner } from "./ui";
import { Icon } from "./Icon";

/**
 * Choose where recovered files go. The destination is checked by the backend (exists,
 * writable, enough space, not the same physical disk as a live source) before Start is enabled.
 */
export function RecoverDialog({
  scanId,
  selection,
  sourceLabel,
  onClose,
  onStarted,
}: {
  scanId: number;
  selection: SelectionPart[];
  sourceLabel: string;
  onClose: () => void;
  onStarted: () => void;
}) {
  const { start, active } = useRecovery();
  const [destination, setDestination] = useState<string | null>(null);
  const [report, setReport] = useState<DestinationReport | null>(null);
  const [checking, setChecking] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [override, setOverride] = useState(false);

  useEffect(() => {
    let live = true;
    setChecking(true);
    setError(null);
    api
      .checkRecovery(scanId, selection, destination)
      .then((r) => live && setReport(r))
      .catch((e) => live && setError(errorMessage(e)))
      .finally(() => live && setChecking(false));
    return () => {
      live = false;
    };
  }, [scanId, selection, destination]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const s = report?.summary;
  const c = report?.check ?? null;
  const writable = s ? s.files - s.erased - s.overwritten : 0;
  const canStart =
    !checking && !active && destination !== null && c !== null && c.problems.length === 0 && (!c.same_disk || override) && writable > 0;

  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/50 p-6" onMouseDown={onClose}>
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Recover files"
        className="k-enter w-full max-w-xl rounded-xl border border-line bg-surface shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="border-b border-line px-5 py-4">
          <h2 className="text-[15px] font-semibold">Recover files from {sourceLabel}</h2>
          <p className="mt-1 text-[12.5px] text-muted">
            Files are copied into a new folder at the destination and each one is verified after writing. The
            source is never modified and existing files are never overwritten.
          </p>
        </div>

        <div className="flex flex-col gap-4 p-5 text-[13px]">
          {error && <ErrorBanner message={error} />}
          {s && (
            <div className="grid grid-cols-3 gap-3">
              <Stat label="Files" value={s.files.toLocaleString()} />
              <Stat label="To write" value={formatBytes(s.bytes)} />
              <Stat label="Known incomplete" value={s.partial.toLocaleString()} />
            </div>
          )}
          {s && s.erased > 0 && (
            <p className="rounded-lg border border-line px-3 py-2 text-[12.5px] text-muted">
              {s.erased.toLocaleString()} selected {s.erased === 1 ? "file was" : "files were"} erased (data reads as
              zeros) and will be listed as not recovered rather than written.
            </p>
          )}
          {s && s.overwritten > 0 && (
            <p className="rounded-lg border border-line px-3 py-2 text-[12.5px] text-muted">
              {s.overwritten.toLocaleString()} selected {s.overwritten === 1 ? "file has" : "files have"} been
              completely overwritten by newer files. Nothing of {s.overwritten === 1 ? "it" : "them"} is left, so{" "}
              {s.overwritten === 1 ? "it" : "they"} will be listed as not recovered rather than written as broken copies.
            </p>
          )}

          <div>
            <div className="mb-1.5 text-[12px] font-medium tracking-wide text-muted uppercase">Destination</div>
            <div className="flex items-center gap-2">
              <div
                className="selectable min-w-0 flex-1 truncate rounded-lg border border-line bg-surface-2 px-3 py-2 font-mono text-[12px]"
                title={destination ?? undefined}
              >
                {destination ?? "No folder chosen"}
              </div>
              <Button
                icon="plus"
                onClick={() =>
                  void api.pickFolder().then((p) => {
                    if (p) {
                      setOverride(false);
                      setDestination(p);
                    }
                  })
                }
              >
                Choose folder…
              </Button>
            </div>
          </div>

          {c && (
            <div className="flex flex-col gap-2" aria-live="polite">
              {c.free_bytes !== null && (
                <p className="text-[12.5px] text-muted">
                  {formatBytes(c.free_bytes)} free · {formatBytes(c.required_bytes)} needed
                  {c.dest_disks.length > 0 ? ` · on ${c.dest_disks.join(", ")}` : ""}
                </p>
              )}
              {c.problems.map((p) => (
                <div key={p} role="alert" className="flex gap-2 rounded-lg border border-danger/40 bg-danger/10 px-3 py-2 text-[12.5px] text-danger">
                  <Icon name="alert" size={15} className="mt-0.5 shrink-0" />
                  {p}
                </div>
              ))}
              {c.same_disk && (
                <div className="rounded-lg border border-warn/40 bg-warn/10 px-3 py-2.5 text-[12.5px] text-warn">
                  <div className="flex gap-2 font-medium">
                    <Icon name="alert" size={15} className="mt-0.5 shrink-0" />
                    This folder is on the same physical disk as the drive you are recovering from.
                  </div>
                  <p className="mt-1 pl-6">
                    Writing here can overwrite the very files you are trying to recover. Choose a folder on another
                    drive (for example an external USB disk).
                  </p>
                  <label className="mt-2 flex items-center gap-2 pl-6">
                    <input type="checkbox" checked={override} onChange={(e) => setOverride(e.target.checked)} />
                    I understand the risk and want to recover to this disk anyway
                  </label>
                </div>
              )}
              {c.warnings.map((w) => (
                <p key={w} className="text-[12.5px] text-warn">
                  {w}
                </p>
              ))}
              {c.problems.length === 0 && !c.same_disk && (
                <p className="flex items-center gap-2 text-[12.5px] text-ok">
                  <Icon name="shield" size={15} /> Destination checks passed.
                </p>
              )}
            </div>
          )}
        </div>

        <div className="flex justify-end gap-2 border-t border-line px-5 py-3">
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            icon="recover"
            disabled={!canStart}
            title={active ? "Another recovery is running" : undefined}
            onClick={() => {
              if (!destination) return;
              start(scanId, selection, destination, override, sourceLabel);
              onStarted();
              onClose();
            }}
          >
            Recover {writable > 0 ? `${writable.toLocaleString()} ${writable === 1 ? "file" : "files"}` : ""}
          </Button>
        </div>
      </div>
    </div>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-line px-3 py-2">
      <div className="text-[11px] tracking-wide text-muted uppercase">{label}</div>
      <div className="mt-0.5 text-[15px] font-semibold tabular-nums">{value}</div>
    </div>
  );
}
