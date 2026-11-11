//! Physical disk and volume discovery.
//!
//! **Safety:** enumeration never opens a device for reading or writing. On Windows every handle
//! is opened with zero access rights (metadata IOCTLs only); on macOS only `diskutil` metadata
//! queries are run. Reading device contents is the job of `khodz-io` (Stage 3).

// Overflow-checked arithmetic is enforced for production code; test fixtures build buffers freely.
#![cfg_attr(test, allow(clippy::arithmetic_side_effects))]

mod model;

pub use model::*;

#[cfg(any(windows, test))]
mod assemble;
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))] // compiled on other OSes only for its unit tests
mod win_parse;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(any(target_os = "macos", test))]
mod macos_parse;

#[cfg(target_os = "macos")]
use macos::list_disks as platform_list;
#[cfg(windows)]
use windows::list_disks as platform_list;

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_list() -> khodz_core::Result<Vec<PhysicalDisk>> {
    Err(khodz_core::Error::Unsupported("device discovery is implemented for Windows and macOS only".into()))
}

/// List physical disks with the volumes that live on them, sorted by OS disk order.
pub fn list_disks() -> khodz_core::Result<Vec<PhysicalDisk>> {
    platform_list()
}
