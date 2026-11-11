import { useEffect, useMemo, useState } from "react";
import { api, errorMessage } from "../lib/api";
import { formatBytes, formatDateTime, formatDuration } from "../lib/format";
import { TONES, itemStatus } from "../lib/itemStatus";
import { useScan } from "../lib/scan";
import type { ItemStats, ItemView, Scan, ScanItemRow, ScanPhase } from "../lib/types";
import { Button, Card, EmptyState, ErrorBanner, PageHeader, StatusPill } from "../components/ui";
import { Explorer } from "../components/Explorer";
import { PreviewPane } from "../components/PreviewPane";

export { itemStatus } from "../lib/itemStatus";

const PAGE = 200;

const PHASE_LABEL: Record<ScanPhase, string> = {
  partitions: "Reading partition table",
  metadata: "Reading filesystem metadata",
  carving: "Searching every sector for known file types",
};

const VIEWS: { value: ItemView; label: string }[] = [
  { value: "all_files", label: "All files" },
  { value: "deleted", label: "Deleted" },
  { value: "carved", label: "Found by signature" },
];

function ProgressPanel() {
  const { active, cancel } = useScan();
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(t);
  }, []);
  if (!active) return null;
  const p = active.progress;
  const pct = p && p.bytes_total > 0 ? Math.min(100, (p.bytes_done / p.bytes_total) * 100) : 0;
  const indeterminate = !p || p.phase !== "carving";
  return (
    <Card className="k-enter p-5">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <div className="text-[14px] font-semibold">
            {active.mode === "deep" ? "Deep" : "Quick"} scan of {active.label}
          </div>
          <div className="mt-1 text-[12.5px] text-muted">
            {p ? PHASE_LABEL[p.phase] : "Opening source…"}
            {p?.detail ? ` · ${p.detail}` : ""}
          </div>
        </div>
        <Button onClick={cancel}>Cancel</Button>
      </div>
      <div
        role="progressbar"
        aria-label="Scan progress"
        aria-valuenow={indeterminate ? undefined : Math.round(pct)}
        className="mt-4 h-2 overflow-hidden rounded-full bg-surface-2"
      >
        <div
          className={`h-full rounded-full bg-accent transition-[width] duration-200 ${indeterminate ? "w-1/3 animate-pulse" : ""}`}
          style={indeterminate ? undefined : { width: `${pct}%` }}
        />
      </div>
      <div className="mt-2 flex justify-between text-[12.5px] tabular-nums text-muted">
        <span>{(p?.items_found ?? 0).toLocaleString()} items found</span>
        <span>
          {p && !indeterminate ? `${formatBytes(p.bytes_done)} of ${formatBytes(p.bytes_total)} · ` : ""}
          {formatDuration(active.startedAt, Date.now())}
        </span>
      </div>
    </Card>
  );
}

/** Flat list used while searching (results can come from any folder or volume). */
function SearchResults({
  scanId,
  view,
  search,
  selected,
  onSelect,
}: {
  scanId: number;
  view: ItemView;
  search: string;
  selected: ScanItemRow | null;
  onSelect: (r: ScanItemRow) => void;
}) {
  const [items, setItems] = useState<ScanItemRow[]>([]);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const filter = useMemo(() => ({ view, search }), [view, search]);

  useEffect(() => {
    let live = true;
    api
      .listScanItems(scanId, filter, { limit: PAGE, offset: 0 })
      .then((p) => {
        if (!live) return;
        setItems(p.items);
        setTotal(p.total);
      })
      .catch((e) => live && setError(errorMessage(e)));
    return () => {
      live = false;
    };
  }, [scanId, filter]);

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden rounded-xl border border-line bg-surface">
      {error && <ErrorBanner message={error} />}
      <div role="listbox" aria-label="Search results" className="min-h-0 flex-1 overflow-y-auto">
        {items.map((r) => {
          const st = itemStatus(r);
          return (
            <div
              key={r.id}
              role="option"
              aria-selected={selected?.id === r.id}
              onClick={() => onSelect(r)}
              className={`flex cursor-default items-center gap-3 border-b border-line px-4 py-2 text-[13px] ${
                selected?.id === r.id ? "bg-accent-soft" : "hover:bg-surface-2/60"
              }`}
            >
              <span className="min-w-0 flex-1">
                <span className="block truncate font-medium">{r.name}</span>
                <span className="block truncate text-[12px] text-muted">
                  {r.origin === "carved" ? "Found by signature" : r.filesystem} · {r.path}
                </span>
              </span>
              <span className="w-20 shrink-0 text-right text-[12px] tabular-nums text-muted">{formatBytes(r.size)}</span>
              <span className={`w-32 shrink-0 truncate rounded-md border px-1.5 py-0.5 text-center text-[11px] font-medium ${TONES[st.tone]}`}>
                {st.text}
              </span>
            </div>
          );
        })}
        {items.length === 0 && !error && <p className="p-6 text-center text-[13px] text-muted">No files match.</p>}
      </div>
      <div className="flex items-center justify-between border-t border-line px-4 py-2 text-[12.5px] text-muted">
        <span>
          Showing {items.length.toLocaleString()} of {total.toLocaleString()}
        </span>
        {items.length < total && (
          <Button
            onClick={() =>
              void api
                .listScanItems(scanId, filter, { limit: PAGE, offset: items.length })
                .then((p) => setItems((cur) => [...cur, ...p.items]))
            }
          >
            Load more
          </Button>
        )}
      </div>
    </div>
  );
}

export function ResultsPage() {
  const { active, lastOutcome, lastError } = useScan();
  const [scans, setScans] = useState<Scan[]>([]);
  const [scanId, setScanId] = useState<number | null>(null);
  const [view, setView] = useState<ItemView>("all_files");
  const [search, setSearch] = useState("");
  const [debounced, setDebounced] = useState("");
  const [stats, setStats] = useState<ItemStats | null>(null);
  const [selected, setSelected] = useState<ScanItemRow | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .listScans({ limit: 50, offset: 0 })
      .then((list) => {
        const usable = list.filter((s) => s.status !== "running");
        setScans(usable);
        setScanId((cur) => lastOutcome?.scan_id ?? cur ?? usable[0]?.id ?? null);
      })
      .catch((e) => setError(errorMessage(e)));
  }, [lastOutcome, active]);

  useEffect(() => {
    const t = setTimeout(() => setDebounced(search.trim()), 250);
    return () => clearTimeout(t);
  }, [search]);

  useEffect(() => {
    if (scanId === null) return;
    setSelected(null);
    api
      .scanItemStats(scanId)
      .then(setStats)
      .catch((e) => setError(errorMessage(e)));
  }, [scanId]);

  const scan = scans.find((s) => s.id === scanId) ?? null;

  return (
    <div className="flex h-full flex-col">
      <PageHeader title="Results" subtitle="Browse and preview what your scans found. Nothing is written anywhere until you recover." />
      <div className="flex min-h-0 flex-1 flex-col gap-4 px-8 py-5">
        <ProgressPanel />
        {lastError && <ErrorBanner message={lastError} />}
        {error && <ErrorBanner message={error} />}

        {scans.length === 0 && !active && (
          <EmptyState icon="results" title="No scan results yet">
            Start a scan from Drives or Disk Images. Quick scans list existing and recently deleted files; deep scans
            also search every sector for photos, documents and archives.
          </EmptyState>
        )}

        {scan && (
          <>
            <div className="flex flex-wrap items-center gap-3">
              <label className="text-[12.5px] text-muted" htmlFor="scan-picker">
                Scan
              </label>
              <select
                id="scan-picker"
                value={scan.id}
                onChange={(e) => setScanId(Number(e.target.value))}
                className="h-9 rounded-lg border border-line bg-surface px-3 text-[13px] outline-none focus-visible:ring-2 focus-visible:ring-accent/60"
              >
                {scans.map((s) => (
                  <option key={s.id} value={s.id}>
                    {(s.source_label ?? s.source_path) + ` — ${s.scan_mode}, ${formatDateTime(s.started_at)}`}
                  </option>
                ))}
              </select>
              <StatusPill status={scan.status} />
              {scan.error && <span className="text-[12.5px] text-danger">{scan.error}</span>}
              {stats && (
                <span className="ml-auto text-[12.5px] text-muted tabular-nums">
                  {stats.files.toLocaleString()} files · {stats.deleted.toLocaleString()} deleted ({stats.erased.toLocaleString()} erased) ·{" "}
                  {stats.carved.toLocaleString()} by signature ({stats.carved_complete.toLocaleString()} verified)
                </span>
              )}
            </div>

            <div className="flex flex-wrap items-center justify-between gap-3">
              <div role="tablist" aria-label="Result type" className="inline-flex rounded-lg border border-line bg-surface p-0.5">
                {VIEWS.map((v) => (
                  <button
                    key={v.value}
                    role="tab"
                    type="button"
                    aria-selected={view === v.value}
                    onClick={() => {
                      setView(v.value);
                      setSelected(null);
                    }}
                    className={`rounded-md px-3.5 py-1.5 text-[13px] font-medium transition-colors ${
                      view === v.value ? "bg-surface-2 text-fg" : "text-muted hover:text-fg"
                    }`}
                  >
                    {v.label}
                  </button>
                ))}
              </div>
              <input
                type="search"
                aria-label="Search files"
                placeholder="Search all folders by name or path"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                className="selectable h-9 w-80 rounded-lg border border-line bg-surface px-3 text-[13px] outline-none placeholder:text-muted focus-visible:ring-2 focus-visible:ring-accent/60"
              />
            </div>

            <div className="flex min-h-[460px] flex-1 gap-4">
              {debounced ? (
                <SearchResults scanId={scan.id} view={view} search={debounced} selected={selected} onSelect={setSelected} />
              ) : (
                <Explorer scanId={scan.id} view={view} carvedCount={stats?.carved ?? 0} selected={selected} onSelect={setSelected} />
              )}
              <aside aria-label="Preview" className="w-[360px] shrink-0 overflow-hidden rounded-xl border border-line bg-surface xl:w-[420px]">
                <PreviewPane item={selected} />
              </aside>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
