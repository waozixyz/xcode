//! Validates snapshots of append-only local rollouts before reusing them for resume.
//!
//! Capture before and after reading; any change or unavailable metadata disables reuse. The
//! selected path prevents reuse for a different rollout. Validate under writer ownership,
//! before opening the recorder can materialize a compressed file.

use std::path::Path;

pub(super) async fn read(path: &Path) -> Option<String> {
    let path = codex_rollout::existing_rollout_path(path).await?;
    let path = tokio::fs::canonicalize(path).await.ok()?;
    let metadata = tokio::fs::metadata(&path).await.ok()?;
    let modified = metadata.modified().ok()?;
    let created = metadata.created().ok();
    let identity = {
        use std::os::unix::fs::MetadataExt;
        Some((
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        ))
    };
    serde_json::to_string(&("local", path, metadata.len(), modified, created, identity)).ok()
}
