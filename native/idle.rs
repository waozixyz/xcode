//! Start the idle-memory watcher written in `native/idle.zi`.
//!
//! After the process has been idle for `XCODER_IDLE_RELEASE_SECONDS` (120 by
//! default; 0 disables it) the watcher returns the allocator's free pages,
//! moves private memory to swap and unmaps unmodified program pages. The
//! session stays intact and pages back whatever its next event touches.

/// Call only from the long-lived CLI process: helper personalities that
/// create namespaces must remain single-threaded.
pub fn start() -> bool {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        /// Seconds without work before an idle session's memory is released.
        const DEFAULT_IDLE_SECONDS: f64 = 120.0;
        unsafe extern "C" {
            fn xcoder_idle_watch(idle_milliseconds: i64) -> i32;
        }
        let seconds = std::env::var("XCODER_IDLE_RELEASE_SECONDS")
            .ok()
            .and_then(|value| value.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite())
            .unwrap_or(DEFAULT_IDLE_SECONDS);
        if seconds <= 0.0 {
            return false;
        }
        // SAFETY: the Ziran export takes one integer and starts a detached
        // thread that reads only its own process's mappings.
        return unsafe { xcoder_idle_watch((seconds * 1000.0).min(86_400_000.0) as i64) == 1 };
    }
    #[cfg(not(all(target_os = "linux", target_env = "gnu")))]
    false
}
