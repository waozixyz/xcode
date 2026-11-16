//! Launches the helper with its Bazel-provided native startup libraries available.

use anyhow::Result;
use codex_utils_cargo_bin::cargo_bin;
use tokio::process::Command;

pub(crate) fn voice_host_command() -> Result<Command> {
    let command = Command::new(cargo_bin("codex-voice-host")?);
    Ok(command)
}
