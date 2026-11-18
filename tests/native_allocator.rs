#![cfg(all(target_os = "linux", target_env = "gnu"))]

#[path = "../native/allocator.rs"]
mod allocator;

use std::ffi::c_void;
use std::sync::{Arc, Barrier};

unsafe extern "C" {
    fn tmpfile() -> *mut c_void;
    fn malloc_info(options: i32, stream: *mut c_void) -> i32;
    fn fflush(stream: *mut c_void) -> i32;
    fn fseek(stream: *mut c_void, offset: i64, origin: i32) -> i32;
    fn fread(buffer: *mut u8, size: usize, count: usize, stream: *mut c_void) -> usize;
    fn fclose(stream: *mut c_void) -> i32;
}

fn arenas() -> usize {
    unsafe {
        let file = tmpfile();
        assert!(!file.is_null());
        assert_eq!(malloc_info(0, file), 0);
        assert_eq!(fflush(file), 0);
        assert_eq!(fseek(file, 0, 0), 0);
        let mut xml = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let count = fread(buffer.as_mut_ptr(), 1, buffer.len(), file);
            if count == 0 {
                break;
            }
            xml.extend_from_slice(&buffer[..count]);
        }
        assert_eq!(fclose(file), 0);
        String::from_utf8(xml).unwrap().matches("<heap nr=").count()
    }
}

#[test]
fn allocator_probe() {
    let Ok(mode) = std::env::var("XCODE_ALLOCATOR_PROBE") else {
        return;
    };
    let configured = allocator::configure();
    assert_eq!(configured, mode == "default");
    let ready = Arc::new(Barrier::new(13));
    let done = Arc::new(Barrier::new(13));
    let threads: Vec<_> = (0..12)
        .map(|_| {
            let ready = ready.clone();
            let done = done.clone();
            std::thread::spawn(move || {
                let blocks: Vec<_> = (0..1000).map(|i| Box::new([i as u8; 1024])).collect();
                ready.wait();
                done.wait();
                std::hint::black_box(blocks);
            })
        })
        .collect();
    ready.wait();
    let count = arenas();
    done.wait();
    for thread in threads {
        thread.join().unwrap();
    }
    if mode == "default" {
        assert!(count <= 2, "arena cap was not applied: {count}");
    } else {
        assert!(count > 2, "owner arena setting was overridden: {count}");
    }
}

#[test]
fn real_allocator_cap_and_explicit_owner_settings() {
    for mode in ["default", "environment", "tunable"] {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "allocator_probe", "--nocapture"])
            .env("XCODE_ALLOCATOR_PROBE", mode)
            .env_remove("LD_PRELOAD")
            .env_remove("MALLOC_ARENA_MAX")
            .env_remove("GLIBC_TUNABLES");
        if mode == "environment" {
            command.env("MALLOC_ARENA_MAX", "8");
        }
        if mode == "tunable" {
            command.env("GLIBC_TUNABLES", "glibc.malloc.arena_max=8");
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{mode}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
