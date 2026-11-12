# Khodz Recovery

Open-source data recovery for Windows and macOS. Rust engine, Tauri 2 desktop shell, React +
TypeScript + Tailwind UI, SQLite for scan and recovery history.

> **Status: early development (Stage 8 of 9).** Quick and deep scans run from the app on disk
> images and — when run as Administrator — live drives; results can be browsed by volume and
> folder and previewed (image, text, hex) with an honest status per file. Selected files and
> folders can be recovered to another disk; every file is re-read after writing and checked
> against its SHA-256, and a manifest is written. Partition tables are checked against the
> disk, and erased or deleted partitions can be found (including through backup boot sectors)
> and scanned. Verified on Windows; macOS is untested on real hardware and HFS+/APFS are
> detection-only. Installers are still to come — see
> [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md). Not yet recommended for data you cannot
> afford to lose; work from a disk image where possible.

## Safety first

Source drives are opened read-only and are never written to. Recovery to the same physical disk
as the source is blocked by default. Encryption is never bypassed. Details in [SECURITY.md](SECURITY.md).
What each filesystem can and cannot do is in [SUPPORTED_FILESYSTEMS.md](SUPPORTED_FILESYSTEMS.md).

## Repository layout

```
crates/khodz-core   shared types, errors, cancellation, progress (no I/O)
crates/khodz-db     SQLite schema, migrations, repositories
crates/khodz-device drive and volume discovery (Windows IOCTLs / macOS diskutil)
crates/khodz-io     read-only block access: images, raw devices, cache, bad-sector tolerance
crates/khodz-part   MBR/EBR/GPT partition tables
crates/khodz-fs     filesystem detection; FAT/exFAT/NTFS metadata walking (live + deleted)
crates/khodz-carve  signature carving with per-format structural validation
crates/khodz-engine quick/deep scan orchestration
fixtures/windows    generator for Windows-formatted test images (run as Administrator)
app/                React + TypeScript + Tailwind frontend (Vite)
app/src-tauri       Tauri 2 shell (thin IPC layer)
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full design.

## Development

Prerequisites: Rust (stable), Node.js LTS. Windows also needs MSVC Build Tools ("Desktop
development with C++"); macOS needs Xcode Command Line Tools.

```
cd app
npm install
npm run tauri dev      # run the desktop app
```

Tests:

```
cargo test --workspace
cd app && npm run typecheck && npm test
```
