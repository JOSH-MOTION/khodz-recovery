//! Raw device access (whole disks and volumes), opened read-only.
//!
//! Both platforms require elevated rights for raw reads (Administrator on Windows; root or Full
//! Disk Access on macOS). Without them [`open_device`] returns `Error::AccessDenied` — it never
//! tries to elevate itself.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::RawDevice;
#[cfg(windows)]
pub use windows::RawDevice;

/// Open a disk or volume for read-only access. The returned source accepts unaligned reads.
#[cfg(any(windows, target_os = "macos"))]
pub fn open_device(path: &str) -> khodz_core::Result<crate::AlignedSource<RawDevice>> {
    Ok(crate::AlignedSource::new(RawDevice::open(path)?))
}

/// Accept only device paths, never arbitrary files: image files go through `ImageFile`.
pub fn is_device_path(path: &str) -> bool {
    #[cfg(windows)]
    {
        let p = path.to_ascii_lowercase();
        if let Some(n) = p.strip_prefix(r"\\.\physicaldrive") {
            return !n.is_empty() && n.chars().all(|c| c.is_ascii_digit());
        }
        if let Some(rest) = p.strip_prefix(r"\\.\") {
            let b = rest.as_bytes();
            return b.len() == 2 && b[0].is_ascii_alphabetic() && b[1] == b':';
        }
        if let Some(rest) = p.strip_prefix(r"\\?\volume{") {
            return rest.ends_with('}') && !rest.contains('\\');
        }
        false
    }
    #[cfg(not(windows))]
    {
        path.strip_prefix("/dev/rdisk")
            .or_else(|| path.strip_prefix("/dev/disk"))
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric()))
    }
}

#[cfg(test)]
mod tests {
    use super::is_device_path;

    #[cfg(windows)]
    #[test]
    fn windows_device_paths() {
        assert!(is_device_path(r"\\.\PhysicalDrive0"));
        assert!(is_device_path(r"\\.\PhysicalDrive12"));
        assert!(is_device_path(r"\\.\C:"));
        assert!(is_device_path(r"\\?\Volume{1234-abcd}"));
        assert!(!is_device_path(r"\\.\PhysicalDrive"));
        assert!(!is_device_path(r"\\.\PhysicalDrive0\..\x"));
        assert!(!is_device_path(r"C:\Users\me\disk.img"));
        assert!(!is_device_path(r"\\.\C:\Windows"));
        assert!(!is_device_path(r"\\server\share"));
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_device_paths() {
        assert!(is_device_path("/dev/rdisk2"));
        assert!(is_device_path("/dev/disk3s1"));
        assert!(!is_device_path("/dev/rdisk"));
        assert!(!is_device_path("/dev/rdisk2/../../etc/passwd"));
        assert!(!is_device_path("/Users/me/disk.img"));
    }
}
