import { useCallback, useMemo, useState } from "react";
import type { ItemView, ScanItemRow, Scope, SelectionPart } from "./types";

export interface FolderPick {
  scope: Scope;
  dir: string;
  view: ItemView;
  label: string;
  files: number;
}

export function scopeKey(s: Scope): string {
  return s.kind === "carved" ? "carved" : `v${s.offset}`;
}

function folderKey(f: { scope: Scope; dir: string; view: ItemView }): string {
  return `${scopeKey(f.scope)}|${f.view}|${f.dir}`;
}

/** Whether `path` lies inside `dir` (or is `dir` itself for files directly in it). */
export function within(dir: string, path: string): boolean {
  return dir === "/" || path.startsWith(dir.endsWith("/") ? dir : `${dir}/`);
}

/**
 * Selection for recovery: explicit files plus whole folders (resolved server-side, recursively,
 * under the view that was active when the folder was picked).
 */
export function useSelection() {
  const [items, setItems] = useState<Map<number, ScanItemRow>>(new Map());
  const [folders, setFolders] = useState<Map<string, FolderPick>>(new Map());

  const toggleItem = useCallback((row: ScanItemRow) => {
    setItems((cur) => {
      const next = new Map(cur);
      if (next.has(row.id)) next.delete(row.id);
      else next.set(row.id, row);
      return next;
    });
  }, []);

  const toggleFolder = useCallback((pick: FolderPick) => {
    setFolders((cur) => {
      const next = new Map(cur);
      const k = folderKey(pick);
      if (next.has(k)) next.delete(k);
      else next.set(k, pick);
      return next;
    });
  }, []);

  const clear = useCallback(() => {
    setItems(new Map());
    setFolders(new Map());
  }, []);

  const isFolderSelected = useCallback((f: { scope: Scope; dir: string; view: ItemView }) => folders.has(folderKey(f)), [folders]);

  /** A file is covered if picked directly or by a selected folder that contains it. */
  const isItemCovered = useCallback(
    (row: ScanItemRow, scope: Scope | null) => {
      if (items.has(row.id)) return "direct" as const;
      if (!scope) return null;
      for (const f of folders.values()) {
        if (scopeKey(f.scope) === scopeKey(scope) && within(f.dir, row.path)) return "folder" as const;
      }
      return null;
    },
    [items, folders],
  );

  const parts = useMemo<SelectionPart[]>(() => {
    const p: SelectionPart[] = [];
    if (items.size > 0) p.push({ kind: "items", ids: [...items.keys()] });
    for (const f of folders.values()) p.push({ kind: "folder", scope: f.scope, dir: f.dir, view: f.view });
    return p;
  }, [items, folders]);

  const fileCount = items.size + [...folders.values()].reduce((n, f) => n + f.files, 0);
  return {
    items,
    folders,
    parts,
    fileCountEstimate: fileCount,
    empty: items.size === 0 && folders.size === 0,
    toggleItem,
    toggleFolder,
    isFolderSelected,
    isItemCovered,
    clear,
  };
}

export type Selection = ReturnType<typeof useSelection>;
