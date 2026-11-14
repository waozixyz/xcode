//! Bound GNU allocator arena retention before starting async workers. Explicit
//! owner allocator settings retain precedence; other allocators keep their policy.
pub fn configure() -> bool {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        if std::env::var_os("MALLOC_ARENA_MAX").is_some()
            || std::env::var_os("LD_PRELOAD").is_some_and(|value| !value.is_empty())
            || std::env::var_os("GLIBC_TUNABLES").is_some_and(|value| {
                value
                    .to_string_lossy()
                    .split(':')
                    .any(|setting| setting.starts_with("glibc.malloc."))
            })
        {
            return false;
        }
        unsafe extern "C" {
            fn mallopt(parameter: std::ffi::c_int, value: std::ffi::c_int) -> std::ffi::c_int;
        }
        // glibc's M_ARENA_MAX is -8. malloc reads env tunables before main,
        // so setting an environment variable here would be too late.
        return unsafe { mallopt(-8, 2) == 1 };
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    false
}
