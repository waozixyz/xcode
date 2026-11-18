// Standalone Rust-to-Zi2c linkage test, also runnable before Cargo downloads.
#[repr(C)]
struct Tool { name: *const u8, length: i64, kind: i32 }
unsafe extern "C" {
    fn xcode_resolve_tool(name: *const u8, length: i64, catalog: *const Tool, count: i64) -> i64;
    fn xcode_executor_init(state: *mut Executor, count: i64, lanes: i64) -> i32;
    fn xcode_executor_next(state: *mut Executor) -> i64;
    fn xcode_executor_complete(state: *mut Executor) -> i32;
}
#[repr(C)]
#[derive(Default)]
struct Executor { cursor: i64, active: i64, completed: i64, count: i64, lanes: i64 }
#[test]
fn precompiled_executor_bounds_parallelism_and_completes_exactly_once() {
    let mut state = Executor::default();
    unsafe {
        assert_eq!(xcode_executor_init(&mut state, 9, 4), 1);
        for index in 0..4 { assert_eq!(xcode_executor_next(&mut state), index); }
        assert_eq!(xcode_executor_next(&mut state), -1);
        for index in 4..9 {
            assert_eq!(xcode_executor_complete(&mut state), 1);
            assert_eq!(xcode_executor_next(&mut state), index);
        }
        for _ in 0..4 { assert_eq!(xcode_executor_complete(&mut state), 1); }
        assert_eq!(state.completed, 9);
        assert_eq!(xcode_executor_next(&mut state), -1);
        assert_eq!(xcode_executor_complete(&mut state), 0);
        assert_eq!(xcode_executor_init(&mut state, 129, 1), 0);
        assert_eq!(xcode_executor_init(&mut state, 1, 5), 0);
        assert_eq!(xcode_executor_init(std::ptr::null_mut(), 1, 1), 0);
    }
}
fn lookup(name: &[u8], names: &[&[u8]]) -> i64 {
    let catalog: Vec<_> = names.iter().map(|name| Tool { name: name.as_ptr(), length: name.len() as i64, kind: 1 }).collect();
    unsafe { xcode_resolve_tool(name.as_ptr(), name.len() as i64, catalog.as_ptr(), catalog.len() as i64) }
}
#[test]
fn actual_ffi_exact_utf8_embedded_nul_and_duplicates() {
    let catalog: &[&[u8]] = &[b"shell", "文字工具".as_bytes(), b"a\0b", b"dup", b"dup"];
    assert_eq!(lookup(b"shell", catalog), 0);
    assert_eq!(lookup("文字工具".as_bytes(), catalog), 1);
    assert_eq!(lookup(b"a\0b", catalog), 2);
    assert_eq!(lookup(b"dup", catalog), -1);
    assert_eq!(lookup(b"shel", catalog), -1);
    assert_eq!(lookup(b"", catalog), -1);
    assert_eq!(lookup(b"missing", &[]), -1);
}
#[test]
fn real_ffi_is_reentrant() {
    let threads: Vec<_> = (0..8).map(|_| std::thread::spawn(|| {
        for _ in 0..10000 { assert_eq!(lookup(b"target", &[b"other", b"target"]), 1); }
    })).collect();
    for thread in threads { thread.join().unwrap(); }
}
#[test]
fn malformed_foreign_arguments_are_rejected() {
    unsafe {
        assert_eq!(xcode_resolve_tool(std::ptr::null(), 5, std::ptr::null(), 1), -1);
        assert_eq!(xcode_resolve_tool(b"x".as_ptr(), 1, std::ptr::null(), -1), -1);
        assert_eq!(xcode_resolve_tool(b"x".as_ptr(), 1, std::ptr::null(), 1), -1);
    }
}
