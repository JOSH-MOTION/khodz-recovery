import { useEffect, useRef, useState } from "react";
import { api, errorKind, errorMessage } from "../lib/api";
import { formatBytes } from "../lib/format";
import type { AnalyzedVolume, LayoutReport, ScanSource, SearchOutcome, SearchProgress } from "../lib/types";
import { Button, ErrorBanner } from "./ui";
import { Icon } from "./Icon";
import { FS_LABEL } from "./VolumeLayout";
import { ScanDialog } from "./ScanDialog";

type Search =
  | { status: "idle" }
  | { status: "running"; jobId: string; progress: SearchProgress | null }
  | { status: "done"; outcome: SearchOutcome }
  | { status: "cancelled" }
  | { status: "error"; message: string };

function newJobId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto ? crypto.randomUUID() : `part-${Date.now()}`;
}

/** Bytes the search will read: everything not covered by a verified volume (mirrors the engine). */
export function bytesToSearch(layout: LayoutReport): number {
  const covered = layout.volumes
    .filter((v) => v.verified)
    .map((v) => [v.offset, Math.min(v.offset + v.size, layout.disk_size)] as const)
    .sort((a, b) => a[0] - b[0]);
  let cursor = 0;
  let free = 0;
  for (const [start, end] of covered) {
    if (start > cursor) free += start - cursor;
    cursor = Math.max(cursor, end);
  }
  return free + Math.max(0, layout.disk_size - cursor);
}

export function volumeTitle(v: AnalyzedVolume): string {
  const fs = FS_LABEL[v.filesystem];
  return v.label ? `${v.label} (${fs})` : `${fs} volume`;
}

function accessHint(e: unknown, source: ScanSource): string {
  return errorKind(e) === "access_denied" && source.kind === "device"
    ? `${errorMessage(e)}. Reading a drive's partition structures directly needs Khodz Recovery to run as administrator.`
    : errorMessage(e);
}

/**
 * Check the partition table against the disk contents, and search unclaimed space for lost
 * partitions. Found partitions can be scanned like any other source. Everything is read-only.
 */
export function PartitionCheck({ source, label }: { source: ScanSource; label: string }) {
  const [layout, setLayout] = useState<LayoutReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState<Search>({ status: "idle" });
  const [chosen, setChosen] = useState<Set<number>>(new Set());
  const [scanning, setScanning] = useState(false);
  const live = useRef(true);

  useEffect(() => {
    live.current = true;
    setLayout(null);
    setError(null);
    setSearch({ status: "idle" });
    api
      .inspectPartitions(source)
      .then((l) => live.current && setLayout(l))
      .catch((e) => live.current && setError(accessHint(e, source)));
    return () => {
      live.current = false;
    };
    // The source object is recreated by parents; its path identifies it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [source.kind, source.path]);

  async function runSearch() {
    const jobId = newJobId();
    setSearch({ status: "running", jobId, progress: null });
    try {
      const outcome = await api.searchPartitions(source, jobId, (progress) => {
        if (live.current) setSearch((s) => (s.status === "running" && s.jobId === jobId ? { ...s, progress } : s));
      });
      if (!live.current) return;
      setLayout(outcome.layout);
      setChosen(new Set(outcome.found.map((v, i) => (v.opens ? i : -1)).filter((i) => i >= 0)));
      setSearch({ status: "done", outcome });
    } catch (e) {
      if (live.current) {
        setSearch(errorKind(e) === "cancelled" ? { status: "cancelled" } : { status: "error", message: accessHint(e, source) });
      }
    }
  }

  if (error) return <ErrorBanner message={error} />;
  if (!layout) return <p className="text-[12.5px] text-muted">Checking the partition table against the disk…</p>;

  const toSearch = bytesToSearch(layout);
  const found = search.status === "done" ? search.outcome.found : [];
  const selected = found.filter((_, i) => chosen.has(i));

  return (
    <div className="flex flex-col gap-4">
      {layout.issues.length === 0 ? (
        <p className="flex items-center gap-2 text-[12.5px] text-ok">
          <Icon name="shield" size={15} /> The partition table matches what is on the disk.
        </p>
      ) : (
        <ul aria-label="Partition problems" className="flex flex-col gap-1.5">
          {layout.issues.map((i) => (
            <li key={i} className="flex gap-2 rounded-lg border border-warn/40 bg-warn/10 px-3 py-2 text-[12.5px] text-warn">
              <Icon name="alert" size={15} className="mt-0.5 shrink-0" />
              {i}
            </li>
          ))}
        </ul>
      )}
      {layout.volumes.some((v) => v.boot_backup !== null) && (
        <p className="text-[12.5px] text-muted">
          Volumes with a damaged boot sector are read through their backup copy automatically when you scan.
        </p>
      )}
      {layout.unallocated.length > 0 && (
        <p className="text-[12.5px] text-muted">
          {formatBytes(layout.unallocated.reduce((n, [a, b]) => n + (b - a), 0))} of this disk is not in any partition.
        </p>
      )}

      <div className="rounded-lg border border-line p-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="min-w-0">
            <div className="text-[13px] font-semibold">Lost partitions</div>
            <div className="mt-0.5 text-[12.5px] text-muted">
              {toSearch === 0
                ? "Every byte belongs to a readable volume; there is nowhere for a lost partition to be."
                : `Searches ${formatBytes(toSearch)} not covered by a readable volume for filesystem boot records, including backup copies.`}
            </div>
          </div>
          {search.status !== "running" && toSearch > 0 && (
            <Button icon="scan" onClick={() => void runSearch()}>
              {search.status === "done" ? "Search again" : "Search for lost partitions"}
            </Button>
          )}
        </div>

        {search.status === "running" && <SearchRunning search={search} />}
        {search.status === "cancelled" && <p className="mt-3 text-[12.5px] text-muted">Search cancelled.</p>}
        {search.status === "error" && (
          <div className="mt-3">
            <ErrorBanner message={search.message} />
          </div>
        )}
        {search.status === "done" && found.length === 0 && (
          <p className="mt-3 text-[12.5px] text-muted">
            No lost partitions found in {formatBytes(search.outcome.searched_bytes)}. If files were deleted rather
            than a partition, use a deep scan.
          </p>
        )}
        {found.length > 0 && (
          <div className="mt-3 flex flex-col gap-2">
            <ul aria-label="Found partitions" className="flex flex-col divide-y divide-line rounded-lg border border-line">
              {found.map((v, i) => (
                <li key={v.offset} className="flex gap-3 px-3 py-2.5">
                  <input
                    type="checkbox"
                    aria-label={`Include ${volumeTitle(v)} at ${formatBytes(v.offset)}`}
                    className="mt-1 h-4 w-4 shrink-0 accent-[var(--k-accent)]"
                    checked={chosen.has(i)}
                    disabled={!v.filesystem || v.filesystem === "unknown"}
                    onChange={() =>
                      setChosen((c) => {
                        const n = new Set(c);
                        if (n.has(i)) n.delete(i);
                        else n.add(i);
                        return n;
                      })
                    }
                  />
                  <div className="min-w-0 flex-1 text-[12.5px]">
                    <div className="flex flex-wrap items-baseline justify-between gap-2">
                      <span className="text-[13px] font-medium">{volumeTitle(v)}</span>
                      <span className="tabular-nums text-muted">
                        {formatBytes(v.size)} at byte {v.offset.toLocaleString()}
                      </span>
                    </div>
                    <div className={v.opens ? "text-ok" : "text-muted"}>
                      {v.opens ? "Files can be listed" : "Detected only — files cannot be listed"}
                    </div>
                    {[...v.problems, ...v.notes].map((n) => (
                      <div key={n} className={v.problems.includes(n) ? "text-warn" : "text-muted"}>
                        {n}
                      </div>
                    ))}
                  </div>
                </li>
              ))}
            </ul>
            <div className="flex justify-end">
              <Button variant="primary" icon="scan" disabled={selected.length === 0} onClick={() => setScanning(true)}>
                Scan {selected.length === 1 ? "this partition" : `${selected.length} partitions`}…
              </Button>
            </div>
          </div>
        )}
      </div>

      {scanning && (
        <ScanDialog
          source={source}
          label={`${label} (found partitions)`}
          volumes={selected.map((v) => ({ offset: v.offset, size: v.size, boot_backup: v.boot_backup }))}
          onClose={() => setScanning(false)}
        />
      )}
    </div>
  );
}

function SearchRunning({ search }: { search: Extract<Search, { status: "running" }> }) {
  const p = search.progress;
  const pct = p && p.bytes_total > 0 ? Math.min(100, (p.bytes_done / p.bytes_total) * 100) : 0;
  return (
    <div className="mt-3">
      <div
        role="progressbar"
        aria-label="Partition search progress"
        aria-valuenow={Math.round(pct)}
        className="h-2 overflow-hidden rounded-full bg-surface-2"
      >
        <div className="h-full rounded-full bg-accent transition-[width] duration-150" style={{ width: `${pct}%` }} />
      </div>
      <div className="mt-2 flex items-center justify-between gap-4 text-[12.5px] tabular-nums text-muted">
        <span>
          {p ? `${formatBytes(p.bytes_done)} of ${formatBytes(p.bytes_total)} · ${p.found} found` : "Starting…"}
        </span>
        <Button onClick={() => void api.cancelJob(search.jobId)}>Cancel</Button>
      </div>
    </div>
  );
}
