# Khodz Recovery

Open-source data recovery for Windows and macOS. Rust engine, Tauri 2 desktop shell, React +
TypeScript + Tailwind UI, SQLite for scan and recovery history.

> **Status: early development (Stage 1 of 9).** The application shell, database and navigation
> work. Drive detection, scanning and recovery are **not implemented yet** — see
> [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md). Do not use it on data you care about yet.

## Safety first

Source drives are opened read-only and are never written to. Recovery to the same physical disk
as the source is blocked by default. Encryption is never bypassed. Details in [SECURITY.md](SECURITY.md).
What each filesystem can and cannot do is in [SUPPORTED_FILESYSTEMS.md](SUPPORTED_FILESYSTEMS.md).

## Repository layout

```
crates/khodz-core   shared types, errors, cancellation, progress (no I/O)
crates/khodz-db     SQLite schema, migrations, repositories
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
