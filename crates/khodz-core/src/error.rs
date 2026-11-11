use std::fmt;

/// Engine-wide error type.
///
/// Variants are deliberately coarse: the UI maps each one to a distinct, actionable message.
#[derive(Debug)]
pub enum Error {
    /// Underlying operating-system I/O failure.
    Io(std::io::Error),
    /// The user cancelled the operation.
    Cancelled,
    /// The source device disappeared or stopped responding mid-operation.
    DeviceLost(String),
    /// The medium could not return data for a range (bad sector, CRC error). The device is still
    /// present; callers may skip the range and continue.
    MediaError { offset: u64, detail: String },
    /// The OS refused access (missing elevation, Full Disk Access, locked volume, …).
    AccessDenied(String),
    /// On-disk data failed validation (corrupt structure, out-of-range value, …).
    InvalidData(String),
    /// The requested capability is not supported (see SUPPORTED_FILESYSTEMS.md).
    Unsupported(String),
    /// A recovery destination was rejected by safety checks.
    UnsafeDestination(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(e) => write!(f, "I/O error: {e}"),
            Error::Cancelled => f.write_str("operation cancelled"),
            Error::DeviceLost(d) => write!(f, "device disconnected or unreadable: {d}"),
            Error::MediaError { offset, detail } => write!(f, "unreadable data at byte {offset}: {detail}"),
            Error::AccessDenied(d) => write!(f, "access denied: {d}"),
            Error::InvalidData(d) => write!(f, "invalid data: {d}"),
            Error::Unsupported(d) => write!(f, "unsupported: {d}"),
            Error::UnsafeDestination(d) => write!(f, "unsafe recovery destination: {d}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            Error::AccessDenied(e.to_string())
        } else {
            Error::Io(e)
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_denied_maps_to_access_denied() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "nope");
        assert!(matches!(Error::from(io), Error::AccessDenied(_)));
    }

    #[test]
    fn other_io_errors_stay_io() {
        let io = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "short read");
        let err = Error::from(io);
        assert!(matches!(err, Error::Io(_)));
        assert!(std::error::Error::source(&err).is_some());
    }

    #[test]
    fn display_is_human_readable() {
        assert_eq!(Error::Cancelled.to_string(), "operation cancelled");
        assert_eq!(
            Error::UnsafeDestination("same disk".into()).to_string(),
            "unsafe recovery destination: same disk"
        );
    }
}
