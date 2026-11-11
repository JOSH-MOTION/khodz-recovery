# Khodz Recovery — Architecture

Status: **Stage 1 implemented (2026-11-11)** — `khodz-core`, `khodz-db`, the Tauri shell and the
React UI exist. Every other crate below is design only until IMPLEMENTATION_PLAN.md marks its stage verified.

Engineering priority: **data safety → recovery correctness → reliability → performance → visual polish.**

## 1. System overview

```
┌──────────────────────────────────────────────────────────────┐
│ React + TypeScript + Tailwind UI   (app/src)                 │
│  Drives · Scan · Results explorer · Preview · Recovery · History
└───────────────▲──────────────────────────────┬───────────────┘
                │ Tauri events (progress)      │ Tauri commands (typed, serde)
┌───────────────┴──────────────────────────────▼───────────────┐
│ app/src-tauri  — thin command layer, no recovery logic       │
└───────────────▲──────────────────────────────────────────────┘
                │ Rust APIs only
┌───────────────┴──────────────────────────────────────────────┐
│ khodz-engine   — job orchestration, cancellation, progress   │
│   ├─ khodz-device   device/volume discovery  (win/ & macos/) │
│   ├─ khodz-io       read-only block sources, images, cache   │
│   ├─ khodz-part     MBR/GPT parsing, lost-partition search   │
│   ├─ khodz-fs       ntfs/ fat/ exfat/ hfsplus/ apfs/          │
│   ├─ khodz-carve    signature carving + structural validation│
│   ├─ khodz-recover  destination checks, writing, hashing     │
│   └─ khodz-db       SQLite (rusqlite, bundled) persistence   │
│ khodz-core      — shared types, errors, units, hashing traits│
└──────────────────────────────────────────────────────────────┘
```

Rule: the UI never touches disks; `src-tauri` never parses filesystems. Every crate below the
engine is usable from a CLI (`khodz-cli`, used heavily by tests) without Tauri.

## 2. Crate responsibilities

| Crate | Responsibility | Key invariants |
|---|---|---|
| `khodz-core` | `Offset`, `ByteLen`, `SectorSize`, `Error`, `CancelToken`, `Progress` | No I/O. |
| `khodz-io` *(implemented)* | `trait BlockSource { fn read_at(&self, off, buf) -> Result<usize>; fn len(); fn sector_size(); }` Implementations: `ImageFile` (raw/dd/img), `WinPhysicalDrive`, `MacRawDisk`, `SliceSource` (partition window), `CachedSource` (bounded LRU of aligned chunks). | **There is no write method anywhere in this crate.** Handles are opened read-only at the OS level. Reads are sector-aligned and chunked (default 1 MiB, max cache 64 MiB). |
| `khodz-device` *(implemented)* | Enumerate physical disks and volumes, model/serial/size/bus/removable, volume→disk mapping. | Windows: probe `\\.\PhysicalDriveN` + `FindFirstVolumeW`, zero-access handles, `IOCTL_STORAGE_QUERY_PROPERTY`, `IOCTL_DISK_GET_DRIVE_GEOMETRY_EX`, `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS`. macOS: `diskutil -plist` (chosen over DiskArbitration/IOKit: no `unsafe`, APFS mapping built in). Separate modules; IOCTL and plist parsers are pure and tested on every OS. |
| `khodz-part` | MBR, EBR chains, GPT (header CRC + backup), lost-partition scan by boot-sector/superblock signatures. | Never trusts a table blindly; reports conflicts. |
| `khodz-fs` | One module per family. Each exposes `probe()`, `open()`, `walk()` (live + deleted entries), `extents(file)`. | Every recovered entry carries a **confidence** and an **extent source** (metadata-exact / metadata-assumed-contiguous / carved). |
| `khodz-carve` | Header/footer signatures + per-format validators (JPEG markers, PNG CRC chunks, ZIP central directory, PDF xref/EOF, MP4 box tree, …). | A carved file is "complete" only if its validator passes end-to-end; otherwise "partial" or rejected. |
| `khodz-recover` | Destination validation, streaming copy, SHA-256 during read and re-read after write, manifest. | Refuses destinations on the source physical disk unless an explicit override is set and logged. Never overwrites existing files (unique-name policy). |
| `khodz-db` | Schema migrations, scans, found items (paged), recoveries, settings. | Item tables are paged; the UI never loads a full result set. |
| `khodz-engine` | Job runner (worker threads), cancellation, device-disconnect handling, progress events at ≤10 Hz. | A job ends in exactly one terminal state: completed / cancelled / failed(reason) / device_lost. |

## 3. Data flow: scan

1. User selects a disk, volume or image → engine opens a `BlockSource` (read-only).
2. `khodz-part` identifies partitions; `khodz-fs::probe` identifies filesystems.
3. **Quick scan**: filesystem metadata walk (MFT, FAT dirs, exFAT dir sets, HFS+ catalog, APFS fs-tree).
4. **Deep scan**: quick scan + linear carving pass across unallocated space (or the whole source).
5. Results stream into SQLite in batches (transactions of ~5k rows); UI pages from the DB.

## 4. Data flow: recovery

1. Validate destination (exists, writable, free space ≥ total, **different physical disk**).
2. For each item: read extents → stream to `dest/<name>.partial` while hashing → fsync → rename.
3. Re-read written file, compare SHA-256. Record result (`verified`, `mismatch`, `partial-source`).
4. Write `khodz-manifest.json` alongside recovered files.

## 5. Platform split

| Concern | Windows | macOS |
|---|---|---|
| Raw access | `\\.\PhysicalDriveN`, `\\.\X:` via `CreateFileW(GENERIC_READ, FILE_SHARE_READ\|FILE_SHARE_WRITE)` | `/dev/rdiskN` `open(O_RDONLY)` |
| Privilege | Administrator (app manifest `requireAdministrator` only for the scan helper, see SECURITY.md) | root via privileged helper, or Full Disk Access; images need neither |
| Enumeration | SetupAPI / IOCTLs (`windows-sys`) | DiskArbitration / IOKit (`core-foundation` FFI) |
| Installer | MSI (WiX) + NSIS via Tauri bundler | `.dmg` / `.app` via Tauri bundler; notarization needs Apple credentials |

Code lives in `#[cfg(target_os = "windows")] mod windows;` and `#[cfg(target_os = "macos")] mod macos;`
inside each platform-dependent crate. No cross-platform abstraction leaks OS handles.

## 6. Dependency policy

Allowed core deps: `serde`, `rusqlite` (bundled), `sha2`, `crc32fast`, `windows-sys`,
`libc`, `core-foundation`, `tauri`, `log`. (Stage 1 implements `Error` traits by hand, so
`thiserror` is not needed.) In use today: `rusqlite`, `serde`, `serde_json`, `tauri`,
`tauri-build`; dev-only `tempfile`. Frontend runtime deps: `react`, `react-dom`, `@tauri-apps/api`. Filesystem parsers are written in-house so behaviour on
damaged media is under our control. Every new dependency must be justified in this file.

## 7. Key decisions

- **SQLite via `rusqlite` with `bundled`** — no system SQLite dependency, identical behaviour on both OSes.
- **Own parsers instead of `ntfs`/`fatfs` crates** — those crates target intact volumes and abort on
  corruption; recovery needs tolerant parsing and access to deleted records.
- **Results in DB, not memory** — deep scans can find millions of candidates.
- **Read-only enforced structurally** — `BlockSource` has no write API; OS handles opened read-only;
  a regression test hashes source images before and after every scan/recovery test.
