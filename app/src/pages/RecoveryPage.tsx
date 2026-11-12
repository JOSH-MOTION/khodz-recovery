import { useEffect, useState } from "react";
import { api, errorMessage } from "../lib/api";
import { formatBytes, formatDateTime, formatDuration } from "../lib/format";
import { useRecovery } from "../lib/recovery";
import type { Recovery, RecoveryItemRow, RecoveryOutcomeKind } from "../lib/types";
import { Button, Card, EmptyState, ErrorBanner, PageHeader, StatusPill } from "../components/ui";
import { Icon } from "../components/Icon";

const PAGE = 200;

export const OUTCOME: Record<RecoveryOutcomeKind, { text: string; cls: string; hint: string }> = {
  verified: {
    text: "Verified",
    cls: "border-ok/40 bg-ok/10 text-ok",
    hint: "Written, read back and checked: identical to the data recovered from the source.",
  },
  partial_source: {
    text: "Incomplete",
    cls: "border-warn/40 bg-warn/10 text-warn",
    hint: "Written and checked, but the source data was incomplete — the file may be damaged.",
  },
  mismatch: {
    text: "Write mismatch",
    cls: "border-danger/40 bg-danger/10 text-danger",
    hint: "What was read back from the destination differs from what was written. Check the destination drive.",
  },
  failed: { text: "Not recovered", cls: "border-danger/40 bg-danger/10 text-danger", hint: "Nothing was written." },
};

function Progress() {
  const { active, cancel } = useRecovery();
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((n) => n + 1), 1000);
    return () => clearInterval(t);
  }, []);
  if (!active) return null;
  const p = active.progress;
  const pct = p && p.bytes_total > 0 ? Math.min(100, (p.bytes_done / p.bytes_total) * 100) : 0;
  return (
    <Card className="k-enter p-5">
      <div className="flex items-start justify-between gap-4">
        <div className="min-w-0">
          <div className="text-[14px] font-semibold">Recovering from {active.label}</div>
          <div className="selectable mt-1 truncate text-[12.5px] text-muted">
            {p?.current ? `Writing ${p.current}` : "Preparing…"} · into {active.destination}
          </div>
        </div>
        <Button onClick={cancel}>Cancel</Button>
      </div>
      <div
        role="progressbar"
        aria-label="Recovery progress"
        aria-valuenow={Math.round(pct)}
        className="mt-4 h-2 overflow-hidden rounded-full bg-surface-2"
      >
        <div className="h-full rounded-full bg-accent transition-[width] duration-200" style={{ width: `${pct}%` }} />
      </div>
      <div className="mt-2 flex justify-between text-[12.5px] tabular-nums text-muted">
        <span>
          {(p?.files_done ?? 0).toLocaleString()} of {(p?.files_total ?? 0).toLocaleString()} files
        </span>
        <span>
          {p ? `${formatBytes(p.bytes_done)} of ${formatBytes(p.bytes_total)} · ` : ""}
          {formatDuration(active.startedAt, Date.now())}
        </span>
      </div>
    </Card>
  );
}

function Report({ recovery }: { recovery: Recovery }) {
  const [items, setItems] = useState<RecoveryItemRow[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .listRecoveryItems(recovery.id, { limit: PAGE, offset: 0 })
      .then((r) => live && setItems(r))
      .catch((e) => live && setError(errorMessage(e)));
    return () => {
      live = false;
    };
  }, [recovery.id]);

  const counts: [RecoveryOutcomeKind, number][] = [
    ["verified", recovery.files_verified],
    ["partial_source", recovery.files_partial],
    ["mismatch", recovery.files_mismatch],
    ["failed", recovery.files_failed],
  ];

  return (
    <div className="flex flex-col gap-4">
      <Card className="p-5">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="min-w-0">
            <div className="flex items-center gap-2">
              <h2 className="text-[15px] font-semibold">Recovery report</h2>
              <StatusPill status={recovery.status} />
            </div>
            <div className="selectable mt-1 font-mono text-[12px] break-all text-muted">{recovery.destination}</div>
            <div className="mt-1 text-[12.5px] text-muted">
              {formatDateTime(recovery.started_at)} · {formatBytes(recovery.bytes_written)} written ·{" "}
              {formatDuration(recovery.started_at, recovery.finished_at)}
            </div>
          </div>
          <Button icon="results" onClick={() => void api.showInFolder(recovery.destination).catch(() => {})}>
            Show in folder
          </Button>
        </div>
        <div className="mt-4 grid grid-cols-2 gap-3 md:grid-cols-4">
          {counts.map(([k, n]) => (
            <div key={k} title={OUTCOME[k].hint} className="rounded-lg border border-line px-3 py-2">
              <div className="text-[11px] tracking-wide text-muted uppercase">{OUTCOME[k].text}</div>
              <div className="mt-0.5 text-[18px] font-semibold tabular-nums">{n.toLocaleString()}</div>
            </div>
          ))}
        </div>
        {recovery.same_disk_override && (
          <p className="mt-3 flex gap-2 text-[12.5px] text-warn">
            <Icon name="alert" size={15} className="mt-0.5 shrink-0" />
            Files were written to the same physical disk as the source, by explicit choice.
          </p>
        )}
        {recovery.error && (
          <div className="mt-3">
            <ErrorBanner message={recovery.error} />
          </div>
        )}
        <p className="mt-3 text-[12px] text-muted">
          A <span className="font-mono">khodz-manifest.json</span> in the folder lists every file with its SHA-256.
        </p>
      </Card>

      {error && <ErrorBanner message={error} />}
      <Card className="overflow-hidden">
        <table className="w-full table-fixed text-[13px]">
          <thead className="border-b border-line bg-surface-2 text-left text-[11.5px] tracking-wide text-muted uppercase">
            <tr>
              <th className="px-4 py-2.5 font-medium">File</th>
              <th className="w-24 px-4 py-2.5 text-right font-medium">Size</th>
              <th className="w-36 px-4 py-2.5 font-medium">Result</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-line">
            {items.map((it) => (
              <tr key={it.id}>
                <td className="px-4 py-2.5">
                  <div className="selectable truncate font-medium" title={it.output_path || it.source_path || ""}>
                    {it.output_path || it.source_path}
                  </div>
                  {it.error && <div className="text-[12px] text-muted">{it.error}</div>}
                </td>
                <td className="px-4 py-2.5 text-right tabular-nums">{formatBytes(it.size)}</td>
                <td className="px-4 py-2.5">
                  <span title={OUTCOME[it.outcome].hint} className={`rounded-md border px-1.5 py-0.5 text-[11px] font-medium ${OUTCOME[it.outcome].cls}`}>
                    {OUTCOME[it.outcome].text}
                  </span>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {items.length < recovery.files_total && items.length >= PAGE && (
          <div className="flex justify-end border-t border-line px-4 py-2.5">
            <Button
              onClick={() =>
                void api
                  .listRecoveryItems(recovery.id, { limit: PAGE, offset: items.length })
                  .then((r) => setItems((cur) => [...cur, ...r]))
              }
            >
              Load more
            </Button>
          </div>
        )}
      </Card>
    </div>
  );
}

export function RecoveryPage() {
  const { active, lastOutcome, lastError } = useRecovery();
  const [list, setList] = useState<Recovery[]>([]);
  const [id, setId] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .listRecoveries({ limit: 50, offset: 0 })
      .then((l) => {
        const done = l.filter((r) => r.status !== "running");
        setList(done);
        setId((cur) => lastOutcome?.recovery_id ?? cur ?? done[0]?.id ?? null);
      })
      .catch((e) => setError(errorMessage(e)));
  }, [lastOutcome, active]);

  const current = list.find((r) => r.id === id) ?? null;

  return (
    <>
      <PageHeader title="Recovery" subtitle="Recovered files are verified after writing. Choose files on the Results page." />
      <div className="flex flex-col gap-5 px-8 py-6">
        <Progress />
        {lastError && <ErrorBanner message={lastError} />}
        {error && <ErrorBanner message={error} />}
        {list.length === 0 && !active && (
          <EmptyState icon="recover" title="Nothing recovered yet">
            Tick files or folders on the Results page and choose Recover. Files are written to a folder you choose, on a
            different drive from the one you are recovering, and checked after writing.
          </EmptyState>
        )}
        {list.length > 0 && (
          <div className="flex items-center gap-3">
            <label htmlFor="recovery-picker" className="text-[12.5px] text-muted">
              Recovery
            </label>
            <select
              id="recovery-picker"
              value={current?.id ?? ""}
              onChange={(e) => setId(Number(e.target.value))}
              className="h-9 rounded-lg border border-line bg-surface px-3 text-[13px]"
            >
              {list.map((r) => (
                <option key={r.id} value={r.id}>
                  {formatDateTime(r.started_at)} — {r.files_total.toLocaleString()} files
                </option>
              ))}
            </select>
          </div>
        )}
        {current && <Report recovery={current} />}
      </div>
    </>
  );
}
