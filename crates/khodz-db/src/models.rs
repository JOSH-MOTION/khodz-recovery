use serde::{Deserialize, Serialize};

macro_rules! sql_enum {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }
            pub fn parse(s: &str) -> Option<Self> {
                match s { $($text => Some(Self::$variant),)+ _ => None }
            }
        }

        impl rusqlite::ToSql for $name {
            fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
                Ok(self.as_str().into())
            }
        }

        impl rusqlite::types::FromSql for $name {
            fn column_result(v: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
                let s = v.as_str()?;
                Self::parse(s).ok_or_else(|| rusqlite::types::FromSqlError::Other(
                    format!("invalid {} value: {s}", stringify!($name)).into(),
                ))
            }
        }
    };
}

sql_enum!(SourceKind { PhysicalDisk => "physical_disk", Volume => "volume", Image => "image" });
sql_enum!(ScanMode { Quick => "quick", Deep => "deep" });
sql_enum!(
    /// Terminal states are everything except `Running`.
    ScanStatus {
        Running => "running",
        Completed => "completed",
        Cancelled => "cancelled",
        Failed => "failed",
        DeviceLost => "device_lost",
    }
);
sql_enum!(RecoveryStatus {
    Running => "running",
    Completed => "completed",
    Cancelled => "cancelled",
    Failed => "failed",
});

/// Parameters for starting a scan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewScan {
    pub source_kind: SourceKind,
    pub source_path: String,
    pub source_label: Option<String>,
    pub source_size: u64,
    pub scan_mode: ScanMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scan {
    pub id: i64,
    pub source_kind: SourceKind,
    pub source_path: String,
    pub source_label: Option<String>,
    pub source_size: u64,
    pub scan_mode: ScanMode,
    pub status: ScanStatus,
    pub error: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub items_found: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recovery {
    pub id: i64,
    pub scan_id: Option<i64>,
    pub destination: String,
    pub status: RecoveryStatus,
    pub error: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub files_total: u64,
    pub files_verified: u64,
    pub files_failed: u64,
    pub bytes_written: u64,
}

/// Final counters reported when a recovery job ends.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct RecoveryTotals {
    pub files_total: u64,
    pub files_verified: u64,
    pub files_failed: u64,
    pub bytes_written: u64,
}

/// Bounded page request. `limit` is clamped to [`Page::MAX_LIMIT`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Page {
    pub limit: u32,
    pub offset: u32,
}

impl Page {
    pub const MAX_LIMIT: u32 = 500;

    pub fn clamped_limit(&self) -> u32 {
        self.limit.clamp(1, Self::MAX_LIMIT)
    }
}

impl Default for Page {
    fn default() -> Self {
        Self { limit: 50, offset: 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_round_trip_through_text() {
        for s in [ScanStatus::Running, ScanStatus::DeviceLost, ScanStatus::Cancelled] {
            assert_eq!(ScanStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(SourceKind::parse("nope"), None);
    }

    #[test]
    fn page_limit_is_clamped() {
        assert_eq!(Page { limit: 0, offset: 0 }.clamped_limit(), 1);
        assert_eq!(Page { limit: 10_000, offset: 0 }.clamped_limit(), Page::MAX_LIMIT);
    }
}
