//! Forward-only schema migrations keyed on SQLite's `user_version`.

use rusqlite::Connection;

use crate::{DbError, DbResult};

/// Each entry upgrades the schema from version `index` to `index + 1`. Never edit a shipped
/// migration; append a new one.
const MIGRATIONS: &[&str] = &[
    // v1 — initial schema
    r#"
    CREATE TABLE scans (
        id            INTEGER PRIMARY KEY,
        source_kind   TEXT    NOT NULL CHECK (source_kind IN ('physical_disk','volume','image')),
        source_path   TEXT    NOT NULL,
        source_label  TEXT,
        source_size   INTEGER NOT NULL CHECK (source_size >= 0),
        scan_mode     TEXT    NOT NULL CHECK (scan_mode IN ('quick','deep')),
        status        TEXT    NOT NULL CHECK (status IN ('running','completed','cancelled','failed','device_lost')),
        error         TEXT,
        started_at    INTEGER NOT NULL,
        finished_at   INTEGER,
        items_found   INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX idx_scans_started ON scans(started_at DESC);

    CREATE TABLE scan_items (
        id             INTEGER PRIMARY KEY,
        scan_id        INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
        parent_id      INTEGER REFERENCES scan_items(id) ON DELETE CASCADE,
        name           TEXT    NOT NULL,
        path           TEXT    NOT NULL,
        size           INTEGER NOT NULL CHECK (size >= 0),
        is_dir         INTEGER NOT NULL DEFAULT 0,
        is_deleted     INTEGER NOT NULL DEFAULT 0,
        origin         TEXT    NOT NULL CHECK (origin IN ('metadata','carved')),
        extent_quality TEXT    NOT NULL CHECK (extent_quality IN ('exact','contiguity_assumed','partial','unknown')),
        filesystem     TEXT,
        file_type      TEXT,
        created_at     INTEGER,
        modified_at    INTEGER,
        extents        BLOB
    );
    CREATE INDEX idx_scan_items_parent ON scan_items(scan_id, parent_id);

    CREATE TABLE recoveries (
        id             INTEGER PRIMARY KEY,
        scan_id        INTEGER REFERENCES scans(id) ON DELETE SET NULL,
        destination    TEXT    NOT NULL,
        status         TEXT    NOT NULL CHECK (status IN ('running','completed','cancelled','failed')),
        error          TEXT,
        started_at     INTEGER NOT NULL,
        finished_at    INTEGER,
        files_total    INTEGER NOT NULL DEFAULT 0,
        files_verified INTEGER NOT NULL DEFAULT 0,
        files_failed   INTEGER NOT NULL DEFAULT 0,
        bytes_written  INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX idx_recoveries_started ON recoveries(started_at DESC);

    CREATE TABLE recovery_items (
        id           INTEGER PRIMARY KEY,
        recovery_id  INTEGER NOT NULL REFERENCES recoveries(id) ON DELETE CASCADE,
        scan_item_id INTEGER REFERENCES scan_items(id) ON DELETE SET NULL,
        output_path  TEXT    NOT NULL,
        size         INTEGER NOT NULL,
        sha256       TEXT,
        outcome      TEXT    NOT NULL CHECK (outcome IN ('verified','mismatch','partial_source','failed')),
        error        TEXT
    );

    CREATE TABLE settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    "#,
    // v2 — fields produced by the scan engine (Stage 5)
    r#"
    ALTER TABLE scan_items ADD COLUMN volume_offset INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE scan_items ADD COLUMN name_complete INTEGER NOT NULL DEFAULT 1;
    ALTER TABLE scan_items ADD COLUMN is_system     INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE scan_items ADD COLUMN data_state    TEXT NOT NULL DEFAULT 'present'
        CHECK (data_state IN ('present','erased','unknown'));
    ALTER TABLE scan_items ADD COLUMN carve_status  TEXT
        CHECK (carve_status IS NULL OR carve_status IN ('complete','unverified','partial'));
    ALTER TABLE scan_items ADD COLUMN note          TEXT;
    ALTER TABLE scan_items ADD COLUMN resident      BLOB;
    CREATE INDEX idx_scan_items_path    ON scan_items(scan_id, path);
    CREATE INDEX idx_scan_items_deleted ON scan_items(scan_id, is_deleted);
    CREATE INDEX idx_scan_items_origin  ON scan_items(scan_id, origin);
    "#,
    // v3 — per-volume scan summary (Stage 6 explorer)
    r#"
    CREATE TABLE scan_volumes (
        id         INTEGER PRIMARY KEY,
        scan_id    INTEGER NOT NULL REFERENCES scans(id) ON DELETE CASCADE,
        offset     INTEGER NOT NULL,
        size       INTEGER NOT NULL,
        filesystem TEXT    NOT NULL,
        label      TEXT,
        entries    INTEGER NOT NULL DEFAULT 0,
        deleted    INTEGER NOT NULL DEFAULT 0,
        corrupt    INTEGER NOT NULL DEFAULT 0,
        error      TEXT
    );
    CREATE INDEX idx_scan_volumes ON scan_volumes(scan_id);
    "#,
    // v4 — richer recovery history (Stage 7)
    r#"
    ALTER TABLE recoveries ADD COLUMN files_partial      INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE recoveries ADD COLUMN files_mismatch     INTEGER NOT NULL DEFAULT 0;
    ALTER TABLE recoveries ADD COLUMN same_disk_override INTEGER NOT NULL DEFAULT 0;
    CREATE INDEX idx_recovery_items ON recovery_items(recovery_id);
    "#,
    // v5 — what is really left of a deleted file: how much of its assumed location other files
    // now occupy, and whether its content was checked against its format.
    r#"
    ALTER TABLE scan_items ADD COLUMN overwritten_pct INTEGER
        CHECK (overwritten_pct IS NULL OR overwritten_pct BETWEEN 0 AND 100);
    ALTER TABLE scan_items ADD COLUMN content_check TEXT
        CHECK (content_check IS NULL OR content_check IN ('verified','damaged'));
    "#,
];

pub const LATEST: u32 = MIGRATIONS.len() as u32;

pub fn migrate(conn: &mut Connection) -> DbResult<()> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if current > LATEST {
        return Err(DbError::SchemaTooNew { found: current, supported: LATEST });
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (index as u32).saturating_add(1))?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_fresh_database_to_latest() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        let v: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v, LATEST);
        // Idempotent.
        migrate(&mut conn).unwrap();
    }

    #[test]
    fn upgrades_v1_database_with_data_in_place() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute_batch(
            "INSERT INTO scans (source_kind, source_path, source_size, scan_mode, status, started_at)
               VALUES ('image', 'old.img', 10, 'quick', 'completed', 0);
             INSERT INTO scan_items (scan_id, name, path, size, origin, extent_quality)
               VALUES (1, 'a.txt', '/a.txt', 5, 'metadata', 'exact');",
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        let v: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v, LATEST);
        let (path, state): (String, String) = conn
            .query_row("SELECT path, data_state FROM scan_items", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap();
        assert_eq!(
            (path.as_str(), state.as_str()),
            ("/a.txt", "present"),
            "old rows kept, new columns defaulted"
        );
    }

    #[test]
    fn refuses_newer_schema() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.pragma_update(None, "user_version", LATEST + 1).unwrap();
        assert!(matches!(migrate(&mut conn), Err(DbError::SchemaTooNew { .. })));
    }

    #[test]
    fn check_constraints_reject_bad_enum_values() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        let bad = conn.execute(
            "INSERT INTO scans (source_kind, source_path, source_size, scan_mode, status, started_at)
             VALUES ('floppy', 'x', 0, 'quick', 'running', 0)",
            [],
        );
        assert!(bad.is_err());
    }
}
