//! Local model providers (Ollama, LM Studio) are not part of this build.
//! The entry points remain for the `--oss` flags of the TUI and exec.

use codex_core::config::Config;
use codex_model_provider_info::LMSTUDIO_OSS_PROVIDER_ID;
use codex_model_provider_info::OLLAMA_OSS_PROVIDER_ID;

/// Why `--oss` stops at startup.
pub const OSS_UNAVAILABLE: &str =
    "--oss is unavailable: local model providers (Ollama, LM Studio) are not included in this build";

/// No local provider ships a default model in this build.
pub fn get_default_model_for_oss_provider(_provider_id: &str) -> Option<&'static str> {
    None
}

/// Reports that a local provider cannot be prepared; other providers need no setup.
pub async fn ensure_oss_provider_ready(
    provider_id: &str,
    _config: &Config,
) -> Result<(), std::io::Error> {
    match provider_id {
        LMSTUDIO_OSS_PROVIDER_ID | OLLAMA_OSS_PROVIDER_ID => Err(std::io::Error::other(OSS_UNAVAILABLE)),
        _ => Ok(()),
    }
}
