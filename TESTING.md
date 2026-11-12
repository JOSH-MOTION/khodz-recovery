# Khodz Recovery — Testing Strategy

## Recovery quality on a real pen drive (2026-11-12)

A user's recovered videos would not play. Investigated on the actual drive (SanDisk 3.2Gen1,
30.7 GB FAT32, 92% full, read-only, elevated diagnostic `crates/khodz-engine/examples/diagnose.rs`):

| Finding | Before | After |
|---|---|---|
| Deleted videos whose location is entirely taken by newer camera photos | offered as recoverable; "recovered" copies held other files' bytes | 444 deleted files reported **overwritten**, not written |
| `josh boutique` folder deleted by macOS (entries still live, chains freed) | 26 videos "partial", 1 cluster located | located; **11 verified**, and all 11 recovered copies are **byte-identical** to the intact copies elsewhere on the drive |
| Partly overwritten videos | "Deleted" | "damaged — starts about 77% / 88% / 4% in"; real first differences at 73.7% / 88.4% / 3.7% |

| Suite | Result |
|---|---|
| `cargo test -p khodz-carve` (adds MP4/MOV deep check: intact movie verified; overwritten middle found and located at ~50%; zeroed media; missing/truncated index; hostile tables) | 5 unit + 8 + 1 passed |
| `cargo test -p khodz-engine --test deleted_files -- --ignored` (Windows-built FAT32 volume: folder deleted macOS-style → files intact byte for byte; clusters reused → 100% overwritten; PNG written around a foreign cluster → rebuilt and verified byte for byte) | 3 passed |
| `cargo test -p khodz-db` (adds schema v5 columns, constraints, column-count guard) | 30 passed |
| `cargo test -p khodz-recover` (skip reasons and known damage) · `cargo test -p khodz-recovery` (overwritten skipped, damaged flagged, verified layout wins) | 14 · 23 passed |
| `npm test` (labels: verified / damaged / N% overwritten / overwritten; Recover dialog counts overwritten files) | 66 passed |
| Workspace total, Windows + both macOS clippy targets, Rust 1.85 check | 184 passed · clean · builds |

## Continuous integration (Stage 9)

`.github/workflows/ci.yml` runs on every push to `main`, every pull request, and before every
release: frontend typecheck + tests (Linux), and on **Windows and macOS** `cargo fmt --check`,
`cargo clippy --workspace --all-targets -D warnings`, and
`cargo test --workspace -- --include-ignored --skip reads_real_device_consistently` against the
committed fixtures (`fixtures/unpack.ps1` verifies them against `fixtures/SHA256SUMS` first).
Windows also checks that the workspace still builds with Rust 1.85, the declared minimum.
Status: written and YAML-validated; **not yet run**, because the repository has no GitHub
remote.

## Stage 9 checks (Windows 11 x64, 2026-11-12)

| Check | Result |
|---|---|
| `npm run tauri build` | MSI 4.5 MB + NSIS setup.exe 3.2 MB built (after pinning the npm Tauri packages to the crates' versions) |
| MSI administrative extract (`msiexec /a`, nothing installed) | one file, `khodz-recovery.exe`; identical to the release build except Tauri's bundle-type marker (3 bytes, `UNK` → `MSI`); unsigned |
| setup.exe `/S` per-user install into a scratch folder | exit 0; executable + uninstaller, Start-menu shortcut, uninstall entry "Khodz Recovery 0.1.0", publisher set |
| Installed app | launches; Drives lists this PC's disk and volumes; Disk Images → `mbr-multi.img` quick scan completes with PFAT 10 / LNTFS 11 / LEXFAT 10 files |
| Silent uninstall | exit 0; files, shortcut and uninstall entry removed; scan history (`%APPDATA%\org.khodz.recovery\khodz.db`) kept |
| `fixtures/unpack.ps1` in a clean folder | 7/7 files match `SHA256SUMS` |
| `cargo +1.85 check --workspace --all-targets` (declared minimum Rust) | failed at first (let chains need 1.88) → fixed → passes |
| Workflow files | parse as YAML; jobs and triggers as intended |

## Stage 8 additions (Windows 11 x64, 2026-11-12)

| Suite | Result |
|---|---|
| `cargo test -p khodz-engine --test lost_partitions -- --ignored` — damage applied in memory to copies of the Windows-built images: MBR erased; MBR + all three boot sectors erased; boot sectors erased with the table intact (plain scan uses backups); one MBR entry deleted; an entry shrunk and mistyped; GPT wiped like `diskpart clean`; intact MBR and GPT disks. **Every live file of every recovered volume matches the manifest SHA-256** (24 per MBR case, 8 for GPT) | 8 passed |
| `cargo test -p khodz-engine` unit (gap maths, blank disk fully searched with progress, cancellation) | 3 passed |
| `cargo test -p khodz-fs` (adds boot-record probe: NTFS size/backup/MFT-or-mirror check, noise and partial signatures rejected, HFS+ header offsets) | 14 passed |
| `cargo test -p khodz-io` (adds `PatchedSource`: patch only the start, original untouched) | 4 unit + 14 integration passed |
| `cargo test -p khodz-part` (adds valid-but-different primary/backup GPT) | 11 passed |
| `cargo test -p khodz-recovery -- --include-ignored` (adds search + progress + cancel through the command layer, empty/out-of-range volume lists refused without a history row, MBR-erased fixture → found → scanned into history with labels; source unchanged) | 22 passed |
| `npm test` (adds partition check: "matches" state, problems + unpartitioned space, search progress/cancel, found list with pre-selection of readable volumes, scan of only the chosen volumes with their backup-boot locations, nothing-found and cancelled states, administrator hint for drives; existing page tests updated for the new scan argument) | 65 passed |
| Workspace `cargo test --workspace -- --include-ignored` excluding the admin-only device test | 172 passed |
| Real app | image with MBR + NTFS boot sector erased → 3 volumes found (NTFS via backup) → scanned (31 files, same as intact) → NTFS volume recovered: 8/8 match the manifest; source SHA-256 unchanged |

## Stage 7 additions (Windows 11 x64, 2026-11-12)

| Suite | Result |
|---|---|
| `cargo test -p khodz-recover` (name sanitising incl. reserved names/traversal/length, session folder naming + uniqueness; destination checks: missing folder, not enough space, same-disk only for live sources; real path → physical-disk mapping on this PC; writer: fragmented/sparse/resident files verified, never overwrite, erased/partial/short items reported honestly, unreadable sectors zero-filled and flagged, source loss + cancel abort and remove the partial file, progress) | 14 passed |
| `cargo test -p khodz-recover --test fixtures_end_to_end -- --ignored` (**24 live files of 3 Windows-built volumes + carved files recovered; every one matches the original SHA-256**; fragmented/truncated carved files come out as partial; erased files produce no file; source unchanged) | 1 passed |
| `cargo test -p khodz-db` (adds schema v4, recovery items, problems-first ordering, recursive view-aware folder selection) | 28 passed |
| `cargo test -p khodz-recovery -- --include-ignored` (adds whole-volume recovery via the command layer with Unicode names, manifest + history rows, second run → new session folder, same-disk device refusal, empty selection rejected without a history row) | 18 passed |
| `npm test` (adds selection: files, folders, folder coverage, all-in-folder, clear; Recover dialog: exact selection sent, destination required, problems block, same-disk needs explicit override, erased note; Recovery page: empty state, live progress + cancel → report, per-file outcomes, show in folder, start errors) | 59 passed |
| Workspace `cargo test --workspace -- --include-ignored` excluding the admin-only device test | 152 passed |
| Real app (UI Automation + native folder dialog) | quick scan of `mbr-multi.img` → 2 volumes recovered: 16 verified, 5 erased not written; 16/16 re-hashed match the fixture manifest; source SHA-256 unchanged |

## Stage 6 additions (Windows 11 x64, 2026-11-11)

| Suite | Result |
|---|---|
| `cargo test -p khodz-db` (adds tree: per-volume folders/files, `/docs` vs `/docs0` boundary, `%`/`_` in folder names, same path on two volumes, Deleted view hides folders without deleted files, carved scope, stored volumes + fallback) | 26 passed |
| `cargo test -p khodz-engine` (adds `read_item_range`: extents, sparse, windows across boundaries, truncation, partial items, resident) | 4 passed (+3 fixture) |
| `cargo test -p khodz-recovery -- --include-ignored` (adds volumes persisted; **preview bytes of 5 verified carved files and every live NTFS file match the originals' SHA-256**, windowed reads agree) | 15 passed |
| `npm test` (explorer navigation incl. keyboard, carved location, search mode, previews: text/hex/details/erased/image decode failure/partial-image caption/binary fallback/read errors, default location for raw images, preview helpers incl. "SVG/HTML never rendered") | 49 passed |
| Workspace total `cargo test` / fixture tests (`--ignored`, excluding the admin-only device test) | 124 / 8 passed |

## Stage 5 additions (Windows 11 x64, 2026-11-11)

| Suite | Result |
|---|---|
| `cargo test -p khodz-carve --test carving` (spec-built JPEG/PNG/GIF/ZIP/DOCX/PDF/MP4/BMP/WAV in noise; **fragmentation honesty**: 5 formats × noise / zero / other-file gaps; truncation; CRC corruption; GIF LZW round trip; magic-in-noise rejection; skip + cancel; hostile sizes) | 8 passed |
| `cargo test -p khodz-carve --test encoder_fixture -- --ignored` (System.Drawing JPEG/PNG/GIF/BMP, deflate ZIP, Office package, fragmented & truncated copies) | 1 passed: 5 complete byte-exact, BMP unverified, 5 broken copies never complete, image unchanged |
| `cargo test -p khodz-engine -- --include-ignored` (blank media, cancel, sink errors, quick scan of the multi-volume Windows image, no duplicates in deep scan, deep scan of raw media) | 6 passed |
| `cargo test -p khodz-db` (adds schema v2 upgrade from populated v1, item insert/list/filter/search with LIKE escaping, stats, extents round-trip, CHECK rollback, cascade delete, paging) | 22 passed |
| `cargo test -p khodz-recovery -- --include-ignored` (adds scan persistence, cancelled status, no record for unopenable sources, throttled progress) | 14 passed |
| `npm test` (adds scan dialog from Drives/Disk Images, Results page stats/filters/search/progress/cancel/start errors, status labelling) | 37 passed |
| clippy `-D warnings` workspace + 7 engine crates for `{aarch64,x86_64}-apple-darwin` | clean |

Fixture generator (no admin): `fixtures/windows/make-carving-fixture.ps1`.

**Regression that found a real bug:** the fragmentation test showed a GIF with another file
spliced in was reported complete by a block-structure check; GIF validation now runs a full
LZW decode check. The same suite proves zero-filled gaps (TRIM) and noise never yield
"complete" for JPEG, PNG, GIF, ZIP or PDF.

## Stage 4 additions (Windows 11 x64, 2026-11-11)

| Suite | Result |
|---|---|
| `cargo test -p khodz-core` (adds bounds-checked byte readers) | 11 passed |
| `cargo test -p khodz-part` (GUIDs, type names; 10 spec-built tables: MBR/EBR, EBR loop, overlaps, superfloppy, GPT 512/4096, damaged primary → backup, wiped primary + MBR, corrupt entry array, damaged backup, hostile header values) | 12 passed |
| `cargo test -p khodz-fs` unit (DOS/FILETIME, FAT LFN + deleted-name recovery, exFAT entry sets + checksums, NTFS runlists incl. hostile ones, fixups/torn records) | 11 passed |
| `cargo test -p khodz-fs --test windows_fixtures -- --ignored` (Windows-built images) | 1 passed: 4 images, 6 volumes, 48 live files byte-identical, 14 deleted files discovered, images unchanged |
| `cargo test -p khodz-recovery -- --include-ignored` (adds image analysis incl. fixture) | 11 passed |
| `npm test` (adds partition/filesystem layout view) | 29 passed |
| clippy `-D warnings` workspace (overflow lint on all parsers) | clean |

**Fixtures:** `fixtures/windows/make-fixtures.ps1` (run once as Administrator) uses diskpart and
Windows' own FAT32/exFAT/NTFS formatters on VHDs mounted into folders (no drive letters), writes
deterministic files, deletes some, detaches and strips the VHD footer. `fixtures/out/manifest.json`
records Windows' view of partitions and every file's SHA-256. Images are git-ignored and marked
read-only. Tests needing them are `#[ignore]`d so a fresh checkout's `cargo test` passes.

Not yet: FAT12/16 and HFS+/APFS fixtures; fuzzing.

## Stage 3 additions (Windows 11 x64, 2026-11-11)

| Suite | Result |
|---|---|
| `cargo test -p khodz-io` unit (error classification, device-path validation) | 3 passed |
| `cargo test -p khodz-io --test image_and_adapters` | 14 passed |
| `cargo test -p khodz-io --test raw_device` | 2 passed, 1 ignored (needs elevation) |
| …same, elevated, `--ignored`, `KHODZ_TEST_DEVICE=\\.\PhysicalDrive2` (USB stick) | 1 passed: 30,765,219,840 B, 512 B sectors, boot signature, aligned/unaligned consistency |
| `cargo test -p khodz-recovery` (adds image inspect/fingerprint/cancel, job table) | 9 passed |
| `npm test` (adds Disk Images page) | 27 passed |
| clippy `-D warnings` on Windows + `{aarch64,x86_64}-apple-darwin` (io/device/core) | clean |

What the I/O suite proves (synthetic media, generated per test, never committed):
- **Source unchanged:** SHA-256, mtime and size of the image file identical before/after 500
  random reads, a full chunked pass and a fingerprint.
- **Never requests write access:** a file with the OS read-only attribute opens and reads.
- **Exact reads:** 500 random offset/length reads incl. across and past EOF; 8 threads in parallel.
- **Alignment:** 2,000 random unaligned reads through `AlignedSource` over a source that panics on
  any unaligned access, at 512 and 4096-byte sectors.
- **Bad sectors:** zero-filled, reported as merged ranges, good sectors intact; fingerprints
  report unreadable bytes. **Disconnect:** pass aborts with `DeviceLost` at the right offset;
  a run of 1024+ failures is also treated as device loss.
- **Cancellation** between chunks; **progress** monotonic and complete; **cache** never exceeds
  its bound and serves repeat reads without touching the source.

Run the elevated raw-device test (prompts for admin):
`KHODZ_TEST_DEVICE=\\.\PhysicalDrive2 cargo test -p khodz-io --test raw_device -- --ignored`

## Results through Stage 2, Windows 11 x64, 2026-11-11

| Suite | Result |
|---|---|
| `cargo test -p khodz-core` | 9 passed |
| `cargo test -p khodz-db` | 18 passed |
| `cargo test -p khodz-device` (unit: IOCTL parsers, volume assembly, macOS plist parser) | 18 passed |
| `cargo test -p khodz-device --test real_hardware` (this PC's real disks, not elevated) | 3 passed |
| `cargo test -p khodz-recovery` (Tauri commands) | 4 passed |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo clippy -p khodz-device -p khodz-core --target {aarch64,x86_64}-apple-darwin` | clean (compile only) |
| `cargo fmt --all --check` | clean |
| `npm run typecheck` | clean |
| `npm test` (Vitest) | 20 passed |
| Real app via Windows UI Automation | Drives page lists all 3 real disks and their volumes, matching `Get-Disk`/`Get-Volume`; both themes checked |

Not yet run: anything *on* macOS (needs a Mac or CI runner; `khodz-db` cannot be cross-checked
from Windows because bundled SQLite needs Apple's C toolchain); any disk-reading or recovery
test (no I/O engine exists yet).

Manual device check: `cargo run -p khodz-device --example list_devices`. The frontend tests
mock only the IPC boundary (`src/lib/api.ts`), never scan or recovery data in production code.

## Layers

| Layer | Tooling | Scope |
|---|---|---|
| Unit | `cargo test` | Parsers on byte fixtures, validators, DB repositories, destination rules |
| Integration | `cargo test -p khodz-engine --test '*'` | Full scan → recover on fixture images via `khodz-cli`/engine API |
| Frontend | Vitest + Testing Library, `tsc --noEmit` | Components, navigation, state |
| E2E (later) | `tauri-driver` + WebdriverIO (Windows); manual on macOS | Smoke flows |
| Fuzz | `cargo fuzz` | Every on-disk structure parser |

## Fixture images

Generated by scripts in `fixtures/` and **never committed as binaries** (only scripts + expected
manifests with SHA-256 of each planted file):

| FS | Generator | Where it runs |
|---|---|---|
| FAT32, exFAT, NTFS | `mkfs.fat`, `mkfs.exfat`, `mkntfs` + `mtools`/loop mounts | Linux CI runner |
| FAT32, exFAT, NTFS | `diskpart` VHD create/format (admin) | Windows (optional local) |
| HFS+, APFS | `hdiutil create -fs HFS+/APFS` | macOS CI runner |
| Partition tables | In-repo Rust builder writing MBR/GPT into raw files | Anywhere |
| Carving corpora | In-repo Rust builder concatenating known files, fragmenting some deliberately | Anywhere |

Each fixture script: create image → write known files → delete a subset → unmount → record
manifest (paths, sizes, hashes, which were deleted, which were fragmented).

## Mandatory regression assertions

1. **Source unchanged** — SHA-256 of the image before == after every scan and recovery.
2. **Deleted discovery** — every planted-and-deleted file is found where SUPPORTED_FILESYSTEMS.md
   says it should be.
3. **Content correctness** — recovered files byte-match the manifest hash.
4. **Fragmentation honesty** — deliberately fragmented deleted files are never reported as
   "complete/verified".
5. **Invalid rejection** — corrupt/truncated carving candidates are rejected or labelled partial.
6. **Destination validation** — same-disk destination refused; insufficient space refused;
   existing files not overwritten; path traversal names sanitised.
7. **Cross-platform** — the suite runs on `windows-latest` and `macos-14` in GitHub Actions.

## Running (once Stage 1 lands)

```
cargo test --workspace
cd app && npm test && npx tsc --noEmit
```
