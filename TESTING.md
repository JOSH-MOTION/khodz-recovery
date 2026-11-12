# Khodz Recovery — Testing Strategy

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
