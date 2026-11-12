# Releasing Khodz Recovery

## Building installers locally

Prerequisites: the toolchain in README.md (Rust stable, Node 26, and on Windows the MSVC build
tools). The Tauri CLI downloads WiX and NSIS itself on first use.

```bash
cd app
npm ci
npm run tauri build
```

| Platform | Output (under `target/release/bundle/`) | Notes |
|---|---|---|
| Windows | `msi/Khodz Recovery_<version>_x64_en-US.msi` | per-machine install (asks for administrator) |
| Windows | `nsis/Khodz Recovery_<version>_x64-setup.exe` | per-user install, no administrator prompt |
| macOS | `dmg/Khodz Recovery_<version>_<arch>.dmg`, `macos/Khodz Recovery.app` | build on a Mac; `--target universal-apple-darwin` for one image covering Apple Silicon + Intel |

The Rust crates and the `@tauri-apps/*` npm packages must be on the same minor versions — the
release build refuses to run otherwise. The npm versions are pinned exactly for that reason.

## Releasing through CI

1. Update the version in `Cargo.toml` (`[workspace.package]`), `app/src-tauri/tauri.conf.json`
   and `app/package.json`.
2. Commit, then tag: `git tag v0.2.0 && git push origin v0.2.0`.
3. `.github/workflows/release.yml` runs the full regression suite (`ci.yml`), then builds the
   Windows and macOS installers and attaches them to a **draft, pre-release** GitHub release.
4. Download and smoke-test each installer (see the checklist below), then publish the draft.

Running the workflow manually (Actions → Release → Run workflow) builds the installers as
workflow artifacts without creating a release.

## Code signing

Without signing, installers work but the OS warns:

- **Windows** shows SmartScreen ("Windows protected your PC" → *More info* → *Run anyway*).
- **macOS** Gatekeeper refuses to open an unsigned, un-notarised app downloaded from the internet:
  right-click the app → *Open*, or `xattr -dr com.apple.quarantine "/Applications/Khodz Recovery.app"`.

The release notes state whether each build is signed.

**macOS** (Developer ID + notarisation) — add these repository secrets and the release workflow
signs and notarises automatically:

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | base64 of the exported Developer ID Application `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | password of that `.p12` |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Name (TEAMID)` |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | Apple ID, app-specific password, team ID (notarisation) |

**Windows** (Authenticode) — not wired up yet. It needs a code-signing certificate (an OV/EV
certificate or a cloud signing service) and a `bundle.windows.signCommand` /
`certificateThumbprint` in `tauri.conf.json`.

No signing credentials are stored in the repository.

## Smoke-test checklist (per installer)

- Installs; Start menu / Applications entry appears; app launches with the dark theme.
- Drives page lists the machine's disks.
- Disk Images → open `fixtures/out/mbr-multi.img` (after `fixtures/unpack.ps1`) → three volumes
  readable → quick scan → recover a folder to another drive → "Verified" for every file.
- Uninstalls cleanly; the scan history database in the user's app-data folder is kept.
