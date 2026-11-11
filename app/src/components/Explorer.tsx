import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import { api, errorMessage } from "../lib/api";
import { formatBytes } from "../lib/format";
import { TONES, itemStatus } from "../lib/itemStatus";
import type { FolderEntry, ItemView, ScanItemRow, ScanVolume, Scope } from "../lib/types";
import { Button, ErrorBanner } from "./ui";
import { Icon } from "./Icon";

const PAGE = 200;

export interface Location {
  scope: Scope;
  dir: string;
}

function scopeKey(s: Scope) {
  return s.kind === "carved" ? "carved" : `v${s.offset}`;
}

function volumeName(v: ScanVolume) {
  return v.label ? `${v.label} (${v.filesystem})` : `${v.filesystem} volume`;
}

/** Left pane: volumes of the scan plus the "found by signature" group. */
function Locations({
  volumes,
  carved,
  current,
  onSelect,
}: {
  volumes: ScanVolume[];
  carved: number;
  current: Scope;
  onSelect: (s: Scope) => void;
}) {
  const entries: { scope: Scope; title: string; sub: string; error?: string | null }[] = [
    ...volumes.map((v) => ({
      scope: { kind: "volume", offset: v.offset } as Scope,
      title: volumeName(v),
      sub: `${v.files.toLocaleString()} files · ${v.deleted.toLocaleString()} deleted`,
      error: v.error,
    })),
    ...(carved > 0
      ? [{ scope: { kind: "carved" } as Scope, title: "Found by signature", sub: `${carved.toLocaleString()} files` }]
      : []),
  ];
  return (
    <nav aria-label="Locations" className="flex flex-col gap-1 p-2">
      <div className="px-2 pt-1 pb-1.5 text-[11px] font-medium tracking-wide text-muted uppercase">Locations</div>
      {entries.map((e) => {
        const active = scopeKey(e.scope) === scopeKey(current);
        return (
          <button
            key={scopeKey(e.scope)}
            type="button"
            aria-current={active ? "true" : undefined}
            onClick={() => onSelect(e.scope)}
            className={`flex items-start gap-2.5 rounded-lg px-2.5 py-2 text-left outline-none focus-visible:ring-2 focus-visible:ring-accent/60 ${
              active ? "bg-accent-soft" : "hover:bg-surface-2"
            }`}
          >
            <Icon name={e.scope.kind === "carved" ? "scan" : "drive"} size={16} className="mt-0.5 shrink-0 text-accent" />
            <span className="min-w-0">
              <span className="block truncate text-[13px] font-medium">{e.title}</span>
              <span className="block text-[11.5px] text-muted">{e.sub}</span>
              {e.error && <span className="block text-[11.5px] text-danger">{e.error}</span>}
            </span>
          </button>
        );
      })}
      {entries.length === 0 && <p className="px-2 text-[12.5px] text-muted">Nothing found.</p>}
    </nav>
  );
}

function Breadcrumb({ title, dir, onNavigate }: { title: string; dir: string; onNavigate: (dir: string) => void }) {
  const parts = dir.split("/").filter(Boolean);
  return (
    <ol aria-label="Folder path" className="flex min-w-0 flex-wrap items-center gap-1 text-[12.5px]">
      <li>
        <button type="button" onClick={() => onNavigate("/")} className="font-medium hover:text-accent">
          {title}
        </button>
      </li>
      {parts.map((p, i) => {
        const path = "/" + parts.slice(0, i + 1).join("/");
        return (
          <li key={path} className="flex items-center gap-1">
            <span className="text-muted">/</span>
            <button type="button" onClick={() => onNavigate(path)} className="truncate hover:text-accent">
              {p}
            </button>
          </li>
        );
      })}
    </ol>
  );
}

/**
 * Middle pane: folders then files of the current location. Keyboard: ↑/↓ move, Enter opens a
 * folder, Backspace goes up.
 */
function Listing({
  scanId,
  location,
  title,
  view,
  selected,
  onNavigate,
  onSelect,
}: {
  scanId: number;
  location: Location;
  title: string;
  view: ItemView;
  selected: ScanItemRow | null;
  onNavigate: (dir: string) => void;
  onSelect: (item: ScanItemRow | null) => void;
}) {
  const [folders, setFolders] = useState<FolderEntry[]>([]);
  const [files, setFiles] = useState<ScanItemRow[]>([]);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [cursor, setCursor] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let live = true;
    setError(null);
    setCursor(0);
    api
      .listFolder(scanId, location.scope, location.dir, view, { limit: PAGE, offset: 0 })
      .then((l) => {
        if (!live) return;
        setFolders(l.folders);
        setFiles(l.files);
        setTotal(l.total_files);
      })
      .catch((e) => live && setError(errorMessage(e)));
    return () => {
      live = false;
    };
  }, [scanId, location, view]);

  const loadMore = () =>
    api
      .listFolder(scanId, location.scope, location.dir, view, { limit: PAGE, offset: files.length })
      .then((l) => setFiles((cur) => [...cur, ...l.files]))
      .catch((e) => setError(errorMessage(e)));

  const parent = location.dir === "/" ? null : location.dir.slice(0, location.dir.lastIndexOf("/")) || "/";
  const rows = folders.length + files.length;

  const activate = useCallback(
    (i: number) => {
      if (i < folders.length) onNavigate(folders[i]!.path);
      else onSelect(files[i - folders.length] ?? null);
    },
    [folders, files, onNavigate, onSelect],
  );

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const next = Math.max(0, Math.min(rows - 1, cursor + (e.key === "ArrowDown" ? 1 : -1)));
      setCursor(next);
      if (next >= folders.length) onSelect(files[next - folders.length] ?? null);
    } else if (e.key === "Enter") {
      activate(cursor);
    } else if (e.key === "Backspace" && parent !== null) {
      e.preventDefault();
      onNavigate(parent);
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-2 border-b border-line px-4 py-2.5">
        <button
          type="button"
          aria-label="Up one folder"
          disabled={parent === null}
          onClick={() => parent !== null && onNavigate(parent)}
          className="grid h-7 w-7 place-items-center rounded-md border border-line text-muted hover:text-fg disabled:opacity-40"
        >
          ↑
        </button>
        <Breadcrumb title={title} dir={location.dir} onNavigate={onNavigate} />
      </div>
      {error && (
        <div className="p-3">
          <ErrorBanner message={error} />
        </div>
      )}
      <div
        ref={listRef}
        role="listbox"
        aria-label="Files and folders"
        tabIndex={0}
        onKeyDown={onKey}
        className="min-h-0 flex-1 overflow-y-auto outline-none focus-visible:ring-2 focus-visible:ring-accent/40 focus-visible:ring-inset"
      >
        {folders.map((f, i) => (
          <div
            key={f.path}
            role="option"
            aria-selected={cursor === i}
            onClick={() => setCursor(i)}
            onDoubleClick={() => onNavigate(f.path)}
            className={`flex cursor-default items-center gap-3 border-b border-line px-4 py-2 text-[13px] ${
              cursor === i ? "bg-surface-2" : "hover:bg-surface-2/60"
            }`}
          >
            <span aria-hidden className="text-accent">▸</span>
            <span className="min-w-0 flex-1 truncate font-medium">{f.name}</span>
            <span className="shrink-0 text-[12px] text-muted tabular-nums">
              {f.files.toLocaleString()} files{f.deleted > 0 ? ` · ${f.deleted.toLocaleString()} deleted` : ""}
            </span>
            <button
              type="button"
              aria-label={`Open ${f.name}`}
              onClick={(e) => {
                e.stopPropagation();
                onNavigate(f.path);
              }}
              className="rounded-md px-2 py-0.5 text-[12px] text-accent hover:bg-accent-soft"
            >
              Open
            </button>
          </div>
        ))}
        {files.map((r, j) => {
          const i = folders.length + j;
          const st = itemStatus(r);
          const isSel = selected?.id === r.id;
          return (
            <div
              key={r.id}
              role="option"
              aria-selected={isSel}
              onClick={() => {
                setCursor(i);
                onSelect(r);
              }}
              className={`flex cursor-default items-center gap-3 border-b border-line px-4 py-2 text-[13px] ${
                isSel ? "bg-accent-soft" : "hover:bg-surface-2/60"
              }`}
            >
              {/* Names wrap rather than truncate: carved names differ only at the end. */}
              <span className="min-w-0 flex-1 break-all" title={r.name}>
                {r.name}
                {!r.name_complete && r.origin === "metadata" && <span className="ml-1.5 text-[11px] text-muted">(name partly lost)</span>}
              </span>
              <span className="w-16 shrink-0 text-right text-[12px] tabular-nums text-muted">{formatBytes(r.size)}</span>
              <span title={st.hint} className={`w-28 shrink-0 truncate rounded-md border px-1.5 py-0.5 text-center text-[11px] font-medium ${TONES[st.tone]}`}>
                {st.text}
              </span>
            </div>
          );
        })}
        {rows === 0 && !error && <p className="p-6 text-center text-[13px] text-muted">This folder has no files in this view.</p>}
        {files.length < total && (
          <div className="flex items-center justify-between px-4 py-2.5 text-[12.5px] text-muted">
            <span>
              {files.length.toLocaleString()} of {total.toLocaleString()} files
            </span>
            <Button onClick={() => void loadMore()}>Load more</Button>
          </div>
        )}
      </div>
    </div>
  );
}

export function Explorer({
  scanId,
  view,
  carvedCount,
  selected,
  onSelect,
}: {
  scanId: number;
  view: ItemView;
  carvedCount: number;
  selected: ScanItemRow | null;
  onSelect: (item: ScanItemRow | null) => void;
}) {
  const [volumes, setVolumes] = useState<ScanVolume[]>([]);
  const [location, setLocation] = useState<Location | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api
      .listVolumes(scanId, view)
      .then((v) => {
        if (!live) return;
        setVolumes(v);
        setLocation((cur) => {
          const stillUseful =
            cur &&
            (cur.scope.kind === "carved"
              ? carvedCount > 0
              : v.some((x) => cur.scope.kind === "volume" && x.offset === cur.scope.offset && x.files > 0));
          if (cur && stillUseful && view !== "carved") return cur;
          if (view === "carved") return { scope: { kind: "carved" }, dir: "/" };
          // Open where the files are: a volume with files, else the signature results, else
          // the first volume (which then explains itself, e.g. with its read error).
          const first = v.find((x) => x.files > 0);
          if (first) return { scope: { kind: "volume", offset: first.offset }, dir: "/" };
          if (carvedCount > 0 || v.length === 0) return { scope: { kind: "carved" }, dir: "/" };
          return { scope: { kind: "volume", offset: v[0]!.offset }, dir: "/" };
        });
      })
      .catch((e) => live && setError(errorMessage(e)));
    return () => {
      live = false;
    };
  }, [scanId, view, carvedCount]);

  // Reset when switching scans.
  useEffect(() => setLocation(null), [scanId]);

  // A volume with no files and no error (e.g. raw space with no filesystem) is just noise.
  const shownVolumes = view === "carved" ? [] : volumes.filter((v) => v.files > 0 || v.error);
  const title =
    location?.scope.kind === "volume"
      ? volumeName(volumes.find((v) => location.scope.kind === "volume" && v.offset === location.scope.offset) ?? volumes[0]!)
      : "Found by signature";

  return (
    <div className="grid min-h-0 flex-1 grid-cols-[200px_minmax(0,1fr)] overflow-hidden rounded-xl border border-line bg-surface">
      <div className="min-h-0 overflow-y-auto border-r border-line">
        <Locations
          volumes={shownVolumes}
          carved={view === "deleted" ? 0 : carvedCount}
          current={location?.scope ?? { kind: "carved" }}
          onSelect={(scope) => {
            setLocation({ scope, dir: "/" });
            onSelect(null);
          }}
        />
      </div>
      {error && <ErrorBanner message={error} />}
      {location && (
        <Listing
          scanId={scanId}
          location={location}
          title={title}
          view={view}
          selected={selected}
          onNavigate={(dir) => {
            setLocation({ ...location, dir });
            onSelect(null);
          }}
          onSelect={onSelect}
        />
      )}
    </div>
  );
}
