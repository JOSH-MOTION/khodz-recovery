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
crates/khodz-engine quick/deep scan orchestration, partition analysis, lost-partition search
crates/khodz-recover destination checks, verified writing, output naming
fixtures/           committed Windows-built test images (zip + SHA256SUMS) and their generators
app/                React + TypeScript + Tailwind frontend (Vite)
app/src-tauri       Tauri 2 shell (thin IPC layer)
.github/workflows   CI (Windows + macOS regression suite) and release (installers)
```

See [ARCHITECTURE.md](ARCHITECTURE.md) for the full design.

## Development

Prerequisites: Rust (stable; 1.85 minimum), Node.js 26. Windows also needs MSVC Build Tools
("Desktop development with C++"); macOS needs Xcode Command Line Tools.

```bash
cd app
npm ci
npm run tauri dev      # run the desktop app
```

Tests (what CI runs on Windows and macOS):

```bash
pwsh fixtures/unpack.ps1    # once: expands and verifies the test images into fixtures/out
cargo test --workspace -- --include-ignored --skip reads_real_device_consistently
cd app && npm run typecheck && npm test
```

Installers: `cd app && npm run tauri build` (MSI + setup.exe on Windows, .app + .dmg on macOS).
See [RELEASING.md](RELEASING.md) for releases and code signing.

## Installing

Download the installer for your platform from the project's GitHub releases. Installers are
currently **unsigned**: Windows SmartScreen and macOS Gatekeeper will warn before the first
launch ([RELEASING.md](RELEASING.md) explains how to proceed). To read a live drive, run Khodz
Recovery as administrator (Windows) or grant Full Disk Access / run as root (macOS); disk
images need no special rights.

## License

Not chosen yet — a license must be added before the first public release.
