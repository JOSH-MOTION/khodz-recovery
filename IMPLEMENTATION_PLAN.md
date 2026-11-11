# Khodz Recovery — Implementation Plan

Status legend: ☐ not started · ◐ in progress · ☑ done **and verified by tests on the named OS**.

## Environment audit (2026-11-11, Windows 11 Pro 26200, x86_64)

Initial audit found an empty workspace and no toolchain. Installed via winget on 2026-11-11:

| Tool | Status |
|---|---|
| Rust | 1.99.1 stable, `x86_64-pc-windows-msvc` (rustup 1.29.3) |
| MSVC Build Tools 2022 (VCTools workload) | Installed, links Rust binaries |
| Node.js / npm | 26.13.0 / 11.20.0 |
| Git | 2.56.0 (per-user install) |
| WebView2 runtime | 157.0.4341.42 |
| WiX / NSIS | Not installed; Tauri CLI downloads them on first bundle (Stage 9) |
| Python | Not installed (not needed) |
| macOS build host | **Not available from this machine.** macOS code, APFS/HFS+ fixtures and `.dmg` packaging must be verified on the user's Mac or a GitHub Actions `macos-14` runner. |

Key library versions: Tauri 2.11, tauri-build 2.6, rusqlite 0.40 (bundled SQLite), React 19.3,
Vite 8.3, Tailwind 4.3, TypeScript 7.0, Vitest 5.1.

## Stages

| # | Stage | Exit criteria | Status |
|---|---|---|---|
| 0 | Audit + docs | This file and the four companion docs exist | ☑ |
| 1 | Foundation | Cargo workspace builds; `khodz-core`, `khodz-db` with migrations + unit tests; Tauri 2 app launches; React shell with sidebar nav (Drives, Disk Images, Results, Recovery, History, Settings), dark/light/system theme, Tailwind tokens; `cargo test` + `npm test` + `tsc --noEmit` green on Windows | ☑ Windows (2026-11-11) · macOS unverified |
| 2 | Device discovery | Lists physical disks + volumes with size/model/bus/removable on Windows; macOS module compiles and is tested in CI; UI drive cards from real data (no mocks) | ☑ Windows (2026-11-11) · macOS: parser unit-tested + cross-compiled, **not yet run on a Mac** |
| 3 | Read-only access + images | `BlockSource` for images, Windows drives, macOS rdisk; bounded cache; cancellation; disconnect → `DeviceLost`; source-unchanged hash test | ☑ Windows images (2026-11-11) · raw-device reads: unelevated denial verified, elevated read **not yet run** · macOS: compiles only |
| 4 | Partitions + FS parsing | MBR/GPT; probe+walk for FAT32, exFAT, NTFS, HFS+, APFS (live files); fixture images per FS | ☐ |
| 5 | Quick + deep scan | Deleted-entry discovery (NTFS, FAT32, exFAT first); carving for JPEG/PNG/GIF/PDF/ZIP-family/MP4/MOV/MP3/WAV with validators; fragmentation honesty tests | ☐ |
| 6 | Results explorer + preview | Virtualised tree/list from paged DB; filters; image/text/hex preview (preview reads ≤ 8 MiB) | ☐ |
| 7 | Recovery + integrity | Destination validation incl. same-disk refusal; streaming copy + SHA-256 verify; manifest; recovery history | ☐ |
| 8 | Partition analysis | Lost partition search, overlapping/conflicting table reporting, scan of found partitions | ☐ |
| 9 | Installers + CI matrix | MSI/NSIS on Windows; `.dmg` on macOS (unsigned unless credentials supplied); GitHub Actions matrix running the full regression suite | ☐ |

Each stage: implement → run tests → fix → update docs → only then continue.

## Stage 3 — delivered

- `crates/khodz-io`: `BlockSource` trait (read-only by construction: no write method),
  `ImageFile` (`OpenOptions::read(true)` only), `RawDevice` for Windows (`GENERIC_READ`, share
  read/write, size via `IOCTL_DISK_GET_LENGTH_INFO`) and macOS (`O_RDONLY`, size via
  `DKIOCGETBLOCK{SIZE,COUNT}`), with device-path validation so arbitrary files can't be opened
  as devices.
- Adapters: `AlignedSource` (unaligned reads over sector-aligned devices; bounce buffer capped
  at 4 MiB), `SliceSource` (partition windows that cannot escape their bounds),
  `CachedSource` (LRU, hard cap — default 64 MiB), `MemSource`.
- Fault tolerance: OS errors classified as *media error* (bad sector → zero-fill, record, keep
  going), *device lost* (abort), *access denied*. 1024 consecutive bad sectors are treated as a
  lost device so a yanked drive can't stall a scan. `for_each_chunk` = bounded-memory pass with
  cancellation and progress; `sha256_source` fingerprints a source and reports unreadable bytes.
- App: Disk Images page — native file picker (`tauri-plugin-dialog`, `dialog:allow-open` only),
  image details, SHA-256 fingerprint with streamed progress (Tauri channel, ≤10 Hz), throughput,
  ETA, Cancel. Background jobs tracked by id in `AppState` with cancel tokens.
- Dev profile now optimizes dependencies (`opt-level = 3`): SHA-256 in an unoptimized build ran
  at 4.6 MiB/s, which made dev builds unrepresentative.

Verified in the real app: a 256 MiB read-only image opened via the native dialog; the app's
SHA-256 matched `Get-FileHash`; Cancel stopped a running fingerprint; the file's hash, mtime and
read-only flag were unchanged afterwards. Throughput after the profile fix: 256 MiB in 4.1 s,
on par with Windows' own `Get-FileHash` (3.9 s) on the same file (was 51.3 s).

Not yet done / limitations:
- **Elevation:** raw drive reads need Administrator (Windows) / root + Full Disk Access (macOS).
  Without it the app reports "access denied" (verified). An elevated helper process is planned
  before scanning live drives (Stage 5); until then live drives can be scanned only by running
  the app elevated, and images need nothing.
- The elevated raw-read test (`raw_device.rs`, `--ignored`) has not been run yet.
- Only raw images; E01/AFF, compressed DMG, VHD/VHDX are not supported.
- Image sector size is assumed 512 until Stage 4 detects it from GPT/filesystem headers.

## Stage 2 — delivered

- `crates/khodz-device`: `list_disks()` → physical disks (display name, model, vendor, serial,
  size, logical/physical sector size, bus, removable, system, virtual, SSD, media present) with
  their volumes (mount points, label, filesystem, size, free, on-disk offset, encryption when
  known, system flag, spanned flag).
- **Windows:** probes `\\.\PhysicalDrive0..63` and `FindFirstVolumeW`; all handles opened with
  **zero access rights** using only `FILE_ANY_ACCESS` IOCTLs, so no elevation is needed and no
  data can be read or written. IOCTL output is parsed from raw bytes by bounds-checked pure
  functions (unit-tested on every OS). Critical-error dialogs suppressed for empty card readers.
- **macOS:** `diskutil list -plist` + `diskutil info -plist` (decision: Apple-supported, gives
  APFS container→physical store mapping, no privileges, no `unsafe`). Synthesized APFS container
  disks are folded into the physical disk holding the store.
- UI: drive cards with bus/system/removable/SSD badges, proportional partition map, per-volume
  usage; refresh button and re-enumeration on window focus (hot-plug). Scan stays disabled.

Verified on this machine against `Get-Disk`/`Get-Volume`: 3 disks (SATA system SSD, USB HDD,
USB stick), all 5 visible volumes plus the hidden EFI partition, sizes exact to the byte.

Known limitations (documented, not hidden):
- BitLocker status is not reported on Windows yet (`encrypted = null`); needs WMI or elevation.
- Partition scheme on Windows is left empty until Stage 4 parses MBR/GPT itself (the OS layout
  IOCTL needs read access).
- macOS fixtures are hand-written from documented `diskutil` keys and must be checked against
  real output on a Mac before macOS is marked verified.
- No push notifications for hot-plug; the list refreshes on Refresh or window focus.

## Stage 1 — delivered

- `crates/khodz-core`: `Error` (maps OS permission errors to `AccessDenied`), `ByteSize`, checked
  `align_up`/`align_down`, `CancelToken`, `Progress` + `ProgressThrottle`.
- `crates/khodz-db`: forward-only migrations on `user_version` (refuses newer schemas), schema v1
  with CHECK constraints on every enum column, scan/recovery lifecycles that can end exactly once,
  startup cleanup that marks jobs interrupted by a crash as `failed`, bounded paging (max 500 rows),
  whitelisted settings keys.
- `app/src-tauri`: thin commands (`app_info`, `get_setting`, `set_setting`, `list_scans`,
  `list_recoveries`) with a stable `{kind, message}` error shape; capability limited to
  `core:default` + `core:window:allow-set-theme` (no fs/shell plugins); strict CSP.
- `app/`: sidebar shell, History page backed by the real DB, Settings (theme persisted to SQLite,
  native title bar follows theme, real About info), honest empty states for unbuilt features.

Decisions: no router or icon library (avoid deps); `arithmetic_side_effects` clippy lint enabled
workspace-wide so future parsers must use checked arithmetic; `unsafe_code = "deny"` workspace-wide
(platform crates will opt out narrowly with justification).

Open product decisions: project **license** (not yet chosen — needed before public release);
bundle **identifier** `org.khodz.recovery` is a placeholder.

## Stage 1 original task list

1. `Cargo.toml` workspace; `rust-toolchain.toml` pinned stable; `clippy` + `rustfmt` config.
2. `crates/khodz-core`: error type, units, `CancelToken`, progress struct. Unit tests.
3. `crates/khodz-db`: rusqlite bundled, migration table, schema v1 (`scans`, `scan_items`,
   `recoveries`, `recovery_items`, `settings`), repository functions. Tests against in-memory and temp-file DBs.
4. `app/` via `create-tauri-app` (React-TS, Vite), Tailwind, `app/src-tauri` wired to the workspace.
5. Commands: `app_info`, `list_scans`, `get_setting`/`set_setting` — backed by the real DB in the OS app-data dir.
6. UI shell: window chrome, sidebar, routes, theme toggle persisted via `settings`, empty states that
   say "No drives detected yet — device discovery arrives in Stage 2" (no fake drives).
7. Frontend tests: Vitest + Testing Library for navigation and theme toggle.
