import { useRef, useState } from "react";
import { api, errorKind, errorMessage, IMAGE_EXTENSIONS } from "../lib/api";
import { formatBytes, formatDateTime } from "../lib/format";
import type { FingerprintResult, ImageAnalysis, ImageInfo, JobProgress } from "../lib/types";
import { Button, Card, EmptyState, ErrorBanner, PageHeader } from "../components/ui";
import { Icon } from "../components/Icon";
import { VolumeLayout } from "../components/VolumeLayout";
import { ScanDialog } from "../components/ScanDialog";

type Analysis = { status: "loading" } | { status: "ready"; data: ImageAnalysis } | { status: "error"; message: string };

type Job =
  | { status: "idle" }
  | { status: "running"; id: string; progress: JobProgress; startedAt: number }
  | { status: "done"; result: FingerprintResult }
  | { status: "cancelled" }
  | { status: "error"; message: string };

function newJobId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `job-${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

export function DiskImagesPage() {
  const [image, setImage] = useState<ImageInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [opening, setOpening] = useState(false);
  const [job, setJob] = useState<Job>({ status: "idle" });
  const [analysis, setAnalysis] = useState<Analysis>({ status: "loading" });
  const running = job.status === "running";

  async function openImage() {
    setError(null);
    setOpening(true);
    try {
      const path = await api.pickImageFile();
      if (!path) return;
      const info = await api.inspectImage(path);
      setImage(info);
      setJob({ status: "idle" });
      setAnalysis({ status: "loading" });
      api
        .analyzeImage(path)
        .then((data) => setAnalysis({ status: "ready", data }))
        .catch((e) => setAnalysis({ status: "error", message: errorMessage(e) }));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setOpening(false);
    }
  }

  return (
    <>
      <PageHeader
        title="Disk Images"
        subtitle="Scan a raw disk image instead of a live drive — the safest way to work on a failing disk."
        actions={
          <Button variant="primary" icon="plus" onClick={openImage} disabled={opening || running}>
            Open image
          </Button>
        }
      />
      <div className="flex max-w-4xl flex-col gap-5 px-8 py-6">
        {error && <ErrorBanner message={error} />}
        {!image && !error && (
          <EmptyState icon="image" title="No image open">
            Open a sector-by-sector image ({IMAGE_EXTENSIONS.map((e) => `.${e}`).join(", ")}). Images are opened
            read-only. Compressed and container formats (E01, compressed DMG, VHDX) are not supported yet.
          </EmptyState>
        )}
        {image && <ImageCard image={image} job={job} setJob={setJob} analysis={analysis} />}
      </div>
    </>
  );
}

function ImageCard({
  image,
  job,
  setJob,
  analysis,
}: {
  image: ImageInfo;
  job: Job;
  setJob: (j: Job) => void;
  analysis: Analysis;
}) {
  const latest = useRef<Job>(job);
  latest.current = job;
  const [scanning, setScanning] = useState(false);

  async function fingerprint() {
    const id = newJobId();
    const startedAt = Date.now();
    setJob({ status: "running", id, startedAt, progress: { bytes_done: 0, bytes_total: image.size_bytes } });
    try {
      const result = await api.fingerprintImage(image.path, id, (progress) => {
        if (latest.current.status === "running") setJob({ status: "running", id, startedAt, progress });
      });
      setJob({ status: "done", result });
    } catch (e) {
      setJob(errorKind(e) === "cancelled" ? { status: "cancelled" } : { status: "error", message: errorMessage(e) });
    }
  }

  return (
    <Card className="k-enter">
      <div className="flex items-start gap-4 p-5">
        <div className="grid h-12 w-12 shrink-0 place-items-center rounded-xl border border-line bg-surface-2">
          <Icon name="image" size={24} className="text-accent" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline justify-between gap-3">
            <h2 className="truncate text-[15px] font-semibold">{image.file_name}</h2>
            <span className="shrink-0 text-[15px] font-semibold tabular-nums">{formatBytes(image.size_bytes)}</span>
          </div>
          <div className="selectable mt-1 truncate font-mono text-[11.5px] text-muted" title={image.path}>
            {image.path}
          </div>
          <div className="mt-2 flex flex-wrap gap-1.5">
            <span className="flex items-center gap-1 rounded-md border border-ok/40 bg-ok/10 px-1.5 py-0.5 text-[11px] font-medium text-ok">
              <Icon name="shield" size={12} /> Opened read-only
            </span>
            {image.file_read_only && (
              <span className="rounded-md border border-line px-1.5 py-0.5 text-[11px] font-medium text-muted">
                File marked read-only
              </span>
            )}
          </div>
        </div>
      </div>

      <dl className="grid grid-cols-3 gap-px border-y border-line bg-line text-[12.5px]">
        {[
          ["Size", `${image.size_bytes.toLocaleString()} bytes`],
          ["Sector size", `${image.sector_size} B (assumed)`],
          ["Modified", formatDateTime(image.modified_ms)],
        ].map(([k, v]) => (
          <div key={k} className="bg-surface px-5 py-3">
            <dt className="text-muted">{k}</dt>
            <dd className="mt-0.5 font-medium tabular-nums">{v}</dd>
          </div>
        ))}
      </dl>

      <div className="border-b border-line p-5">
        <h3 className="mb-3 text-[13.5px] font-semibold">Partitions &amp; filesystems</h3>
        {analysis.status === "loading" && <p className="text-[12.5px] text-muted">Reading partition table…</p>}
        {analysis.status === "error" && <ErrorBanner message={analysis.message} />}
        {analysis.status === "ready" && <VolumeLayout analysis={analysis.data} diskSize={image.size_bytes} />}
      </div>

      <div className="p-5">
        <h3 className="text-[13.5px] font-semibold">Fingerprint</h3>
        <p className="mt-1 text-[12.5px] text-muted">
          SHA-256 of every byte. Record it now to prove later that the image was never modified.
        </p>
        <div className="mt-4">
          <FingerprintPanel job={job} onStart={fingerprint} />
        </div>
      </div>

      <div className="flex justify-end border-t border-line px-5 py-3">
        <Button variant="primary" icon="scan" onClick={() => setScanning(true)}>
          Scan image
        </Button>
        {scanning && (
          <ScanDialog
            source={{ kind: "image", path: image.path }}
            label={image.file_name}
            onClose={() => setScanning(false)}
          />
        )}
      </div>
    </Card>
  );
}

function FingerprintPanel({ job, onStart }: { job: Job; onStart: () => void }) {
  const [copied, setCopied] = useState(false);

  if (job.status === "running") {
    const { bytes_done, bytes_total } = job.progress;
    const pct = bytes_total > 0 ? (bytes_done / bytes_total) * 100 : 0;
    const seconds = Math.max((Date.now() - job.startedAt) / 1000, 0.001);
    const rate = bytes_done / seconds;
    const eta = rate > 0 ? Math.max(0, (bytes_total - bytes_done) / rate) : null;
    return (
      <div>
        <div
          role="progressbar"
          aria-label="Fingerprint progress"
          aria-valuenow={Math.round(pct)}
          aria-valuemin={0}
          aria-valuemax={100}
          className="h-2 overflow-hidden rounded-full bg-surface-2"
        >
          <div className="h-full rounded-full bg-accent transition-[width] duration-150" style={{ width: `${pct}%` }} />
        </div>
        <div className="mt-2 flex items-center justify-between gap-4 text-[12.5px]">
          <span className="tabular-nums text-muted">
            {formatBytes(bytes_done)} of {formatBytes(bytes_total)} · {formatBytes(rate)}/s
            {eta !== null && bytes_done > 0 ? ` · ${Math.ceil(eta)}s left` : ""}
          </span>
          <Button onClick={() => void api.cancelJob(job.id)}>Cancel</Button>
        </div>
      </div>
    );
  }

  if (job.status === "done") {
    const r = job.result;
    return (
      <div className="flex flex-col gap-3">
        <div className="flex items-center gap-2 rounded-lg border border-line bg-surface-2 px-3 py-2">
          <code aria-label="SHA-256" className="selectable flex-1 font-mono text-[12px] break-all">
            {r.sha256}
          </code>
          <Button
            onClick={() => {
              void navigator.clipboard?.writeText(r.sha256).then(() => setCopied(true));
            }}
          >
            {copied ? "Copied" : "Copy"}
          </Button>
        </div>
        <div className="text-[12.5px] text-muted">
          {formatBytes(r.bytes)} hashed in {(r.elapsed_ms / 1000).toFixed(1)}s.
        </div>
        {r.unreadable_bytes > 0 && (
          <ErrorBanner
            message={`${formatBytes(r.unreadable_bytes)} could not be read and were hashed as zeros (${r.unreadable_ranges.length} ranges).`}
          />
        )}
        <div>
          <Button onClick={onStart}>Compute again</Button>
        </div>
      </div>
    );
  }

  return (
    <div className="flex items-center gap-3">
      <Button icon="shield" onClick={onStart}>
        Compute SHA-256
      </Button>
      {job.status === "cancelled" && <span className="text-[12.5px] text-muted">Cancelled.</span>}
      {job.status === "error" && (
        <span role="alert" className="text-[12.5px] text-danger">
          {job.message}
        </span>
      )}
    </div>
  );
}
