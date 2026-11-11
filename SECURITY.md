# Khodz Recovery — Security & Data-Safety Model

## Non-negotiable guarantees

1. **Source media is never written.** Enforced three ways:
   - The `BlockSource` trait has no write method; no crate that touches a source has a write path.
   - OS handles are opened read-only (`GENERIC_READ` on Windows, `O_RDONLY` on macOS). We never
     lock, dismount, or issue `FSCTL_*` calls that modify volume state.
   - Regression tests hash every fixture image before and after each scan/recovery test.
2. **No recovery to the source physical disk by default.** The destination's volume is resolved to
   its physical disk number (Windows `IOCTL_STORAGE_GET_DEVICE_NUMBER`; macOS DiskArbitration
   whole-disk BSD name) and compared to the source. Override requires an explicit, logged opt-in with
   a warning that recovery may overwrite the very data being recovered.
3. **No encryption bypass.** BitLocker, FileVault/APFS-encrypted and other encrypted volumes are
   detected and reported as "encrypted — unlock with the OS first". We read only what the OS exposes
   after the user unlocks it. No key extraction, no brute force.
4. **No OS security bypass.** Elevation uses the platform mechanism (UAC prompt / macOS
   authorization + privileged helper). We do not disable SIP, TCC, or Defender.

## Device discovery (Stage 2)

Listing drives never opens a device for reading. On Windows every handle is opened with
`dwDesiredAccess = 0` and only `FILE_ANY_ACCESS` IOCTLs are issued; a unit test pins the access
constant at zero. On macOS only `diskutil … -plist` metadata queries run, with device ids
validated as `disk[0-9a-z]+` before being passed to the command. No elevation is requested.
`unsafe` is confined to `crates/khodz-device/src/windows.rs` (FFI), with a SAFETY note per block.

## Read-only storage access (Stage 3)

- Images: `OpenOptions::new().read(true)` only — a test proves files with the OS read-only
  attribute open, i.e. write access is never requested.
- Raw devices: `GENERIC_READ` / `O_RDONLY` only, shared with the OS (never locked or
  dismounted). Only paths matching `\\.\PhysicalDriveN`, `\\.\X:`, `\\?\Volume{…}` or
  `/dev/[r]diskN…` are accepted, so the device opener cannot be pointed at arbitrary files.
- Without elevation the OS refuses raw access and the app reports *access denied*; it never
  tries to elevate itself.
- `unsafe` additionally appears in `crates/khodz-io/src/device/{windows,macos}.rs` for the size
  IOCTLs, each with a SAFETY note.

## Privilege design

- The UI process runs unprivileged.
- Raw device access needs Administrator (Windows) or root/Full Disk Access (macOS). Plan: a small
  elevated scan helper process launched on demand, speaking a narrow IPC protocol (open device
  read-only, read range, enumerate). It accepts only device paths it enumerated itself.
- Disk-image scanning needs no elevation.

## Untrusted input

Every byte read from a disk is attacker-controlled (a crafted USB stick is a realistic threat).
- Parsers use checked arithmetic and bounds-checked slices; no `unsafe` in `khodz-fs`/`khodz-carve`.
- All lengths/counts from on-disk structures are capped (e.g. directory recursion depth, B-tree
  node count, extent count, filename length) to prevent memory exhaustion and infinite loops
  (cycle detection on directory and B-tree traversal).
- Fuzz targets (`cargo fuzz`) for each parser are part of Stage 4/5 exit criteria.
- Previews are rendered from size-capped buffers; images are decoded in Rust or by the webview from
  a blob — never written to disk, never executed. HTML/SVG/scripts are previewed as text only.

## Recovered-file handling

- Output names are sanitised (no path separators, reserved Windows names, control chars) and
  confined to the chosen destination directory (no `..` traversal from on-disk names).
- Existing files are never overwritten.

## Reporting vulnerabilities

Open a private security advisory on the project repository. Do not file public issues for
vulnerabilities in parsers.
