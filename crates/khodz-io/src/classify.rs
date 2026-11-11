//! Map OS read errors to the engine's recovery-relevant categories.
//!
//! The distinction matters: a *media* error (bad sector) means "skip this range and keep
//! going"; a *device lost* error means "stop, the drive is gone".

use khodz_core::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadFailure {
    DeviceLost,
    Media,
    AccessDenied,
    Other,
}

#[cfg(windows)]
pub(crate) fn kind_for_os_code(code: i32) -> ReadFailure {
    match code {
        // ERROR_INVALID_HANDLE, ERROR_NOT_READY, ERROR_DEV_NOT_EXIST, ERROR_NO_SUCH_DEVICE,
        // ERROR_FILE_INVALID (volume externally altered), ERROR_DEVICE_NOT_CONNECTED,
        // ERROR_DEVICE_REMOVED
        6 | 21 | 55 | 433 | 1006 | 1167 | 1617 => ReadFailure::DeviceLost,
        // ERROR_CRC, ERROR_SECTOR_NOT_FOUND, ERROR_READ_FAULT, ERROR_GEN_FAILURE,
        // ERROR_DEVICE_HARDWARE_ERROR, ERROR_IO_DEVICE
        23 | 27 | 30 | 31 | 483 | 1117 => ReadFailure::Media,
        5 => ReadFailure::AccessDenied,
        _ => ReadFailure::Other,
    }
}

#[cfg(unix)]
pub(crate) fn kind_for_os_code(code: i32) -> ReadFailure {
    match code {
        6 /* ENXIO */ | 19 /* ENODEV */ | 9 /* EBADF */ => ReadFailure::DeviceLost,
        5 /* EIO */ => ReadFailure::Media,
        1 /* EPERM */ | 13 /* EACCES */ => ReadFailure::AccessDenied,
        _ => ReadFailure::Other,
    }
}

#[cfg(not(any(windows, unix)))]
pub(crate) fn kind_for_os_code(_code: i32) -> ReadFailure {
    ReadFailure::Other
}

/// Convert an I/O error from reading `what` at `offset` into an engine [`Error`].
pub fn classify_io_error(e: std::io::Error, offset: u64, what: &str) -> Error {
    let kind = match e.raw_os_error() {
        Some(code) => kind_for_os_code(code),
        None if e.kind() == std::io::ErrorKind::PermissionDenied => ReadFailure::AccessDenied,
        None => ReadFailure::Other,
    };
    match kind {
        ReadFailure::DeviceLost => Error::DeviceLost(format!("{what}: {e}")),
        ReadFailure::Media => Error::MediaError { offset, detail: format!("{what}: {e}") },
        ReadFailure::AccessDenied => Error::AccessDenied(format!("{what}: {e}")),
        ReadFailure::Other => Error::Io(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(code: i32) -> std::io::Error {
        std::io::Error::from_raw_os_error(code)
    }

    #[cfg(windows)]
    #[test]
    fn windows_codes() {
        assert!(matches!(classify_io_error(os(1167), 0, "d"), Error::DeviceLost(_)));
        assert!(matches!(classify_io_error(os(21), 0, "d"), Error::DeviceLost(_)));
        assert!(matches!(classify_io_error(os(23), 4096, "d"), Error::MediaError { offset: 4096, .. }));
        assert!(matches!(classify_io_error(os(5), 0, "d"), Error::AccessDenied(_)));
        assert!(matches!(classify_io_error(os(87), 0, "d"), Error::Io(_)));
    }

    #[cfg(unix)]
    #[test]
    fn unix_codes() {
        assert!(matches!(classify_io_error(os(6), 0, "d"), Error::DeviceLost(_)));
        assert!(matches!(classify_io_error(os(5), 512, "d"), Error::MediaError { offset: 512, .. }));
        assert!(matches!(classify_io_error(os(13), 0, "d"), Error::AccessDenied(_)));
    }

    #[test]
    fn non_os_errors() {
        let e = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "x");
        assert!(matches!(classify_io_error(e, 0, "d"), Error::AccessDenied(_)));
        let e = std::io::Error::other("x");
        assert!(matches!(classify_io_error(e, 0, "d"), Error::Io(_)));
    }
}
