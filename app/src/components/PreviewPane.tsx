import { useEffect, useMemo, useState } from "react";
import { api, errorMessage } from "../lib/api";
import { formatBytes, formatDateTime } from "../lib/format";
import {
  HEX_PREVIEW_MAX,
  IMAGE_MIME,
  IMAGE_PREVIEW_MAX,
  QUALITY_TEXT,
  TEXT_PREVIEW_MAX,
  hexRows,
  isAllZero,
  previewKind,
} from "../lib/preview";
import type { ScanItemDetail, ScanItemRow } from "../lib/types";
import { itemStatus } from "../lib/itemStatus";
import { ErrorBanner } from "./ui";
import { Icon } from "./Icon";

type Tab = "preview" | "hex" | "details";

type Loaded =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; detail: ScanItemDetail; bytes: Uint8Array };

export function PreviewPane({ item }: { item: ScanItemRow | null }) {
  const [tab, setTab] = useState<Tab>("preview");
  const [state, setState] = useState<Loaded>({ status: "loading" });

  useEffect(() => {
    if (!item) return;
    let live = true;
    setState({ status: "loading" });
    const t = (item.file_type ?? "").toLowerCase();
    const want = t in IMAGE_MIME ? Math.min(item.size, IMAGE_PREVIEW_MAX) : Math.max(TEXT_PREVIEW_MAX, HEX_PREVIEW_MAX);
    Promise.all([api.getScanItem(item.id), api.readItemData(item.id, 0, want)])
      .then(([detail, bytes]) => live && setState({ status: "ready", detail, bytes }))
      .catch((e) => live && setState({ status: "error", message: errorMessage(e) }));
    return () => {
      live = false;
    };
  }, [item]);

  if (!item) {
    return (
      <div className="grid h-full place-items-center p-6 text-center text-[13px] text-muted">
        Select a file to preview it.
      </div>
    );
  }

  const st = itemStatus(item);
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="border-b border-line px-4 pt-4 pb-3">
        <div className="selectable truncate text-[14px] font-semibold" title={item.name}>
          {item.name}
        </div>
        <div className="mt-0.5 text-[12px] text-muted">
          {formatBytes(item.size)} · {st.text}
        </div>
        <div role="tablist" aria-label="Preview mode" className="mt-3 inline-flex rounded-lg border border-line p-0.5">
          {(["preview", "hex", "details"] as const).map((t) => (
            <button
              key={t}
              role="tab"
              type="button"
              aria-selected={tab === t}
              onClick={() => setTab(t)}
              className={`rounded-md px-3 py-1 text-[12.5px] font-medium capitalize ${
                tab === t ? "bg-surface-2 text-fg" : "text-muted hover:text-fg"
              }`}
            >
              {t}
            </button>
          ))}
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-4">
        {state.status === "loading" && <p className="text-[12.5px] text-muted">Reading from source…</p>}
        {state.status === "error" && <ErrorBanner message={state.message} />}
        {state.status === "ready" && tab === "preview" && <Preview item={item} bytes={state.bytes} />}
        {state.status === "ready" && tab === "hex" && <HexView bytes={state.bytes.subarray(0, HEX_PREVIEW_MAX)} total={item.size} />}
        {state.status === "ready" && tab === "details" && <Details detail={state.detail} />}
      </div>
    </div>
  );
}

function Notice({ tone, children }: { tone: "warn" | "danger" | "muted"; children: React.ReactNode }) {
  const cls = {
    warn: "border-warn/40 bg-warn/10 text-warn",
    danger: "border-danger/40 bg-danger/10 text-danger",
    muted: "border-line text-muted",
  }[tone];
  return (
    <div className={`mb-3 flex gap-2 rounded-lg border px-3 py-2 text-[12.5px] ${cls}`}>
      <Icon name="alert" size={15} className="mt-0.5 shrink-0" />
      <span>{children}</span>
    </div>
  );
}

function Preview({ item, bytes }: { item: ScanItemRow; bytes: Uint8Array }) {
  const kind = previewKind(item.file_type, item.size, bytes);
  if (item.size === 0) return <p className="text-[12.5px] text-muted">This file is empty.</p>;
  if (isAllZero(bytes.subarray(0, 64 * 1024))) {
    return (
      <Notice tone="danger">
        The data reads back as zeros. It was erased (typically by TRIM on SSDs and flash drives) and cannot be
        recovered by any tool.
      </Notice>
    );
  }
  return (
    <>
      {item.extent_quality !== "exact" && (
        <Notice tone="warn">{QUALITY_TEXT[item.extent_quality]} The preview may show damaged content.</Notice>
      )}
      {/* Browsers render truncated images with blank areas, so a successful decode only proves
          the file is intact when its structure was also verified. */}
      {kind === "image" && (
        <ImagePreview
          bytes={bytes}
          mime={IMAGE_MIME[(item.file_type ?? "").toLowerCase()]!}
          complete={item.extent_quality === "exact" && item.carve_status !== "partial"}
        />
      )}
      {kind === "text" && <TextPreview bytes={bytes.subarray(0, TEXT_PREVIEW_MAX)} truncated={item.size > TEXT_PREVIEW_MAX} />}
      {kind === "binary" && (
        <>
          <p className="mb-3 text-[12.5px] text-muted">No visual preview for this file type. Showing the first bytes:</p>
          <HexView bytes={bytes.subarray(0, 2048)} total={item.size} />
        </>
      )}
    </>
  );
}

function ImagePreview({ bytes, mime, complete }: { bytes: Uint8Array; mime: string; complete: boolean }) {
  const url = useMemo(
    () => (typeof URL.createObjectURL === "function" ? URL.createObjectURL(new Blob([bytes as BlobPart], { type: mime })) : null),
    [bytes, mime],
  );
  const [state, setState] = useState<{ ok: boolean; w?: number; h?: number } | null>(null);
  useEffect(() => () => {
    if (url) URL.revokeObjectURL(url);
  }, [url]);
  if (!url) return null;
  return (
    <div>
      {state?.ok === false && (
        <Notice tone="warn">The image could not be decoded. The file is probably damaged or incomplete.</Notice>
      )}
      <div className="grid place-items-center rounded-lg border border-line bg-[repeating-conic-gradient(var(--k-surface-2)_0_25%,transparent_0_50%)] bg-[length:16px_16px] p-2">
        <img
          src={url}
          alt="Preview"
          className={`max-h-[60vh] max-w-full object-contain ${state?.ok === false ? "hidden" : ""}`}
          onLoad={(e) => setState({ ok: true, w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
          onError={() => setState({ ok: false })}
        />
      </div>
      {state?.ok && (
        <p className="mt-2 text-[12px] text-muted tabular-nums">
          {state.w} × {state.h} px ·{" "}
          {complete
            ? "decoded successfully"
            : "the decoder accepted the data, but missing parts of the image are shown blank"}
        </p>
      )}
    </div>
  );
}

function TextPreview({ bytes, truncated }: { bytes: Uint8Array; truncated: boolean }) {
  const text = useMemo(() => new TextDecoder("utf-8", { fatal: false }).decode(bytes), [bytes]);
  return (
    <>
      <pre className="selectable font-mono text-[12px] leading-relaxed break-words whitespace-pre-wrap">{text}</pre>
      {truncated && <p className="mt-2 text-[12px] text-muted">Showing the first {formatBytes(TEXT_PREVIEW_MAX)}.</p>}
    </>
  );
}

function HexView({ bytes, total }: { bytes: Uint8Array; total: number }) {
  const rows = useMemo(() => hexRows(bytes), [bytes]);
  return (
    <>
      <div role="table" aria-label="Hex view" className="selectable font-mono text-[11.5px] leading-[1.55]">
        {rows.map((r) => (
          <div role="row" key={r.offset} className="flex gap-3 whitespace-pre">
            <span className="text-muted">{r.offset}</span>
            <span>{r.hex.padEnd(47, " ")}</span>
            <span className="text-muted">{r.ascii}</span>
          </div>
        ))}
      </div>
      {total > bytes.length && (
        <p className="mt-2 text-[12px] text-muted">
          First {formatBytes(bytes.length)} of {formatBytes(total)}.
        </p>
      )}
    </>
  );
}

function Details({ detail: d }: { detail: ScanItemDetail }) {
  const st = itemStatus(d);
  const rows: [string, React.ReactNode][] = [
    ["Path", <span className="selectable font-mono text-[12px] break-all">{d.path}</span>],
    ["Size", `${d.size.toLocaleString()} bytes (${formatBytes(d.size)})`],
    ["Status", `${st.text}${st.hint ? ` — ${st.hint}` : ""}`],
    ["Found by", d.origin === "carved" ? "File signature (deep scan)" : `${d.filesystem ?? "Filesystem"} metadata`],
    ["Location", QUALITY_TEXT[d.extent_quality]],
    ["Modified", formatDateTime(d.modified_at)],
    ["Created", formatDateTime(d.created_at)],
  ];
  if (d.note) rows.push(["Note", d.note]);
  return (
    <div className="flex flex-col gap-4">
      <dl className="grid grid-cols-[88px_1fr] gap-x-3 gap-y-2 text-[12.5px]">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-muted">{k}</dt>
            <dd>{v}</dd>
          </div>
        ))}
      </dl>
      <div>
        <h4 className="mb-1.5 text-[12px] font-semibold tracking-wide text-muted uppercase">
          On-disk extents ({d.resident ? "stored inside the file record" : d.extents.length})
        </h4>
        <ul className="selectable font-mono text-[11.5px] text-muted">
          {d.extents.slice(0, 20).map(([off, len], i) => (
            <li key={i}>
              {off === null ? "sparse" : `@ ${off.toLocaleString()}`} · {len.toLocaleString()} bytes
            </li>
          ))}
          {d.extents.length > 20 && <li>… {d.extents.length - 20} more</li>}
        </ul>
      </div>
    </div>
  );
}
