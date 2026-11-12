//! Recovery jobs: resolve a selection, validate the destination, write and verify files, record
//! history and write `khodz-manifest.json` into the session folder.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};

use khodz_core::{CancelToken, Error};
use khodz_db::{
    ItemView, NewRecoveryItem, Page, Recovery, RecoveryItemRow, RecoveryStatus, RecoveryTotals,
    ScanItemDetail, Scope, SourceKind,
};
use khodz_engine::AbsExtent;
use khodz_recover::{
    DestinationCheck, ItemResult, Outcome, RecoverItem, RecoveryProgress, RecoverySummary, SourceRef,
    check_destination, create_session_dir, output_relative_path, recover,
};

use crate::commands::CommandError;
use crate::scans::{ScanSource, open_source_for_scan};
use crate::state::AppState;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SelectionPart {
    Items { ids: Vec<i64> },
    Folder { scope: Scope, dir: String, view: ItemView },
}

#[derive(Debug, Serialize)]
pub struct SelectionSummary {
    pub files: u64,
    pub bytes: u64,
    /// Files whose data was erased; they will be reported as failed, not written.
    pub erased: u64,
    /// Files that will be written but are known to be incomplete.
    pub partial: u64,
}

#[derive(Debug, Serialize)]
pub struct DestinationReport {
    pub summary: SelectionSummary,
    pub check: Option<DestinationCheck>,
}

#[derive(Debug, Serialize)]
pub struct RecoveryOutcome {
    pub recovery_id: i64,
    pub status: RecoveryStatus,
    pub session_dir: String,
    pub summary: RecoverySummary,
    pub error: Option<String>,
}

fn invalid(msg: impl Into<String>) -> CommandError {
    CommandError { kind: "invalid_request", message: msg.into() }
}

/// Resolve a selection to item details (files only, de-duplicated, in a stable order).
fn resolve(
    state: &AppState,
    scan_id: i64,
    selection: &[SelectionPart],
) -> Result<Vec<ScanItemDetail>, CommandError> {
    let db = state.db()?;
    let mut ids: BTreeSet<i64> = BTreeSet::new();
    for part in selection {
        match part {
            SelectionPart::Items { ids: list } => ids.extend(list.iter().copied()),
            SelectionPart::Folder { scope, dir, view } => {
                ids.extend(db.folder_item_ids(scan_id, *scope, dir, *view)?)
            }
        }
    }
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let d = db.get_scan_item(id)?;
        if d.scan_id != scan_id {
            return Err(invalid(format!("item {id} does not belong to scan {scan_id}")));
        }
        if !d.row.is_dir {
            out.push(d);
        }
    }
    Ok(out)
}

fn summarize(items: &[ScanItemDetail]) -> SelectionSummary {
    let erased = |d: &ScanItemDetail| d.row.data_state == "erased";
    SelectionSummary {
        files: items.len() as u64,
        bytes: items.iter().filter(|d| !erased(d)).map(|d| d.row.size).fold(0, u64::saturating_add),
        erased: items.iter().filter(|d| erased(d)).count() as u64,
        partial: items
            .iter()
            .filter(|d| {
                !erased(d)
                    && (d.row.extent_quality == "partial" || d.row.carve_status.as_deref() == Some("partial"))
            })
            .count() as u64,
    }
}

fn source_ref(source: &ScanSource) -> SourceRef<'_> {
    match source {
        ScanSource::Image { path } => SourceRef::Image(Path::new(path)),
        ScanSource::Device { path, .. } => SourceRef::Device(path),
    }
}

fn scan_source(state: &AppState, scan_id: i64) -> Result<ScanSource, CommandError> {
    let scan = state.db()?.get_scan(scan_id)?;
    Ok(match scan.source_kind {
        SourceKind::Image => ScanSource::Image { path: scan.source_path },
        _ => ScanSource::Device { path: scan.source_path, label: scan.source_label },
    })
}

pub fn check_impl(
    state: &AppState,
    scan_id: i64,
    selection: &[SelectionPart],
    destination: Option<&str>,
) -> Result<DestinationReport, CommandError> {
    let items = resolve(state, scan_id, selection)?;
    let summary = summarize(&items);
    let check = match destination {
        Some(d) => {
            Some(check_destination(Path::new(d), source_ref(&scan_source(state, scan_id)?), summary.bytes))
        }
        None => None,
    };
    Ok(DestinationReport { summary, check })
}

#[tauri::command]
pub async fn check_recovery(
    app: AppHandle,
    scan_id: i64,
    selection: Vec<SelectionPart>,
    destination: Option<String>,
) -> Result<DestinationReport, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        check_impl(&app.state::<AppState>(), scan_id, &selection, destination.as_deref())
    })
    .await
    .map_err(|e| CommandError::internal(e.to_string()))?
}

/// Output folder name for a volume ("LNTFS (NTFS)") or for signature results.
fn group_names(state: &AppState, scan_id: i64) -> Result<HashMap<u64, String>, CommandError> {
    let vols = state.db()?.list_scan_volumes(scan_id, ItemView::Everything)?;
    Ok(vols
        .into_iter()
        .map(|v| {
            let name = match v.label {
                Some(l) => format!("{l} ({})", v.filesystem),
                None => format!("{} volume at {}", v.filesystem, khodz_core::ByteSize(v.offset)),
            };
            (v.offset, name)
        })
        .collect())
}

fn to_recover_item(d: &ScanItemDetail, groups: &HashMap<u64, String>) -> RecoverItem {
    let rel = if d.row.origin == "carved" {
        output_relative_path("Found by signature", d.row.path.trim_start_matches("/$Carved"))
    } else {
        let g =
            groups.get(&d.volume_offset).cloned().unwrap_or_else(|| format!("Volume at {}", d.volume_offset));
        output_relative_path(&g, &d.row.path)
    };
    RecoverItem {
        id: d.row.id,
        rel_path: rel,
        size: d.row.size,
        extents: d.extents.iter().map(|&(offset, len)| AbsExtent { offset, len }).collect(),
        resident: d.resident.clone(),
        location_partial: d.row.extent_quality == "partial"
            || d.row.carve_status.as_deref() == Some("partial"),
        erased: d.row.data_state == "erased",
        modified_ms: d.row.modified_at,
    }
}

#[derive(Serialize)]
struct ManifestFile<'a> {
    source_path: &'a str,
    output: Option<String>,
    size: u64,
    sha256: Option<&'a str>,
    outcome: &'static str,
    note: Option<&'a str>,
    location: &'a str,
}

#[allow(clippy::too_many_arguments)]
fn write_manifest(
    session: &Path,
    source: &ScanSource,
    scan_id: i64,
    same_disk_override: bool,
    started: SystemTime,
    status: RecoveryStatus,
    summary: &RecoverySummary,
    files: &[(ScanItemDetail, ItemResult)],
) -> std::io::Result<()> {
    let secs = |t: SystemTime| t.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (kind, path) = match source {
        ScanSource::Image { path } => ("image", path.as_str()),
        ScanSource::Device { path, .. } => ("device", path.as_str()),
    };
    let doc = serde_json::json!({
        "tool": "Khodz Recovery",
        "version": env!("CARGO_PKG_VERSION"),
        "started_unix": secs(started),
        "finished_unix": secs(SystemTime::now()),
        "status": status.as_str(),
        "source": { "kind": kind, "path": path },
        "scan_id": scan_id,
        "same_disk_override": same_disk_override,
        "summary": summary,
        "hash_algorithm": "SHA-256 of the bytes written; each file was re-read from the destination and compared",
        "files": files.iter().map(|(d, r)| ManifestFile {
            source_path: &d.row.path,
            output: r.output.as_ref().map(|p| p.to_string_lossy().replace('\\', "/")),
            size: r.bytes_written,
            sha256: r.sha256.as_deref(),
            outcome: r.outcome.as_str(),
            note: r.note.as_deref(),
            location: &d.row.extent_quality,
        }).collect::<Vec<_>>(),
    });
    let text = serde_json::to_string_pretty(&doc).map_err(std::io::Error::other)?;
    // create_new: never overwrite (the session folder is new, so this cannot collide).
    let mut f =
        std::fs::OpenOptions::new().write(true).create_new(true).open(session.join("khodz-manifest.json"))?;
    std::io::Write::write_all(&mut f, text.as_bytes())?;
    f.sync_all()
}

pub fn run_recovery(
    state: &AppState,
    scan_id: i64,
    selection: &[SelectionPart],
    destination: &str,
    allow_same_disk: bool,
    cancel: &CancelToken,
    mut on_progress: impl FnMut(&RecoveryProgress),
) -> Result<RecoveryOutcome, CommandError> {
    let items = resolve(state, scan_id, selection)?;
    if items.is_empty() {
        return Err(invalid("Nothing selected to recover."));
    }
    let source = scan_source(state, scan_id)?;
    let summary0 = summarize(&items);
    let check = check_destination(Path::new(destination), source_ref(&source), summary0.bytes);
    if !check.ok(allow_same_disk) {
        let mut why = check.problems.clone();
        if check.same_disk && !allow_same_disk {
            why.push("The destination is on the same physical disk as the drive being recovered.".into());
        }
        return Err(CommandError { kind: "unsafe_destination", message: why.join(" ") });
    }
    let src = open_source_for_scan(state, scan_id)?;
    let groups = group_names(state, scan_id)?;
    let work: Vec<RecoverItem> = items.iter().map(|d| to_recover_item(d, &groups)).collect();
    let by_id: HashMap<i64, &ScanItemDetail> = items.iter().map(|d| (d.row.id, d)).collect();

    let started = SystemTime::now();
    let session = create_session_dir(Path::new(destination), started)?;
    let session_str = session.display().to_string();
    let rec = state.db()?.create_recovery(Some(scan_id), &session_str, check.same_disk && allow_same_disk)?;

    let mut results: Vec<(ScanItemDetail, ItemResult)> = Vec::with_capacity(work.len());
    let mut pending: Vec<NewRecoveryItem> = Vec::new();
    let mut last = Instant::now().checked_sub(Duration::from_secs(1)).unwrap_or_else(Instant::now);
    let flush = |pending: &mut Vec<NewRecoveryItem>| -> khodz_core::Result<()> {
        if pending.is_empty() {
            return Ok(());
        }
        let mut db = state.db().map_err(|e| Error::InvalidData(e.message))?;
        db.insert_recovery_items(rec.id, pending).map_err(|e| Error::InvalidData(e.to_string()))?;
        pending.clear();
        Ok(())
    };
    let result = recover(
        &*src,
        &work,
        &session,
        cancel,
        &mut |p| {
            if last.elapsed() >= Duration::from_millis(100) || p.files_done == p.files_total {
                last = Instant::now();
                on_progress(p);
            }
        },
        &mut |r| {
            if let Some(d) = by_id.get(&r.id) {
                results.push(((*d).clone(), r.clone()));
            }
            pending.push(NewRecoveryItem {
                scan_item_id: Some(r.id),
                output_path: r.output.as_ref().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
                size: r.bytes_written,
                sha256: r.sha256.clone(),
                outcome: r.outcome.as_str().into(),
                error: r.note.clone(),
            });
            if pending.len() >= 500 { flush(&mut pending) } else { Ok(()) }
        },
    );
    let flushed = flush(&mut pending);

    let (status, error, summary) = match (result, flushed) {
        (Ok(s), Ok(())) => (
            if s.stopped.is_some() { RecoveryStatus::Failed } else { RecoveryStatus::Completed },
            s.stopped.clone(),
            s,
        ),
        (Err(Error::Cancelled), _) => (RecoveryStatus::Cancelled, None, tally(&results)),
        (Err(e), _) | (Ok(_), Err(e)) => (RecoveryStatus::Failed, Some(e.to_string()), tally(&results)),
    };
    let manifest = write_manifest(
        &session,
        &source,
        scan_id,
        rec.same_disk_override,
        started,
        status,
        &summary,
        &results,
    );
    let error = match (error, manifest) {
        (e, Ok(())) => e,
        (Some(e), Err(m)) => Some(format!("{e}; manifest not written: {m}")),
        (None, Err(m)) => Some(format!("Files were recovered but the manifest could not be written: {m}")),
    };
    state.db()?.finish_recovery(
        rec.id,
        status,
        RecoveryTotals {
            files_total: summary.files_total,
            files_verified: summary.verified,
            files_failed: summary.failed,
            bytes_written: summary.bytes_written,
            files_partial: summary.partial,
            files_mismatch: summary.mismatched,
        },
        error.as_deref(),
    )?;
    Ok(RecoveryOutcome { recovery_id: rec.id, status, session_dir: session_str, summary, error })
}

/// Counters from the results gathered before an abort.
fn tally(results: &[(ScanItemDetail, ItemResult)]) -> RecoverySummary {
    let mut s = RecoverySummary { files_total: results.len() as u64, ..RecoverySummary::default() };
    for (_, r) in results {
        s.bytes_written = s.bytes_written.saturating_add(r.bytes_written);
        let n = match r.outcome {
            Outcome::Verified => &mut s.verified,
            Outcome::Mismatch => &mut s.mismatched,
            Outcome::PartialSource => &mut s.partial,
            Outcome::Failed => &mut s.failed,
        };
        *n = n.saturating_add(1);
    }
    s
}

#[tauri::command]
pub async fn start_recovery(
    app: AppHandle,
    scan_id: i64,
    selection: Vec<SelectionPart>,
    destination: String,
    allow_same_disk: bool,
    job_id: String,
    on_progress: Channel<RecoveryProgress>,
) -> Result<RecoveryOutcome, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let cancel = state.register_job(&job_id)?;
        let r = run_recovery(&state, scan_id, &selection, &destination, allow_same_disk, &cancel, |p| {
            let _ = on_progress.send(p.clone());
        });
        state.finish_job(&job_id);
        r
    })
    .await
    .map_err(|e| CommandError::internal(e.to_string()))?
}

#[tauri::command]
pub fn list_recovery_items(
    state: State<'_, AppState>,
    recovery_id: i64,
    page: Option<Page>,
) -> Result<Vec<RecoveryItemRow>, CommandError> {
    Ok(state.db()?.list_recovery_items(recovery_id, page.unwrap_or_default())?)
}

#[tauri::command]
pub fn get_recovery(state: State<'_, AppState>, recovery_id: i64) -> Result<Recovery, CommandError> {
    Ok(state.db()?.get_recovery(recovery_id)?)
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::scans::run_scan;
    use khodz_engine::ScanMode;

    fn state() -> AppState {
        AppState::new(khodz_db::Database::open_in_memory().unwrap(), "memory".into())
    }

    #[test]
    #[ignore = "needs fixtures/out/carving.img and gpt-ntfs.img"]
    fn recovers_selection_writes_manifest_and_history() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/out/");
        let s = state();
        let scan = run_scan(
            &s,
            &ScanSource::Image { path: format!("{dir}gpt-ntfs.img") },
            ScanMode::Quick,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        let vols = s.db().unwrap().list_scan_volumes(scan.scan_id, ItemView::AllFiles).unwrap();
        let scope = Scope::Volume { offset: vols[0].offset };
        let selection = vec![SelectionPart::Folder { scope, dir: "/".into(), view: ItemView::AllFiles }];

        let report = check_impl(&s, scan.scan_id, &selection, None).unwrap();
        assert!(report.summary.files >= 11 && report.summary.erased == 0, "{:?}", report.summary);

        let out = tempfile::tempdir().unwrap();
        let mut progress = Vec::new();
        let r = run_recovery(
            &s,
            scan.scan_id,
            &selection,
            out.path().to_str().unwrap(),
            false,
            &CancelToken::new(),
            |p| progress.push(p.clone()),
        )
        .unwrap();
        assert_eq!(r.status, RecoveryStatus::Completed);
        assert_eq!(r.summary.verified, r.summary.files_total);
        let session = PathBuf::from(&r.session_dir);
        assert!(session.starts_with(out.path()));

        // Folder structure and Unicode names survive; content matches.
        let unicode = session.join("KNTFS (NTFS)").join("café ñ 日本.txt");
        assert!(unicode.exists(), "{unicode:?}");
        let fs_manifest: serde_json::Value = serde_json::from_str(
            std::fs::read_to_string(format!("{dir}manifest.json")).unwrap().trim_start_matches('\u{feff}'),
        )
        .unwrap();
        let ntfs =
            fs_manifest["images"].as_array().unwrap().iter().find(|i| i["file"] == "gpt-ntfs.img").unwrap();
        let files = ntfs["volumes"]["files"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| ntfs["volumes"][0]["files"].as_array().unwrap().clone());
        for f in files.iter().filter(|f| f["deleted"] == false) {
            let p = session.join("KNTFS (NTFS)").join(f["path"].as_str().unwrap().replace('/', "\\"));
            let got: String =
                Sha256::digest(std::fs::read(&p).unwrap()).iter().map(|b| format!("{b:02x}")).collect();
            assert_eq!(got, f["sha256"].as_str().unwrap(), "{p:?}");
        }

        // Manifest + history.
        let m: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(session.join("khodz-manifest.json")).unwrap())
                .unwrap();
        assert_eq!(m["status"], "completed");
        assert_eq!(m["files"].as_array().unwrap().len() as u64, r.summary.files_total);
        let rec = s.db().unwrap().get_recovery(r.recovery_id).unwrap();
        assert_eq!(rec.files_verified, r.summary.verified);
        let rows =
            s.db().unwrap().list_recovery_items(r.recovery_id, Page { limit: 500, offset: 0 }).unwrap();
        assert_eq!(rows.len() as u64, r.summary.files_total);
        assert!(rows.iter().all(|x| x.sha256.as_deref().is_some_and(|h| h.len() == 64)));
        assert!(progress.last().is_some_and(|p| p.files_done == p.files_total));

        // Recovering again never overwrites: a new session folder is created.
        let r2 = run_recovery(
            &s,
            scan.scan_id,
            &selection,
            out.path().to_str().unwrap(),
            false,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        assert_ne!(r2.session_dir, r.session_dir);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "needs fixtures/out/gpt-ntfs.img"]
    fn device_source_on_same_disk_is_refused_without_override() {
        // Pretend the scan came from the physical disk that holds the temp folder.
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/out/");
        let s = state();
        let scan = run_scan(
            &s,
            &ScanSource::Image { path: format!("{dir}gpt-ntfs.img") },
            ScanMode::Quick,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap();
        let out = tempfile::tempdir().unwrap();
        let disk = khodz_recover::disks_for_path(out.path()).unwrap()[0].clone();
        let check = check_destination(out.path(), SourceRef::Device(&format!(r"\\.\{disk}")), 1);
        assert!(check.same_disk && !check.ok(false));
        let _ = scan;
    }

    #[test]
    fn nothing_selected_and_bad_destination_are_errors() {
        let s = state();
        let scan = s
            .db()
            .unwrap()
            .create_scan(&khodz_db::NewScan {
                source_kind: SourceKind::Image,
                source_path: "Z:/x.img".into(),
                source_label: None,
                source_size: 1,
                scan_mode: khodz_db::ScanMode::Quick,
            })
            .unwrap();
        let e = run_recovery(
            &s,
            scan.id,
            &[SelectionPart::Items { ids: vec![] }],
            "Z:/nowhere",
            false,
            &CancelToken::new(),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(e.kind, "invalid_request");
        assert!(
            s.db().unwrap().list_recoveries(Page::default()).unwrap().is_empty(),
            "no history row for a refused run"
        );
    }
}
