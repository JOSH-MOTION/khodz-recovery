//! Raw device access against real hardware.
#![cfg(any(windows, target_os = "macos"))]

use khodz_core::Error;
use khodz_io::BlockSource;
use khodz_io::device::open_device;

#[test]
fn rejects_non_device_paths() {
    assert!(matches!(open_device("C:/Users/x/file.img"), Err(Error::InvalidData(_))));
    assert!(matches!(open_device("/etc/passwd"), Err(Error::InvalidData(_))));
}

/// Unelevated: the OS must refuse raw access and we must report it as AccessDenied (never a
/// crash, never an attempt to elevate). Elevated: the open succeeds read-only.
#[test]
fn raw_open_is_denied_or_read_only() {
    #[cfg(windows)]
    let path = r"\\.\PhysicalDrive0";
    #[cfg(target_os = "macos")]
    let path = "/dev/rdisk0";
    match open_device(path) {
        Err(Error::AccessDenied(msg)) => assert!(msg.contains(path), "{msg}"),
        Ok(dev) => {
            assert!(dev.len() > 0);
            let mut b = [0u8; 512];
            assert_eq!(dev.read_exact_at(0, &mut b).unwrap(), 512);
        }
        Err(other) => panic!("unexpected error: {other}"),
    }
}

/// Needs elevation and a test device: `KHODZ_TEST_DEVICE=\\.\PhysicalDrive2`.
/// Run with `cargo test -p khodz-io --test raw_device -- --ignored`.
#[test]
#[ignore = "requires administrator rights and KHODZ_TEST_DEVICE"]
fn reads_real_device_consistently() {
    let path = std::env::var("KHODZ_TEST_DEVICE").expect("set KHODZ_TEST_DEVICE");
    let dev = open_device(&path).expect("open (run elevated)");
    assert!(dev.len() > 1 << 20);
    assert!(matches!(dev.sector_size(), 512 | 4096));

    // Sector 0 of an MBR- or GPT-partitioned disk ends with the 0x55AA boot signature.
    let mut sector0 = vec![0u8; dev.sector_size() as usize];
    dev.read_exact_at(0, &mut sector0).unwrap();
    assert_eq!(&sector0[510..512], &[0x55, 0xAA], "boot signature");

    // Unaligned read through the adapter matches the aligned bytes.
    let mut first = vec![0u8; 1 << 20];
    dev.read_exact_at(0, &mut first).unwrap();
    let mut odd = vec![0u8; 1000];
    dev.read_exact_at(777, &mut odd).unwrap();
    assert_eq!(&odd[..], &first[777..1777]);

    // Reading past the end returns nothing rather than erroring.
    let mut tail = [0u8; 4096];
    assert_eq!(dev.read_at(dev.len(), &mut tail).unwrap(), 0);
    println!("{path}: {} bytes, {} B sectors — OK", dev.len(), dev.sector_size());
}
