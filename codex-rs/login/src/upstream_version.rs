//! Keeps the version reported to the Codex service on the newest upstream
//! Codex release (see `codex_protocol::client_version`).
//!
//! The latest release is cached in `CODEX_HOME/xcoder-upstream-version.json`.
//! [`load_cached`] records it at startup, before any request is built, and
//! [`refresh_in_background`] asks GitHub for the latest `openai/codex` release
//! once the cache is older than [`REFRESH_AFTER`]; the answer applies to the
//! rest of this process and to every later one.
//! `XCODER_UPSTREAM_VERSION_CHECK=0` turns the network check off.

use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use codex_protocol::client_version::normalize;
use codex_protocol::client_version::record_latest_upstream_version;
use serde::Deserialize;
use serde::Serialize;

use crate::default_client::create_client;
use crate::default_client::default_headers;

const CACHE_FILENAME: &str = "xcoder-upstream-version.json";
const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/openai/codex/releases/latest";
const CHECK_ENV_VAR: &str = "XCODER_UPSTREAM_VERSION_CHECK";
/// How old the cached release may be before it is checked again.
pub const REFRESH_AFTER: Duration = Duration::from_secs(20 * 60 * 60);

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct CachedRelease {
    version: String,
    /// Seconds since the Unix epoch.
    checked_at: u64,
}

#[derive(Deserialize)]
struct ReleaseInfo {
    tag_name: String,
}

fn cache_path(codex_home: &Path) -> PathBuf {
    codex_home.join(CACHE_FILENAME)
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn read_cache(codex_home: &Path) -> Option<CachedRelease> {
    let contents = std::fs::read_to_string(cache_path(codex_home)).ok()?;
    serde_json::from_str(&contents).ok()
}

/// Records the cached latest release, if any. Call before building requests.
pub fn load_cached(codex_home: &Path) {
    if let Some(cached) = read_cache(codex_home) {
        record_latest_upstream_version(&cached.version);
    }
}

fn needs_refresh(cached: Option<&CachedRelease>, now: u64) -> bool {
    cached.is_none_or(|cached| now.saturating_sub(cached.checked_at) >= REFRESH_AFTER.as_secs())
}

fn check_enabled() -> bool {
    std::env::var(CHECK_ENV_VAR).map_or(true, |value| value.trim() != "0")
}

/// Starts a background check when the cache is stale; a failed check leaves
/// the cache and the reported version unchanged.
///
/// The check runs on its own detached thread and runtime, so a slow network
/// (or a DNS lookup that never returns) can never delay the process exiting.
pub fn refresh_in_background(codex_home: PathBuf) {
    if !check_enabled() || !needs_refresh(read_cache(&codex_home).as_ref(), now_seconds()) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("xcoder-upstream-version".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::debug!("upstream Codex version check failed: {error}");
                    return;
                }
            };
            if let Err(error) = runtime.block_on(refresh(&codex_home)) {
                tracing::debug!("upstream Codex version check failed: {error}");
            }
        });
    if let Err(error) = spawned {
        tracing::debug!("upstream Codex version check failed: {error}");
    }
}

async fn refresh(codex_home: &Path) -> Result<(), String> {
    let release = create_client()
        .get(LATEST_RELEASE_URL)
        .headers(default_headers())
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json::<ReleaseInfo>()
        .await
        .map_err(|error| error.to_string())?;
    let version = normalize(&release.tag_name)
        .ok_or_else(|| format!("unexpected release tag {}", release.tag_name))?;
    record_latest_upstream_version(&version);
    write_cache(
        codex_home,
        &CachedRelease {
            version,
            checked_at: now_seconds(),
        },
    )
}

fn write_cache(codex_home: &Path, cached: &CachedRelease) -> Result<(), String> {
    let path = cache_path(codex_home);
    let contents = serde_json::to_string(cached).map_err(|error| error.to_string())?;
    // Write a sibling and rename it so concurrent sessions never read half a file.
    let temporary = path.with_extension(format!("json.{}", std::process::id()));
    std::fs::write(&temporary, contents + "\n").map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, &path).map_err(|error| error.to_string())
}

#[cfg(test)]
#[path = "upstream_version_tests.rs"]
mod tests;
