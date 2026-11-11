import { formatBytes } from "../lib/format";
import type { FsKind, ImageAnalysis, TableKind, VolumeAnalysis } from "../lib/types";
import { Icon } from "./Icon";

const TABLE_LABEL: Record<TableKind, string> = {
  mbr: "MBR partition table",
  gpt: "GPT partition table",
  none: "No partition table (filesystem starts at sector 0)",
  unknown: "No recognisable partition table",
};

export const FS_LABEL: Record<FsKind, string> = {
  fat12: "FAT12",
  fat16: "FAT16",
  fat32: "FAT32",
  ex_fat: "exFAT",
  ntfs: "NTFS",
  hfs_plus: "HFS+",
  hfsx: "HFSX",
  apfs: "APFS",
  bit_locker: "BitLocker",
  ext: "ext2/3/4",
  unknown: "Unknown",
};

/** What the engine can do with a volume, stated plainly. */
export function volumeStatus(v: VolumeAnalysis): { text: string; tone: "ok" | "warn" | "muted" } {
  if (v.fs.encrypted) return { text: "Encrypted — unlock in the OS first", tone: "warn" };
  if (v.walkable) return { text: "Readable", tone: "ok" };
  if (v.fs.kind === "unknown") return { text: "No filesystem recognised", tone: "muted" };
  return { text: "Detected only", tone: "muted" };
}

const TONE = {
  ok: "border-ok/40 bg-ok/10 text-ok",
  warn: "border-warn/40 bg-warn/10 text-warn",
  muted: "border-line text-muted",
};

export function VolumeLayout({ analysis, diskSize }: { analysis: ImageAnalysis; diskSize: number }) {
  const { table, volumes } = analysis;
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between gap-3 text-[12.5px]">
        <span className="font-medium">{TABLE_LABEL[table.kind]}</span>
        <span className="text-muted tabular-nums">
          {table.kind === "gpt" && table.disk_guid ? `GUID ${table.disk_guid}` : null}
          {table.kind === "mbr" && table.disk_signature !== null
            ? `Signature ${table.disk_signature.toString(16).toUpperCase().padStart(8, "0")}`
            : null}
        </span>
      </div>

      <div
        role="img"
        aria-label={`Partition map with ${volumes.length} volumes`}
        className="relative h-3 overflow-hidden rounded-full bg-surface-2"
      >
        {diskSize > 0 &&
          volumes.map((v) => (
            <div
              key={`${v.offset}`}
              title={`${FS_LABEL[v.fs.kind]} ${v.fs.label ?? ""} · ${formatBytes(v.size)}`}
              className={`absolute inset-y-0 border-r-2 border-surface ${v.walkable ? "bg-accent/70" : "bg-muted/40"}`}
              style={{ left: `${(v.offset / diskSize) * 100}%`, width: `${Math.max((v.size / diskSize) * 100, 0.6)}%` }}
            />
          ))}
      </div>

      {table.warnings.length > 0 && (
        <ul role="alert" className="flex flex-col gap-1 rounded-lg border border-warn/40 bg-warn/10 px-3 py-2 text-[12.5px] text-warn">
          {table.warnings.map((w) => (
            <li key={w} className="flex gap-2">
              <Icon name="alert" size={14} className="mt-0.5 shrink-0" /> {w}
            </li>
          ))}
        </ul>
      )}

      <table className="w-full text-[12.5px]">
        <thead>
          <tr className="text-left text-[11px] tracking-wide text-muted uppercase">
            <th className="py-1.5 pr-3 font-medium">#</th>
            <th className="py-1.5 pr-3 font-medium">Filesystem</th>
            <th className="py-1.5 pr-3 font-medium">Label</th>
            <th className="py-1.5 pr-3 font-medium">Partition type</th>
            <th className="py-1.5 pr-3 text-right font-medium">Size</th>
            <th className="py-1.5 font-medium">Status</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-line border-t border-line">
          {volumes.map((v) => {
            const status = volumeStatus(v);
            return (
              <tr key={`${v.offset}`}>
                <td className="py-2 pr-3 tabular-nums text-muted">{v.partition ?? "—"}</td>
                <td className="py-2 pr-3 font-medium">
                  {FS_LABEL[v.fs.kind]}
                  {v.fs.cluster_size ? (
                    <span className="ml-1.5 font-normal text-muted">{formatBytes(v.fs.cluster_size)} clusters</span>
                  ) : null}
                </td>
                <td className="py-2 pr-3">{v.fs.label ?? <span className="text-muted">—</span>}</td>
                <td className="py-2 pr-3 text-muted">{v.type_name}</td>
                <td className="py-2 pr-3 text-right tabular-nums">{formatBytes(v.size)}</td>
                <td className="py-2">
                  <span className={`rounded-md border px-1.5 py-0.5 text-[11px] font-medium ${TONE[status.tone]}`}>
                    {status.text}
                  </span>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
