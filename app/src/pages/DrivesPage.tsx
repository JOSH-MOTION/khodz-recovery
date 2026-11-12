import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { formatBytes } from "../lib/format";
import type { BusType, PhysicalDisk, Volume } from "../lib/types";
import { useAsync } from "../lib/useAsync";
import { Button, Card, EmptyState, ErrorBanner, PageHeader } from "../components/ui";
import { Icon } from "../components/Icon";
import { ScanDialog } from "../components/ScanDialog";
import { PartitionCheck } from "../components/PartitionCheck";

const BUS_LABEL: Record<BusType, string> = {
  usb: "USB",
  sata: "SATA",
  nvme: "NVMe",
  pci_express: "PCIe",
  sas: "SAS",
  scsi: "SCSI",
  ata: "ATA",
  atapi: "ATAPI",
  sd_card: "SD card",
  mmc: "MMC",
  thunderbolt: "Thunderbolt",
  apple_fabric: "Apple internal",
  firewire: "FireWire",
  fibre_channel: "Fibre Channel",
  raid: "RAID",
  iscsi: "iSCSI",
  storage_spaces: "Storage Spaces",
  virtual: "Virtual",
  disk_image: "Disk image",
  ufs: "UFS",
  unknown: "Unknown bus",
};

export function isExternal(d: PhysicalDisk): boolean {
  return d.removable || ["usb", "thunderbolt", "firewire", "sd_card", "mmc"].includes(d.bus);
}

export function DrivesPage() {
  const { state, reload } = useAsync(api.listDevices);
  const [selected, setSelected] = useState<string | null>(null);

  // Drives come and go (USB sticks); refresh when the user returns to the window.
  useEffect(() => {
    window.addEventListener("focus", reload);
    return () => window.removeEventListener("focus", reload);
  }, [reload]);

  const disks = state.status === "ready" ? state.data : [];
  const total = disks.reduce((sum, d) => sum + d.size_bytes, 0);

  return (
    <>
      <PageHeader
        title="Drives"
        subtitle={
          state.status === "ready" && disks.length > 0
            ? `${disks.length} ${disks.length === 1 ? "drive" : "drives"} · ${formatBytes(total)} total. Choose a drive to scan for lost files.`
            : "Choose a drive or partition to scan for lost files."
        }
        actions={
          <Button icon="refresh" onClick={reload} disabled={state.status === "loading"}>
            Refresh
          </Button>
        }
      />
      <div className="px-8 py-6">
        {state.status === "loading" && (
          <div aria-label="Loading drives" className="grid grid-cols-1 gap-4 xl:grid-cols-2">
            {[0, 1].map((i) => (
              <div key={i} className="h-56 animate-pulse rounded-xl border border-line bg-surface" />
            ))}
          </div>
        )}
        {state.status === "error" && <ErrorBanner message={state.error} onRetry={reload} />}
        {state.status === "ready" && disks.length === 0 && (
          <EmptyState icon="drive" title="No drives found">
            The operating system reported no disks. Connect the drive you want to recover from, then press Refresh.
          </EmptyState>
        )}
        {state.status === "ready" && disks.length > 0 && (
          <div className="k-enter grid grid-cols-1 items-start gap-4 xl:grid-cols-2">
            {disks.map((d) => (
              <DriveCard
                key={d.id}
                disk={d}
                selected={selected === d.id}
                onSelect={() => setSelected(d.id === selected ? null : d.id)}
              />
            ))}
          </div>
        )}
      </div>
    </>
  );
}

function Chip({ children, tone = "neutral", title }: { children: string; tone?: "neutral" | "warn" | "accent"; title?: string }) {
  const tones = {
    neutral: "border-line text-muted",
    warn: "border-warn/40 text-warn bg-warn/10",
    accent: "border-accent/40 text-accent bg-accent-soft",
  };
  return (
    <span title={title} className={`rounded-md border px-1.5 py-0.5 text-[11px] font-medium ${tones[tone]}`}>
      {children}
    </span>
  );
}

function DriveCard({ disk, selected, onSelect }: { disk: PhysicalDisk; selected: boolean; onSelect: () => void }) {
  const external = isExternal(disk);
  const [scanning, setScanning] = useState(false);
  const [checking, setChecking] = useState(false);
  return (
    <Card
      className={`flex flex-col transition-[box-shadow,border-color] ${
        selected ? "border-accent shadow-[0_0_0_1px_var(--k-accent)]" : "hover:border-muted/50"
      }`}
    >
      <button
        type="button"
        onClick={onSelect}
        aria-pressed={selected}
        aria-label={`${disk.display_name}, ${formatBytes(disk.size_bytes)}`}
        className="flex items-start gap-4 rounded-t-xl p-5 text-left outline-none focus-visible:ring-2 focus-visible:ring-accent/60"
      >
        <div className="grid h-12 w-12 shrink-0 place-items-center rounded-xl border border-line bg-surface-2">
          <Icon name={external ? "usb" : "drive"} size={24} className="text-accent" />
        </div>
        <div className="min-w-0 flex-1">
          <div className="flex items-baseline justify-between gap-3">
            <h2 className="truncate text-[15px] font-semibold">{disk.display_name}</h2>
            <span className="shrink-0 text-[15px] font-semibold tabular-nums">{formatBytes(disk.size_bytes)}</span>
          </div>
          <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
            <Chip>{BUS_LABEL[disk.bus]}</Chip>
            {disk.is_system && (
              <Chip tone="warn" title="This drive runs the operating system. Recover files to a different drive.">
                System
              </Chip>
            )}
            {external && <Chip>{disk.removable ? "Removable" : "External"}</Chip>}
            {disk.solid_state === true && <Chip>SSD</Chip>}
            {disk.solid_state === false && <Chip>HDD</Chip>}
            {disk.is_virtual && <Chip>Virtual</Chip>}
            {!disk.media_present && <Chip tone="warn">No media</Chip>}
          </div>
        </div>
      </button>

      {disk.media_present && <PartitionMap disk={disk} />}

      <ul className="flex flex-col divide-y divide-line border-t border-line">
        {disk.volumes.length === 0 && (
          <li className="px-5 py-3 text-[13px] text-muted">
            {disk.media_present ? "No volumes the operating system can see." : "Insert media to see its volumes."}
          </li>
        )}
        {disk.volumes.map((v) => (
          <VolumeRow key={v.id} volume={v} />
        ))}
      </ul>

      <div className="mt-auto flex items-center justify-between gap-3 border-t border-line px-5 py-3">
        <div className="selectable min-w-0 truncate font-mono text-[11.5px] text-muted" title={disk.device_path}>
          {disk.device_path} · {disk.logical_sector_size} B sectors
        </div>
        <div className="flex shrink-0 gap-2">
          <Button
            disabled={!disk.media_present}
            title="Check the partition table and search for lost partitions (needs administrator rights)"
            onClick={() => setChecking(true)}
          >
            Partitions…
          </Button>
          <Button
            variant="primary"
            icon="scan"
            disabled={!disk.media_present}
            title="Reading a drive directly requires running Khodz Recovery as administrator"
            onClick={() => setScanning(true)}
          >
            Scan
          </Button>
        </div>
        {checking && <PartitionDialog disk={disk} onClose={() => setChecking(false)} />}
        {scanning && (
          <ScanDialog
            source={{ kind: "device", path: disk.device_path, label: disk.display_name }}
            label={disk.display_name}
            onClose={() => setScanning(false)}
          />
        )}
      </div>
    </Card>
  );
}

function PartitionDialog({ disk, onClose }: { disk: PhysicalDisk; onClose: () => void }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div className="fixed inset-0 z-40 grid place-items-center bg-black/50 p-6" onMouseDown={onClose}>
      <div
        role="dialog"
        aria-modal="true"
        aria-label={`Partitions of ${disk.display_name}`}
        className="k-enter max-h-full w-full max-w-2xl overflow-y-auto rounded-xl border border-line bg-surface shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex items-start justify-between gap-4 border-b border-line px-5 py-4">
          <div>
            <h2 className="text-[15px] font-semibold">Partitions of {disk.display_name}</h2>
            <p className="mt-1 flex items-center gap-1.5 text-[12.5px] text-muted">
              <Icon name="shield" size={14} className="text-ok" /> Read-only: the partition table is never changed.
            </p>
          </div>
          <Button onClick={onClose}>Close</Button>
        </div>
        <div className="p-5">
          <PartitionCheck
            source={{ kind: "device", path: disk.device_path, label: disk.display_name }}
            label={disk.display_name}
          />
        </div>
      </div>
    </div>
  );
}

/** Proportional map of where volumes sit on the disk; gaps are space no volume claims. */
function PartitionMap({ disk }: { disk: PhysicalDisk }) {
  const placed = disk.volumes.filter((v) => v.offset_bytes !== null && v.size_bytes !== null && disk.size_bytes > 0);
  return (
    <div className="px-5 pb-4">
      <div
        role="img"
        aria-label={`Partition map: ${placed.length} of ${disk.volumes.length} volumes placed`}
        className="relative h-2.5 overflow-hidden rounded-full bg-surface-2"
      >
        {placed.map((v) => {
          const left = ((v.offset_bytes ?? 0) / disk.size_bytes) * 100;
          const width = Math.max(((v.size_bytes ?? 0) / disk.size_bytes) * 100, 0.6);
          return (
            <div
              key={v.id}
              title={`${volumeName(v)} · ${v.filesystem ?? "unknown"} · ${formatBytes(v.size_bytes ?? 0)}`}
              className={`absolute inset-y-0 border-r-2 border-surface ${v.is_system ? "bg-accent" : "bg-accent/55"}`}
              style={{ left: `${left}%`, width: `${width}%` }}
            />
          );
        })}
      </div>
    </div>
  );
}

function volumeName(v: Volume): string {
  return v.label ?? (v.mount_points.length > 0 ? "Local disk" : "Unnamed volume");
}

function VolumeRow({ volume: v }: { volume: Volume }) {
  const mount = v.mount_points[0];
  const used = v.size_bytes !== null && v.free_bytes !== null ? v.size_bytes - v.free_bytes : null;
  const pct = used !== null && v.size_bytes ? Math.min(100, (used / v.size_bytes) * 100) : null;
  return (
    <li className="flex items-center gap-3 px-5 py-2.5 text-[13px]">
      <span
        className={`w-14 shrink-0 truncate rounded-md px-1.5 py-0.5 text-center font-mono text-[11.5px] ${
          mount ? "bg-surface-2 text-fg" : "text-muted"
        }`}
        title={v.mount_points.join(", ") || "Not mounted"}
      >
        {mount ? mount.replace(/\\$/, "") : "—"}
      </span>
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          <span className="truncate font-medium">{volumeName(v)}</span>
          {v.encrypted && <Icon name="lock" size={13} className="shrink-0 text-warn" />}
          {v.is_system && <span className="text-[11px] text-muted">· OS</span>}
        </div>
        {pct !== null && (
          <div className="mt-1 h-1 w-full max-w-48 overflow-hidden rounded-full bg-surface-2">
            <div className={`h-full rounded-full ${pct > 90 ? "bg-warn" : "bg-accent/70"}`} style={{ width: `${pct}%` }} />
          </div>
        )}
      </div>
      <span className="w-16 shrink-0 text-right text-[12px] text-muted">{v.filesystem ?? "Unknown"}</span>
      <span className="w-36 shrink-0 text-right text-[12px] tabular-nums text-muted">
        {v.size_bytes === null
          ? "—"
          : used !== null
            ? `${formatBytes(used)} of ${formatBytes(v.size_bytes)}`
            : formatBytes(v.size_bytes)}
      </span>
    </li>
  );
}
