//! The client version Xcoder reports to the Codex service.
//!
//! Xcoder keeps upstream's `0.0.0` development workspace version, but the
//! Codex service hides newer models (for example `gpt-6.1-sol`) from `0.0.0`
//! clients and refuses them on model requests. Everything that tells the
//! service which client is calling reports the tracked upstream release
//! instead of the development placeholder.

/// Upstream Codex release whose model catalog and request contract this build tracks.
pub const TRACKED_UPSTREAM_CLIENT_VERSION: &str = "0.169.0";

/// The `major.minor.patch` version reported to the Codex service.
pub fn reported_client_version() -> &'static str {
    reported_version(env!("CARGO_PKG_VERSION"))
}

fn reported_version(build_version: &str) -> &str {
    let whole = build_version
        .split(['-', '+'])
        .next()
        .unwrap_or(build_version);
    if whole == "0.0.0" {
        TRACKED_UPSTREAM_CLIENT_VERSION
    } else {
        whole
    }
}

#[cfg(test)]
#[path = "client_version_tests.rs"]
mod tests;
