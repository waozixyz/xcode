//! Platform-specific pipe ownership beneath the shared MCP framing layer.

use std::io;

use codex_utils_pty::Child;
use codex_utils_pty::Command;

pub(super) type Stdin = tokio::process::ChildStdin;
pub(super) type Stdout = tokio::process::ChildStdout;
pub(super) type Stderr = tokio::process::ChildStderr;

pub(super) struct LocalStdio {
    pub(super) child: Child,
    pub(super) stdin: Stdin,
    pub(super) stdout: Stdout,
    pub(super) stderr: Option<Stderr>,
}

pub(super) fn spawn(command: Command) -> io::Result<LocalStdio> {
    {
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("MCP server stdin was not piped"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("MCP server stdout was not piped"))?;
        let stderr = child.stderr.take();
        Ok(LocalStdio {
            child,
            stdin,
            stdout,
            stderr,
        })
    }
}
