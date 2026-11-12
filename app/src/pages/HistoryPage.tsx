import { useState } from "react";
import { api } from "../lib/api";
import { formatBytes, formatDateTime, formatDuration } from "../lib/format";
import type { Recovery, Scan } from "../lib/types";
import { useAsync } from "../lib/useAsync";
import { Button, Card, EmptyState, ErrorBanner, PageHeader, StatusPill } from "../components/ui";

type Tab = "scans" | "recoveries";

export function HistoryPage() {
  const [tab, setTab] = useState<Tab>("scans");
  const { state, reload } = useAsync(() => Promise.all([api.listScans(), api.listRecoveries()]));

  return (
    <>
      <PageHeader
        title="History"
        subtitle="Every scan and recovery run on this computer."
        actions={
          <Button icon="refresh" onClick={reload}>
            Refresh
          </Button>
        }
      />
      <div className="px-8 pt-5">
        <div role="tablist" aria-label="History type" className="inline-flex rounded-lg border border-line bg-surface p-0.5">
          {(["scans", "recoveries"] as const).map((t) => (
            <button
              key={t}
              role="tab"
              type="button"
              aria-selected={tab === t}
              onClick={() => setTab(t)}
              className={`rounded-md px-3.5 py-1.5 text-[13px] font-medium capitalize transition-colors ${
                tab === t ? "bg-surface-2 text-fg" : "text-muted hover:text-fg"
              }`}
            >
              {t}
            </button>
          ))}
        </div>
      </div>
      <div className="px-8 py-5">
        {state.status === "loading" && <p className="text-[13px] text-muted">Loading…</p>}
        {state.status === "error" && <ErrorBanner message={state.error} onRetry={reload} />}
        {state.status === "ready" &&
          (tab === "scans" ? <ScanTable scans={state.data[0]} /> : <RecoveryTable recoveries={state.data[1]} />)}
      </div>
    </>
  );
}

const th = "px-4 py-2.5 text-left text-[11.5px] font-medium tracking-wide text-muted uppercase";
const td = "px-4 py-3 text-[13px]";

function ScanTable({ scans }: { scans: Scan[] }) {
  if (scans.length === 0) {
    return (
      <EmptyState icon="history" title="No scans yet">
        Scans you run will be listed here with their outcome, so you can return to results later.
      </EmptyState>
    );
  }
  return (
    <Card className="k-enter overflow-hidden">
      <table className="w-full">
        <thead className="border-b border-line bg-surface-2">
          <tr>
            <th className={th}>Source</th>
            <th className={th}>Mode</th>
            <th className={th}>Status</th>
            <th className={th}>Found</th>
            <th className={th}>Started</th>
            <th className={th}>Duration</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-line">
          {scans.map((s) => (
            <tr key={s.id} className="hover:bg-surface-2/60">
              <td className={td}>
                <div className="font-medium">{s.source_label ?? s.source_path}</div>
                <div className="text-[12px] text-muted">{formatBytes(s.source_size)}</div>
              </td>
              <td className={`${td} capitalize`}>{s.scan_mode}</td>
              <td className={td}>
                <StatusPill status={s.status} />
              </td>
              <td className={`${td} tabular-nums`}>{s.items_found.toLocaleString()}</td>
              <td className={`${td} text-muted`}>{formatDateTime(s.started_at)}</td>
              <td className={`${td} text-muted tabular-nums`}>{formatDuration(s.started_at, s.finished_at)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </Card>
  );
}

function RecoveryTable({ recoveries }: { recoveries: Recovery[] }) {
  if (recoveries.length === 0) {
    return (
      <EmptyState icon="recover" title="No recoveries yet">
        Each recovery is recorded with its destination and how many files passed integrity verification.
      </EmptyState>
    );
  }
  return (
    <Card className="k-enter overflow-hidden">
      <table className="w-full">
        <thead className="border-b border-line bg-surface-2">
          <tr>
            <th className={th}>Destination</th>
            <th className={th}>Status</th>
            <th className={th}>Verified</th>
            <th className={th}>Incomplete</th>
            <th className={th}>Failed</th>
            <th className={th}>Written</th>
            <th className={th}>Started</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-line">
          {recoveries.map((r) => (
            <tr key={r.id} className="hover:bg-surface-2/60">
              <td className={`${td} selectable font-medium`}>{r.destination}</td>
              <td className={td}>
                <StatusPill status={r.status} />
              </td>
              <td className={`${td} tabular-nums`}>
                {r.files_verified} / {r.files_total}
              </td>
              <td className={`${td} tabular-nums ${r.files_partial > 0 ? "text-warn" : ""}`}>{r.files_partial}</td>
              <td
                className={`${td} tabular-nums ${r.files_failed + r.files_mismatch > 0 ? "text-danger" : ""}`}
                title={r.files_mismatch > 0 ? `${r.files_mismatch} failed verification after writing` : undefined}
              >
                {r.files_failed + r.files_mismatch}
              </td>
              <td className={`${td} tabular-nums`}>{formatBytes(r.bytes_written)}</td>
              <td className={`${td} text-muted`}>{formatDateTime(r.started_at)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </Card>
  );
}
