/// The build's own package version, used for local client/server compatibility checks.
pub const CODEX_CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The Codex version shown to the user: the same upstream Codex version Xcoder reports to the
/// service, never the build's 0.0.0 placeholder.
pub(crate) fn codex_display_version() -> &'static str {
    codex_protocol::client_version::reported_client_version()
}
