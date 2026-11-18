//! The client version Xcode reports to the Codex service.
//!
//! The Codex service hides newer models (for example `gpt-6.1-sol`) from
//! older clients and refuses them on model requests. Xcode therefore reports
//! the newest upstream Codex release it knows about, never its own version:
//! Xcode's release number (`xcode --version`) is separate and unrelated.
//!
//! The newest release known is the larger of [`TRACKED_UPSTREAM_CLIENT_VERSION`],
//! built in, and the latest upstream release recorded at startup from the
//! cache that the CLI refreshes in the background (see
//! [`record_latest_upstream_version`]).

use std::sync::RwLock;

/// Oldest upstream Codex release this build may report; the floor before any
/// newer release has been seen.
pub const TRACKED_UPSTREAM_CLIENT_VERSION: &str = "0.169.0";

static LATEST_UPSTREAM: RwLock<Option<&'static str>> = RwLock::new(None);

/// The `major.minor.patch` version reported to the Codex service.
pub fn reported_client_version() -> &'static str {
    let latest = LATEST_UPSTREAM.read().ok().and_then(|latest| *latest);
    match latest {
        Some(latest) if is_newer(latest, TRACKED_UPSTREAM_CLIENT_VERSION) => latest,
        _ => TRACKED_UPSTREAM_CLIENT_VERSION,
    }
}

/// Records an upstream Codex release (`0.170.0`, `v0.170.0` or `rust-v0.170.0`).
/// The reported version only ever moves forward; malformed or older values are
/// ignored. Returns the normalized version when it was accepted.
pub fn record_latest_upstream_version(version: &str) -> Option<String> {
    let version = normalize(version)?;
    let mut latest = LATEST_UPSTREAM.write().ok()?;
    let current = latest.unwrap_or(TRACKED_UPSTREAM_CLIENT_VERSION);
    if !is_newer(&version, current) {
        return None;
    }
    *latest = Some(Box::leak(version.clone().into_boxed_str()));
    Some(version)
}

/// `rust-v0.170.0`, `v0.170.0` or `0.170.0` as `0.170.0`; prereleases and
/// anything else are rejected.
pub fn normalize(version: &str) -> Option<String> {
    let version = version.trim();
    let version = version
        .strip_prefix("rust-v")
        .or_else(|| version.strip_prefix('v'))
        .unwrap_or(version);
    parse(version).map(|(major, minor, patch)| format!("{major}.{minor}.{patch}"))
}

/// Whether `candidate` is a strictly newer `major.minor.patch` than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse(candidate), parse(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.');
    let mut next = || {
        let part = parts.next()?;
        (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| part.parse().ok())
            .flatten()
    };
    let parsed = (next()?, next()?, next()?);
    parts.next().is_none().then_some(parsed)
}

#[cfg(test)]
#[path = "client_version_tests.rs"]
mod tests;
