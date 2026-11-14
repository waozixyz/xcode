#![cfg(all(target_os = "linux", target_env = "gnu"))]
//! The idle-memory release in `native/idle.zi`: it must lower the resident
//! set, lose no data, and run once per idle period.

use std::time::{Duration, Instant};

unsafe extern "C" {
    fn xcoder_idle_release(report: *mut i64) -> i32;
    fn xcoder_idle_watch(idle_milliseconds: i64) -> i32;
    fn xcoder_idle_releases() -> i64;
}

const SIZE: usize = 64 << 20;

fn checksum(data: &[u8]) -> u64 {
    data.iter()
        .fold(0xcbf29ce484222325u64, |hash, byte| (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3))
}

#[test]
fn idle_release_keeps_data_and_runs_once_per_idle_period() {
    // Distinct contents on every page, so a lost or zeroed page changes the sum.
    let mut data = vec![0u8; SIZE];
    for (index, byte) in data.iter_mut().enumerate() {
        *byte = (index.wrapping_mul(2654435761) >> 9) as u8;
    }
    let expected = checksum(&data);

    let mut report = [0i64; 5];
    assert_eq!(unsafe { xcoder_idle_release(report.as_mut_ptr()) }, 1);
    let [before, after, unmapped, paged, mappings] = report;
    println!("resident {before} -> {after} bytes; unmapped {unmapped}, to swap {paged}, {mappings} mappings");
    assert!(before > SIZE as i64, "the buffer was resident before the release");
    assert!(mappings > 0);
    // Unmodified program pages are always dropped. Anonymous pages leave only
    // when swap space is free, so the size of the drop depends on the host.
    assert!(unmapped > 0);
    assert!(paged >= SIZE as i64);
    assert!(after < before);
    assert_eq!(checksum(&data), expected, "released memory must come back unchanged");
    assert_eq!(unsafe { xcoder_idle_releases() }, 1);

    // The watcher starts once and releases after the process goes quiet.
    assert_eq!(unsafe { xcoder_idle_watch(500) }, 1);
    assert_eq!(unsafe { xcoder_idle_watch(500) }, 0);
    let deadline = Instant::now() + Duration::from_secs(30);
    while unsafe { xcoder_idle_releases() } < 2 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(unsafe { xcoder_idle_releases() }, 2, "an idle process is released");
    // Still idle and not grown: no further passes.
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(unsafe { xcoder_idle_releases() }, 2, "one release per idle period");
    assert_eq!(checksum(&data), expected);
}
