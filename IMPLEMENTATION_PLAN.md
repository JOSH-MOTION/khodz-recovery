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
| 3 | Read-only access + images | `BlockSource` for images, Windows drives, macOS rdisk; bounded cache; cancellation; disconnect → `DeviceLost`; source-unchanged hash test | ☑ Windows images + raw devices (2026-11-11; elevated read verified on a USB stick, unelevated denial verified) · macOS: compiles only |
| 4 | Partitions + FS parsing | MBR/GPT; probe+walk for FAT32, exFAT, NTFS, HFS+, APFS (live files); fixture images per FS | ◐ MBR/GPT + FAT32/exFAT/NTFS ☑ verified on Windows-built images (2026-11-11) · HFS+/APFS detection only — walking needs a Mac for fixtures |
| 5 | Quick + deep scan | Deleted-entry discovery (NTFS, FAT32, exFAT first); carving for JPEG/PNG/GIF/PDF/ZIP-family/MP4/MOV/MP3/WAV with validators; fragmentation honesty tests | ☑ Windows, images (2026-11-11) · live-drive scans need the app run as Administrator (helper not built) · MP3 not carved |
| 6 | Results explorer + preview | Virtualised tree/list from paged DB; filters; image/text/hex preview (preview reads ≤ 8 MiB) | ☑ Windows (2026-11-11) · paged rather than virtualised lists · image previews read ≤ 32 MiB |
| 7 | Recovery + integrity | Destination validation incl. same-disk refusal; streaming copy + SHA-256 verify; manifest; recovery history | ☑ Windows, images (2026-11-12) · same-disk detection for live drives verified on this PC's disks · macOS path compiles only |
| 8 | Partition analysis | Lost partition search, overlapping/conflicting table reporting, scan of found partitions | ☑ Windows, images (2026-11-12) · live drives need Administrator · HFS+/APFS found but not walkable |
| 9 | Installers + CI matrix | MSI/NSIS on Windows; `.dmg` on macOS (unsigned unless credentials supplied); GitHub Actions matrix running the full regression suite | ◐ Windows MSI + setup.exe built and verified (2026-11-12) · CI and release workflows written and syntax-checked but **not yet run** (no GitHub remote) · macOS `.dmg` builds only in CI, so unverified |

Each stage: implement → run tests → fix → update docs → only then continue.

## Stage 9 — delivered on Windows; CI pending a GitHub remote

- Bundling (`tauri.conf.json`): Windows MSI (per-machine, WiX) and NSIS setup.exe (per-user,
  no administrator prompt; WebView2 bootstrapper if missing); macOS `.app` + `.dmg`
  (minimum macOS 11). Publisher, copyright and descriptions set. No license field: the
  project license is still an open decision.
- Release blocker found and fixed: the release build refuses mismatched Tauri versions, and
  the npm packages (api 2.12, plugin-opener 2.7, plugin-dialog 2.8) were ahead of the Rust
  crates (2.11 / 2.5 / 2.7). Raising the Rust crates would have required raising the
  workspace's minimum Rust version, so the npm packages are now pinned exactly to the crates'
  versions instead.
- MSRV bug found and fixed: the workspace declared Rust 1.85 but did not build with it (two
  `let` chains, stable only since 1.88). Rewritten; `cargo +1.85 check --workspace
  --all-targets` now passes and CI keeps checking it.
- Fixtures: the exact Windows-built images verified locally are committed as
  `fixtures/windows-built-fixtures.zip` (9.9 MB) with `fixtures/SHA256SUMS`;
  `fixtures/unpack.ps1` expands and verifies them on Windows or macOS. Regenerating them on a
  CI runner could legitimately differ (TRIM), which would make tests environment-dependent.
- `.github/workflows/ci.yml`: frontend (typecheck + tests) on Linux; Rust on Windows and macOS
  — fmt, clippy `-D warnings`, a Rust 1.85 (MSRV) build check, and the whole test suite
  including the fixture tests (only the admin-only raw-device test is skipped).
- `.github/workflows/release.yml`: on a `v*` tag, runs the CI suite, then builds Windows
  (MSI + NSIS) and macOS universal (`.dmg`) installers into a draft pre-release. macOS signing
  and notarisation switch on only when the Apple secrets exist; Windows Authenticode is not
  wired up. Release notes state whether each build is signed.
- RELEASING.md: local builds, release steps, signing, smoke-test checklist.

Verified on this PC: `npm run tauri build` → `Khodz Recovery_0.1.0_x64_en-US.msi` (4.5 MB) and
`…_x64-setup.exe` (3.2 MB). MSI administrative extract contains the release executable (byte-
identical except Tauri's 3-byte bundle-type marker). setup.exe silent per-user install → files,
Start-menu shortcut and uninstall entry (publisher, version) present → the installed app
launches, lists this PC's drive, and scans `mbr-multi.img` with the expected per-volume counts →
silent uninstall removes the files, shortcut and entry and keeps the scan history.

Not verified: the workflows themselves (they need the repository on GitHub), the macOS build
and `.dmg`, and the macOS-specific tests (`khodz-device`, `khodz-recover` disk mapping), which
have only been cross-compiled — the first CI run will be their first run on a Mac.

## Stage 8 — delivered

- `khodz-fs::probe`: recognises FAT12/16/32, exFAT and NTFS boot sectors, HFS+/HFSX volume
  headers and APFS container superblocks; knows each format's volume size and backup location;
  verifies the structure a boot record points to (FAT media bytes, exFAT FAT entry 0, NTFS
  `FILE` record at the MFT or its mirror) so random data is not mistaken for a volume.
- `khodz-io::PatchedSource`: read-only view with the first sector replaced in memory — opens a
  volume through its backup boot sector without touching the medium.
- `khodz-part`: valid-but-different primary and backup GPTs are now reported.
- `khodz-engine::partitions`:
  - `inspect_layout`: for every table entry, what is really there; problems in plain words
    (no filesystem, damaged boot sector, filesystem larger than its entry, type byte not
    matching the filesystem, overlaps, entries past the end); partition types that hold no
    filesystem by design (Microsoft reserved, LDM, swap…) are not flagged; unpartitioned space.
  - Backup boot sectors are used automatically, so a normal scan now reads volumes whose
    primary boot sector is destroyed.
  - `search_lost_partitions`: reads only space no readable volume covers, in 4 MiB chunks at
    512-byte granularity, tolerating bad sectors, cancellable with progress; accepts a boot
    record as a primary or as a backup copy whose primary is gone; skips over volumes it finds;
    notes overlaps with the current table and volumes truncated by the end of the disk.
  - `scan_volumes`: scan an explicit list of volumes (found partitions).
- App: `inspect_partitions`, `search_partitions` (job with progress and cancel), `start_scan`
  with optional volumes. UI: "Partition check" on Disk Images and a "Partitions…" dialog per
  drive — problems list, lost-partition search with progress, found volumes with what can be
  done with each, and "Scan N partitions…" through the normal scan dialog.

Verified end to end on copies of the Windows-built images, damaged the way real disks are:
MBR erased; MBR and every boot sector erased (all three volumes recovered through backups);
boot sectors erased with the table intact; one entry deleted; an entry shrunk and mistyped;
a GPT disk wiped like `diskpart clean`; intact disks (no false alarms, nothing "found"). In
each recovery case every live file matches the manifest SHA-256. In the real app: an image
with its MBR and NTFS boot sector erased → table problem shown → search finds PFAT, LNTFS (via
its backup boot sector) and LEXFAT → scan lists the same 31 files as the intact disk →
recovering the NTFS volume gives 8/8 files matching the manifest; source hash unchanged.

Known gaps: partitions with no surviving boot record at all are not found (deep-scan carving
still works there); HFS+/APFS backup headers are not used; partitions are never rewritten into
the table (by design).

## Stage 7 — delivered

- `crates/khodz-recover`:
  - **Destination check**: the folder must exist and pass a create/delete write probe, and free
    space must be ≥ the bytes to write + 64 MiB. For live-drive sources the destination's
    physical disks are compared with the source's (Windows: volume → disk extents IOCTL;
    macOS: `diskutil info -plist`, incl. APFS physical stores). Same disk → refused unless the
    user ticks an explicit override, which is stored with the recovery. If the disks cannot be
    determined for a live source, recovery is refused rather than guessed.
  - **Writer**: every run goes into a new `Khodz Recovery <date time>` folder, grouped per
    volume (`Label (FS)`) and `Found by signature`. Names are sanitised per component
    (reserved names, forbidden/control chars, trailing dots, 120-char cap) and can never escape
    the folder. Files are created with `create_new` (" (n)" suffix on clashes, never
    overwrite), streamed in 1 MiB chunks with SHA-256, `fsync`ed, given the original modified
    time, then **re-read from the destination and compared**.
  - **Outcomes**: *verified*, *partial_source* (written, but the source data was known
    incomplete or had unreadable sectors — zero-filled and stated), *mismatch* (read-back
    differs) and *failed* (erased data is never written as a file of zeros). Device loss or
    cancel stops the run and removes the half-written file; a full disk stops it with a reason.
- `khodz-db` schema v4: recovery counts for partial/mismatch, same-disk override flag, per-file
  `recovery_items` (output path, size, SHA-256, outcome, note) indexed by recovery; recursive,
  view-aware folder selection.
- App: `check_recovery`, `start_recovery` (progress channel, cancellable), `get_recovery`,
  `list_recovery_items`. The source is re-opened and refused if its size changed since the
  scan. `khodz-manifest.json` (per-file SHA-256 + outcome, summary, override flag) is written
  next to the files.
- UI: checkboxes on files and folders (a ticked folder covers everything under it in the
  current view), "All in folder", a selection bar, the Recover dialog (summary, erased-file
  note, folder picker, live destination check, same-disk warning with explicit override), the
  Recovery page (live progress + cancel, report with per-file results and notes, "Show in
  folder"), and History showing incomplete and failed counts.

Verified end to end: the fixture test recovers all 24 live files of the three Windows-built
volumes plus the carved files; each matches its original SHA-256, erased files produce no
output and the source image is byte-identical afterwards. In the real app: a quick scan of
`mbr-multi.img`, two whole volumes ticked, recovery through the native folder picker → 16
files verified (independently re-hashed against the fixture manifest: 16/16 match), 5 erased
files reported as not recovered, manifest written, source hash unchanged.

Known gaps: a file that was not recovered is listed by its source path, without its volume.
Recovering from a live drive needs the app running as Administrator, like scanning.

## Stage 6 — delivered

- `khodz-db` schema v3: `scan_volumes` (filesystem, label, counts, per-volume error) stored at
  the end of each scan; older scans fall back to grouping their items. Tree queries list the
  sub-folders (with recursive file / deleted counts) and files of any folder per volume or for
  signature results. They use path *range* conditions (`'/a/' ≤ path < '/a0'`) so the
  `(scan_id, path)` index is used and `%`/`_` in real folder names need no escaping.
- `khodz-engine::read_item_range`: reads any byte window of an item through its extents
  (sparse runs → zeros, resident data, stops where a partial item's known extents end).
- App: `read_item_data` returns raw bytes over IPC (≤ 32 MiB) after re-opening the source
  read-only and refusing if its size changed since the scan; `list_volumes`, `list_folder`.
- Explorer: Locations (volumes + "Found by signature"), breadcrumb, folders with counts, files
  with status, keyboard navigation (↑/↓, Enter, Backspace), paging; opens wherever files are.
  Search switches to a flat list across all folders.
- Preview pane: images through the webview decoder (decode failure reported as likely damage;
  for non-verified files the caption says missing parts render blank rather than claiming
  success), text (UTF-8, first 256 KiB), hex (first 16 KiB), details (status, location
  quality explained, timestamps, on-disk extents). Erased data gets a plain explanation.
  SVG/HTML are shown as text only — recovered content is never rendered as markup or executed.
- Access-denied on drive scans now tells the user how to proceed (run as administrator, or use
  a disk image).

Verified end to end: scanning the fixtures and reading every verified carved file and every
live NTFS file back through the preview path reproduces the original SHA-256 (plus windowed
reads). In the real app: a carved 800 × 600 JPEG previews correctly; a fragmented one shows the
partial warning and the honest caption.

Bugs found while verifying: carved names were truncated at exactly the part that distinguishes
them (names now wrap); the explorer opened an empty "Unknown volume" for raw images and, due
to a load-order race, could stay there (now opens where files are; regression test added);
"decoded successfully" over-claimed for partial images (fixed, regression test added).

## Stage 5 — delivered

- `crates/khodz-carve`: sector-aligned signature carving with structural validators —
  JPEG (marker chain, clean entropy data, zero-run detection), PNG (every chunk CRC), GIF
  (block chain **and full LZW decode check: valid codes, exact width × height pixels**), PDF
  (`startxref` must point at `xref` or an xref-stream object; incremental updates), ZIP/DOCX/
  XLSX/PPTX (local headers ↔ central directory ↔ end record), MP4/MOV/HEIC (box chain),
  BMP/WAV/AVI/WebP (header + declared length). Statuses: **Complete** (verified end to end),
  **Unverified** (payload not checkable: BMP, RIFF, MP4/MOV/HEIC), **Partial** (with reason).
  Complete files are skipped over; hostile sizes are capped per format.
- `crates/khodz-engine`: quick scan = partition table → each volume → metadata walk; deep
  scan = quick + carving of the whole source, skipping offsets explained by metadata so files
  are not listed twice. Deleted items whose data reads back as zeros are marked **erased**.
  Volume errors are recorded and the scan continues; device loss/cancel stop it.
- `crates/khodz-db` schema v2 (in-place migration, tested from a populated v1 database):
  scan-item columns for volume offset, name completeness, system flag, data state, carve
  status, note, resident data; batched transactional inserts; filtered, LIKE-escaped, paged
  listing; per-scan stats.
- App: scan jobs with read-only open, results streamed to SQLite in 2,000-item transactions,
  throttled progress (≤10 Hz), cancel, and honest final status (completed / cancelled /
  device lost / failed, with partial results kept). UI: Quick/Deep choice dialog from Drives
  and Disk Images, live progress with phase and cancel, Results page with stats, All / Deleted /
  Found-by-signature views, search, paging and a plain-language status per item.

Verified in the real app: deep scan of the real-encoder image → 12 carved, 6 verified, BMP
unverified, every broken copy partial; quick scan of the Windows-built multi-volume disk → all
7 deleted files across FAT32/NTFS/exFAT, each correctly labelled "Deleted · erased" (TRIM).

Real-world bug caught by the regression suite: a GIF with another file spliced into its middle
passed a block-structure-only check; GIF validation now decodes LZW (see SUPPORTED_FILESYSTEMS.md).

Limitations: scanning a live drive needs the app to run as Administrator (an elevated helper
is still planned); MP3/OLE (old .doc/.xls)/SQLite/RAW photo formats are not carved; a JPEG
fragment gap filled with another JPEG's image data cannot be detected without decoding; carving
assumes files start on a sector boundary.

## Stage 4 — delivered (partially; see status)

- `crates/khodz-core::bytes`: bounds-checked LE/BE readers and UTF-16 decoding used by every parser.
- `crates/khodz-part`: MBR + EBR chain (loop detection, 128-entry cap), GPT at 512/4096-byte
  sectors with header and entry-array CRC32 checks, backup-header fallback and reporting,
  superfloppy detection, overlap / past-end warnings. Partitions are never silently dropped.
- `crates/khodz-fs`: `detect()` for FAT12/16/32, exFAT, NTFS, HFS+/HFSX, APFS, BitLocker, ext;
  `open()` + `walk()` for FAT12/16/32, exFAT and NTFS, reporting live **and** deleted entries
  with extents and an `ExtentQuality` (exact / contiguity assumed / partial).
  - FAT: LFN assembly with checksum; deleted LFN names recovered by brute-forcing the lost
    first byte against the checksum; FAT32 deleted high-word reconstruction; deleted folders
    followed only if their `.`/`..` entries validate.
  - exFAT: entry sets with checksum (including deleted sets, with in-use bits restored),
    NoFatChain contiguity.
  - NTFS: full MFT scan with fixups, runlist decoding (negative/sparse runs), path rebuild via
    parent references with sequence checks and `/$Orphan`, `$Extend\$Deleted` pending deletes.
- Fixture generator `fixtures/windows/make-fixtures.ps1` (needs one admin prompt) builds 4 images
  with Windows' own diskpart/format: MBR+FAT32, GPT+exFAT, GPT+NTFS, MBR with extended partition
  holding NTFS + exFAT; Unicode names, empty and fragmented files, deleted files and a deleted folder.
- App: opening an image now shows its partition table, a partition map and each volume's
  filesystem, label, cluster size and an honest status (Readable / Detected only / Encrypted).
- Tool: `cargo run -p khodz-fs --example walk -- <image>` lists every entry.

Verified: all 4 Windows-built images — partition layouts, disk signatures/GUIDs, volume labels,
48 live files byte-for-byte (SHA-256) including fragmented ones, 14 deleted files discovered,
images unchanged. In the app, `mbr-multi.img` shows the 3 volumes and the disk signature Windows
reported (E0BA3D87).

Real-world findings recorded in SUPPORTED_FILESYSTEMS.md: TRIM zeroes deleted data on the
fixture VHDs (as on SSDs); NTFS pending deletes in `$Extend\$Deleted`; FAT32 high-word clearing.

Remaining for Stage 4: HFS+ catalog and APFS object-map/fs-tree walkers (+ macOS fixtures via
`hdiutil`), FAT12/16 fixtures, NTFS `$ATTRIBUTE_LIST` following and `$MFTMirr` fallback, and
fuzz targets for every parser (planned exit criterion, not done yet).

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
- Elevated raw read verified once, manually, on `\\.\PhysicalDrive2` (USB SanDisk 3.2Gen1,
  FAT32): size 30,765,219,840 B (matches `Get-Disk`), 512 B sectors, 0x55AA boot signature,
  unaligned reads consistent with aligned ones, EOF handled. It is `#[ignore]`d in normal runs
  because it needs Administrator and a chosen device.
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
