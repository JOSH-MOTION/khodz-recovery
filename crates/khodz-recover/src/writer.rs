//! Streaming, verified writing of recovered files.

use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sha2::{Digest, Sha256};

use khodz_core::{CancelToken, Error, Result};
use khodz_engine::AbsExtent;
use khodz_io::{BadRanges, BlockSource, read_tolerant};

const CHUNK: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq)]
pub struct RecoverItem {
    pub id: i64,
    /// Sanitised path relative to the session folder (see `output_relative_path`).
    pub rel_path: PathBuf,
    pub size: u64,
    pub extents: Vec<AbsExtent>,
    pub resident: Option<Vec<u8>>,
    /// The scan only partly knows where this file is (extent quality partial / carved partial).
    pub location_partial: bool,
    /// Nothing of the file is left (erased to zeros, or entirely overwritten by other files):
    /// it is reported as not recovered, with this reason, and nothing is written.
    pub skip_reason: Option<String>,
    /// The scan found the data damaged (part overwritten, or failing its format's checks): it
    /// is written, but reported as incomplete with this reason.
    pub damage: Option<String>,
    pub modified_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Written, and the read-back SHA-256 equals the hash of the data read from the source.
    Verified,
    /// The read-back hash differs from what was written (destination problem).
    Mismatch,
    /// Written and verified, but the source data is incomplete (unreadable sectors, location
    /// only partly known) — the file may be damaged.
    PartialSource,
    Failed,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Verified => "verified",
            Outcome::Mismatch => "mismatch",
            Outcome::PartialSource => "partial_source",
            Outcome::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ItemResult {
    pub id: i64,
    /// Relative to the session folder; None if nothing was written.
    pub output: Option<PathBuf>,
    pub bytes_written: u64,
    pub sha256: Option<String>,
    pub outcome: Outcome,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecoveryProgress {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub current: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RecoverySummary {
    pub files_total: u64,
    pub verified: u64,
    pub mismatched: u64,
    pub partial: u64,
    pub failed: u64,
    pub bytes_written: u64,
    /// Set when the run stopped early for a reason other than cancellation (e.g. disk full).
    pub stopped: Option<String>,
}

/// Create a new, uniquely named session folder inside `dest` (never reuses an existing one).
pub fn create_session_dir(dest: &Path, now: SystemTime) -> Result<PathBuf> {
    let base = crate::names::session_dir_name(now);
    for i in 0..1000u32 {
        let name = if i == 0 { base.clone() } else { format!("{base} ({i})") };
        let p = dest.join(name);
        match std::fs::create_dir(&p) {
            Ok(()) => return Ok(p),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(Error::InvalidData("could not create a unique session folder".into()))
}

/// Create `dir/name`, or `dir/name (1).ext` … if taken. `create_new` guarantees no overwrite,
/// even if another process creates a file at the same moment.
fn create_unique(dir: &Path, name: &str) -> Result<(File, PathBuf)> {
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    for i in 0..10_000u32 {
        let candidate = if i == 0 { name.to_string() } else { format!("{stem} ({i}){ext}") };
        let p = dir.join(&candidate);
        match OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(f) => return Ok((f, p)),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Err(Error::InvalidData(format!("no free file name for {name}")))
}

fn is_disk_full(e: &std::io::Error) -> bool {
    // ERROR_DISK_FULL / ERROR_HANDLE_DISK_FULL on Windows, ENOSPC / EDQUOT on Unix.
    matches!(e.raw_os_error(), Some(112) | Some(39)) && cfg!(windows)
        || matches!(e.raw_os_error(), Some(28) | Some(69)) && cfg!(unix)
        || e.kind() == ErrorKind::StorageFull
}

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

struct Written {
    sha256: String,
    bytes: u64,
    unreadable: u64,
}

/// Stream an item's bytes from the source into `out`, hashing as we go. Unreadable sectors
/// are written as zeros and counted.
fn stream_item<S: BlockSource + ?Sized>(
    src: &S,
    item: &RecoverItem,
    out: &mut File,
    cancel: &CancelToken,
    on_bytes: &mut dyn FnMut(u64),
) -> std::result::Result<Written, WriteError> {
    let mut hasher = Sha256::new();
    let mut written = 0u64;
    let mut bad = BadRanges::default();
    let mut put = |data: &[u8], written: &mut u64| -> std::result::Result<(), WriteError> {
        out.write_all(data).map_err(WriteError::Dest)?;
        hasher.update(data);
        *written = written.saturating_add(data.len() as u64);
        on_bytes(data.len() as u64);
        Ok(())
    };
    if let Some(r) = &item.resident {
        let n = usize::try_from(item.size).unwrap_or(usize::MAX).min(r.len());
        put(&r[..n], &mut written)?;
    } else {
        let mut buf = vec![0u8; CHUNK];
        'extents: for ext in &item.extents {
            let mut done = 0u64;
            while done < ext.len {
                cancel.check().map_err(WriteError::Source)?;
                let remaining_file = item.size.saturating_sub(written);
                if remaining_file == 0 {
                    break 'extents;
                }
                let n = usize::try_from(ext.len.saturating_sub(done).min(remaining_file))
                    .unwrap_or(CHUNK)
                    .min(CHUNK);
                let slot = &mut buf[..n];
                match ext.offset {
                    None => slot.fill(0),
                    Some(off) => {
                        let got = read_tolerant(src, off.saturating_add(done), slot, &mut bad)
                            .map_err(WriteError::Source)?;
                        if got < n {
                            // Source ended early: write what exists and stop.
                            put(&slot[..got], &mut written)?;
                            break 'extents;
                        }
                    }
                }
                put(slot, &mut written)?;
                done = done.saturating_add(n as u64);
            }
        }
    }
    out.flush().map_err(WriteError::Dest)?;
    out.sync_all().map_err(WriteError::Dest)?;
    Ok(Written { sha256: hex(&hasher.finalize()), bytes: written, unreadable: bad.total_bytes() })
}

enum WriteError {
    /// Error reading the source (device lost, cancelled, …): aborts the whole run.
    Source(Error),
    /// Error writing the destination: fails this file (and the run if the disk is full).
    Dest(std::io::Error),
}

fn read_back_sha256(path: &Path) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// Recover `items` from `src` into `session` (an empty folder from [`create_session_dir`]).
/// Each item yields exactly one [`ItemResult`]. Source errors (device lost) and cancellation
/// abort the run after removing the partly written file; destination errors fail the item, and
/// a full destination stops the run with `stopped` set.
pub fn recover<S: BlockSource + ?Sized>(
    src: &S,
    items: &[RecoverItem],
    session: &Path,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(&RecoveryProgress),
    on_item: &mut dyn FnMut(&ItemResult) -> Result<()>,
) -> Result<RecoverySummary> {
    let mut s = RecoverySummary { files_total: items.len() as u64, ..RecoverySummary::default() };
    let bytes_total: u64 =
        items.iter().filter(|i| i.skip_reason.is_none()).map(|i| i.size).fold(0, u64::saturating_add);
    let mut p = RecoveryProgress {
        files_done: 0,
        files_total: s.files_total,
        bytes_done: 0,
        bytes_total,
        current: None,
    };

    for item in items {
        cancel.check()?;
        p.current = Some(item.rel_path.display().to_string());
        on_progress(&p);
        let result = recover_one(src, item, session, cancel, &mut |n| {
            p.bytes_done = p.bytes_done.saturating_add(n);
        });
        let r = match result {
            Ok(r) => r,
            Err(Abort::Source(e)) => return Err(e),
            Err(Abort::DiskFull(r)) => {
                s.failed = s.failed.saturating_add(1);
                on_item(&r)?;
                s.stopped = Some("The destination is full. Free some space or choose another folder.".into());
                p.files_done = p.files_done.saturating_add(1);
                on_progress(&p);
                return Ok(s);
            }
        };
        match r.outcome {
            Outcome::Verified => s.verified = s.verified.saturating_add(1),
            Outcome::Mismatch => s.mismatched = s.mismatched.saturating_add(1),
            Outcome::PartialSource => s.partial = s.partial.saturating_add(1),
            Outcome::Failed => s.failed = s.failed.saturating_add(1),
        }
        s.bytes_written = s.bytes_written.saturating_add(r.bytes_written);
        on_item(&r)?;
        p.files_done = p.files_done.saturating_add(1);
        on_progress(&p);
    }
    p.current = None;
    on_progress(&p);
    Ok(s)
}

enum Abort {
    Source(Error),
    DiskFull(ItemResult),
}

fn recover_one<S: BlockSource + ?Sized>(
    src: &S,
    item: &RecoverItem,
    session: &Path,
    cancel: &CancelToken,
    on_bytes: &mut dyn FnMut(u64),
) -> std::result::Result<ItemResult, Abort> {
    let fail = |note: String| ItemResult {
        id: item.id,
        output: None,
        bytes_written: 0,
        sha256: None,
        outcome: Outcome::Failed,
        note: Some(note),
    };
    if let Some(reason) = &item.skip_reason {
        return Ok(fail(reason.clone()));
    }
    let Some(name) = item.rel_path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return Ok(fail("No file name".into()));
    };
    let dir = session.join(item.rel_path.parent().unwrap_or(Path::new("")));
    if !dir.starts_with(session) {
        return Ok(fail("Unsafe output path rejected".into()));
    }
    if let Err(e) = std::fs::create_dir_all(&dir) {
        let full = is_disk_full(&e);
        let r = fail(format!("Cannot create folder: {e}"));
        return if full { Err(Abort::DiskFull(r)) } else { Ok(r) };
    }
    let (mut file, path) = match create_unique(&dir, &name) {
        Ok(v) => v,
        Err(e) => return Ok(fail(format!("Cannot create file: {e}"))),
    };
    let rel = path.strip_prefix(session).map(Path::to_path_buf).unwrap_or_else(|_| item.rel_path.clone());

    let written = match stream_item(src, item, &mut file, cancel, on_bytes) {
        Ok(w) => w,
        Err(err) => {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return match err {
                WriteError::Source(e) => Err(Abort::Source(e)),
                WriteError::Dest(e) => {
                    let r = fail(format!("Writing failed: {e}"));
                    if is_disk_full(&e) { Err(Abort::DiskFull(r)) } else { Ok(r) }
                }
            };
        }
    };
    if let Some(t) = item
        .modified_ms
        .and_then(|m| u64::try_from(m).ok())
        .and_then(|ms| UNIX_EPOCH.checked_add(Duration::from_millis(ms)))
    {
        // Best effort: keep the original modification time.
        let _ = file.set_modified(t);
    }
    drop(file);

    let mut notes = Vec::new();
    let readback = read_back_sha256(&path);
    let mut outcome = match &readback {
        Ok(h) if *h == written.sha256 => Outcome::Verified,
        Ok(_) => {
            notes.push("The file read back from the destination differs from what was written.".to_string());
            Outcome::Mismatch
        }
        Err(e) => {
            notes.push(format!("Could not read the file back to verify it: {e}"));
            Outcome::Mismatch
        }
    };
    if outcome == Outcome::Verified {
        if written.unreadable > 0 {
            notes.push(format!(
                "{} could not be read from the source and were written as zeros.",
                khodz_core::ByteSize(written.unreadable)
            ));
            outcome = Outcome::PartialSource;
        }
        if written.bytes < item.size {
            notes.push(format!(
                "Only {} of {} could be located.",
                khodz_core::ByteSize(written.bytes),
                khodz_core::ByteSize(item.size)
            ));
            outcome = Outcome::PartialSource;
        } else if let Some(damage) = &item.damage {
            notes.push(damage.clone());
            outcome = Outcome::PartialSource;
        } else if item.location_partial {
            notes.push("Only part of this file's location was known; it may contain other data.".into());
            outcome = Outcome::PartialSource;
        }
    }
    Ok(ItemResult {
        id: item.id,
        output: Some(rel),
        bytes_written: written.bytes,
        sha256: Some(written.sha256),
        outcome,
        note: if notes.is_empty() { None } else { Some(notes.join(" ")) },
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use khodz_io::MemSource;

    use super::*;

    fn data(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 31 % 251) as u8).collect()
    }

    fn item(id: i64, rel: &str, extents: Vec<AbsExtent>, size: u64) -> RecoverItem {
        RecoverItem {
            id,
            rel_path: PathBuf::from(rel),
            size,
            extents,
            resident: None,
            location_partial: false,
            skip_reason: None,
            damage: None,
            modified_ms: Some(1_700_000_000_000),
        }
    }

    fn sha(d: &[u8]) -> String {
        hex(&Sha256::digest(d))
    }

    fn run(
        src: &dyn BlockSource,
        items: &[RecoverItem],
        session: &Path,
    ) -> (RecoverySummary, Vec<ItemResult>) {
        let mut out = Vec::new();
        let s = recover(src, items, session, &CancelToken::new(), &mut |_| {}, &mut |r| {
            out.push(r.clone());
            Ok(())
        })
        .unwrap();
        (s, out)
    }

    #[test]
    fn writes_fragmented_sparse_and_resident_files_and_verifies_them() {
        let disk = data(4 << 20);
        let src = MemSource::new(disk.clone(), 512);
        let tmp = tempfile::tempdir().unwrap();
        let session = create_session_dir(tmp.path(), SystemTime::now()).unwrap();

        let frag = item(
            1,
            "Vol/docs/frag.bin",
            vec![
                AbsExtent { offset: Some(1 << 20), len: 1_500_000 },
                AbsExtent { offset: None, len: 4096 },
                AbsExtent { offset: Some(3 << 20), len: 10_000 },
            ],
            1_514_096,
        );
        let mut resident = item(2, "Vol/small.txt", vec![], 5);
        resident.resident = Some(b"hello world".to_vec());
        let (s, r) = run(&src, &[frag, resident], &session);

        assert_eq!((s.verified, s.failed), (2, 0));
        let mut expected = disk[1 << 20..(1 << 20) + 1_500_000].to_vec();
        expected.extend(vec![0u8; 4096]);
        expected.extend_from_slice(&disk[3 << 20..(3 << 20) + 10_000]);
        let written = std::fs::read(session.join("Vol/docs/frag.bin")).unwrap();
        assert_eq!(written, expected);
        assert_eq!(r[0].sha256.as_deref(), Some(sha(&expected).as_str()));
        assert_eq!(std::fs::read(session.join("Vol/small.txt")).unwrap(), b"hello");
        let mtime = std::fs::metadata(session.join("Vol/small.txt")).unwrap().modified().unwrap();
        assert_eq!(mtime, UNIX_EPOCH + Duration::from_millis(1_700_000_000_000), "modification time kept");
    }

    #[test]
    fn never_overwrites_existing_files() {
        let src = MemSource::new(data(8192), 512);
        let tmp = tempfile::tempdir().unwrap();
        let session = create_session_dir(tmp.path(), SystemTime::now()).unwrap();
        std::fs::create_dir_all(session.join("V")).unwrap();
        std::fs::write(session.join("V/a.txt"), b"precious").unwrap();
        let items = vec![
            item(1, "V/a.txt", vec![AbsExtent { offset: Some(0), len: 100 }], 100),
            item(2, "V/a.txt", vec![AbsExtent { offset: Some(100), len: 100 }], 100),
        ];
        let (_, r) = run(&src, &items, &session);
        assert_eq!(std::fs::read(session.join("V/a.txt")).unwrap(), b"precious");
        let names: HashSet<_> = r.iter().map(|x| x.output.clone().unwrap()).collect();
        assert_eq!(names, HashSet::from([PathBuf::from("V/a (1).txt"), PathBuf::from("V/a (2).txt")]));
    }

    #[test]
    fn session_folders_are_unique() {
        let tmp = tempfile::tempdir().unwrap();
        let t = SystemTime::now();
        let a = create_session_dir(tmp.path(), t).unwrap();
        let b = create_session_dir(tmp.path(), t).unwrap();
        assert_ne!(a, b);
        assert!(b.file_name().unwrap().to_string_lossy().ends_with("(1)"));
    }

    #[test]
    fn erased_partial_and_short_items_are_reported_honestly() {
        let src = MemSource::new(data(64 * 1024), 512);
        let tmp = tempfile::tempdir().unwrap();
        let session = create_session_dir(tmp.path(), SystemTime::now()).unwrap();
        let mut erased = item(1, "V/gone.bin", vec![AbsExtent { offset: Some(0), len: 4096 }], 4096);
        erased.skip_reason = Some("Other files have been written over all of it.".into());
        let mut assumed = item(2, "V/assumed.jpg", vec![AbsExtent { offset: Some(0), len: 4096 }], 4096);
        assumed.location_partial = true;
        let short = item(3, "V/short.bin", vec![AbsExtent { offset: Some(0), len: 1000 }], 5000);
        let mut damaged = item(4, "V/clip.mov", vec![AbsExtent { offset: Some(0), len: 4096 }], 4096);
        damaged.damage = Some("40% overwritten by other files".into());
        let (s, r) = run(&src, &[erased, assumed, short, damaged], &session);
        assert_eq!(r[0].outcome, Outcome::Failed);
        assert_eq!(r[0].note.as_deref(), Some("Other files have been written over all of it."));
        assert!(
            r[0].output.is_none() && !session.join("V/gone.bin").exists(),
            "nothing written for data that is gone"
        );
        assert_eq!(r[1].outcome, Outcome::PartialSource);
        assert_eq!((r[2].outcome, r[2].bytes_written), (Outcome::PartialSource, 1000));
        assert!(r[2].note.as_deref().unwrap().contains("Only 1000 B of 4.9 KiB"));
        assert_eq!(r[3].outcome, Outcome::PartialSource, "known damage is never reported verified");
        assert_eq!(r[3].note.as_deref(), Some("40% overwritten by other files"));
        assert_eq!((s.verified, s.partial, s.failed), (0, 3, 1));
    }

    #[test]
    fn unreadable_sectors_are_zero_filled_and_flagged() {
        struct Bad(MemSource);
        impl BlockSource for Bad {
            fn len(&self) -> u64 {
                self.0.len()
            }
            fn sector_size(&self) -> u32 {
                512
            }
            fn read_at(&self, off: u64, buf: &mut [u8]) -> khodz_core::Result<usize> {
                if off < 1024 && off + buf.len() as u64 > 512 {
                    return Err(Error::MediaError { offset: 512, detail: "CRC".into() });
                }
                self.0.read_at(off, buf)
            }
            fn describe(&self) -> String {
                "bad".into()
            }
        }
        let d = data(8192);
        let src = Bad(MemSource::new(d.clone(), 512));
        let tmp = tempfile::tempdir().unwrap();
        let session = create_session_dir(tmp.path(), SystemTime::now()).unwrap();
        let (_, r) =
            run(&src, &[item(1, "V/f.bin", vec![AbsExtent { offset: Some(0), len: 4096 }], 4096)], &session);
        assert_eq!(r[0].outcome, Outcome::PartialSource);
        let w = std::fs::read(session.join("V/f.bin")).unwrap();
        assert_eq!(&w[..512], &d[..512]);
        assert!(w[512..1024].iter().all(|&b| b == 0));
        assert_eq!(&w[1024..], &d[1024..4096]);
    }

    #[test]
    fn source_loss_and_cancellation_abort_and_clean_up() {
        struct Lost;
        impl BlockSource for Lost {
            fn len(&self) -> u64 {
                1 << 20
            }
            fn sector_size(&self) -> u32 {
                512
            }
            fn read_at(&self, _: u64, _: &mut [u8]) -> khodz_core::Result<usize> {
                Err(Error::DeviceLost("unplugged".into()))
            }
            fn describe(&self) -> String {
                "lost".into()
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let session = create_session_dir(tmp.path(), SystemTime::now()).unwrap();
        let it = item(1, "V/x.bin", vec![AbsExtent { offset: Some(0), len: 4096 }], 4096);
        let r = recover(
            &Lost,
            std::slice::from_ref(&it),
            &session,
            &CancelToken::new(),
            &mut |_| {},
            &mut |_| Ok(()),
        );
        assert!(matches!(r, Err(Error::DeviceLost(_))));
        assert!(!session.join("V/x.bin").exists(), "partial file removed");

        let cancel = CancelToken::new();
        cancel.cancel();
        let src = MemSource::new(data(8192), 512);
        let r = recover(&src, &[it], &session, &cancel, &mut |_| {}, &mut |_| Ok(()));
        assert!(matches!(r, Err(Error::Cancelled)));
    }

    #[test]
    fn progress_reports_files_and_bytes() {
        let src = MemSource::new(data(1 << 20), 512);
        let tmp = tempfile::tempdir().unwrap();
        let session = create_session_dir(tmp.path(), SystemTime::now()).unwrap();
        let items: Vec<_> = (0..3)
            .map(|i| item(i, &format!("V/{i}.bin"), vec![AbsExtent { offset: Some(0), len: 1000 }], 1000))
            .collect();
        let mut last = None;
        recover(
            &src,
            &items,
            &session,
            &CancelToken::new(),
            &mut |p| last = Some(p.clone()),
            &mut |_| Ok(()),
        )
        .unwrap();
        let p = last.unwrap();
        assert_eq!((p.files_done, p.files_total, p.bytes_done, p.bytes_total), (3, 3, 3000, 3000));
    }
}
