mod context {
    pub(crate) mod world_state {
        use codex_config::Sourced;
        use codex_utils_string::approx_bytes_for_tokens;
        use codex_utils_string::approx_tokens_from_byte_count;
        use std::io;

        const MAX_MANAGED_DEVELOPER_INSTRUCTIONS_TOKENS: usize = 10_000;
        const REPLACEMENT_NOTICE: &str = "These managed developer instructions replace all previously provided managed developer instructions.";
        const START_MARKER: &str = "<managed_developer_instructions>";
        const END_MARKER: &str = "</managed_developer_instructions>";

        pub(crate) fn validate_managed_developer_instructions(
            requirement: Option<&Sourced<String>>,
        ) -> io::Result<()> {
            let Some(requirement) = requirement else {
                return Ok(());
            };
            let overhead = START_MARKER
                .len()
                .saturating_add(1)
                .saturating_add(REPLACEMENT_NOTICE.len())
                .saturating_add(2)
                .saturating_add(1)
                .saturating_add(END_MARKER.len());
            let rendered_bytes = requirement.value.len().saturating_add(overhead);
            if rendered_bytes > approx_bytes_for_tokens(MAX_MANAGED_DEVELOPER_INSTRUCTIONS_TOKENS) {
                let estimated_tokens = approx_tokens_from_byte_count(rendered_bytes);
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "`additional_developer_instructions` from {} exceeds the model-context limit of {MAX_MANAGED_DEVELOPER_INSTRUCTIONS_TOKENS} estimated tokens ({estimated_tokens} including context markers)",
                        requirement.source,
                    ),
                ));
            }
            Ok(())
        }
    }
}

mod path_utils {
    pub(crate) use codex_utils_path::normalize_for_native_workdir;
    pub(crate) use codex_utils_path::resolve_symlink_write_paths;
    pub(crate) use codex_utils_path::write_atomically;
}

mod responses_metadata {
    const RESERVED_METADATA_KEYS: &[&str] = &[
        "guardian_credits_requested",
        "guardian_recorded_items",
        "mcp_attribution",
        "installation_id",
        "x-codex-installation-id",
        "session_id",
        "thread_id",
        "agent_name",
        "turn_id",
        "window_id",
        "window_number",
        "context_window_id",
        "x-codex-window-id",
        "x-codex-turn-metadata",
        "x-codex-parent-thread-id",
        "x-openai-subagent",
        "request_kind",
        "compaction",
        "code_mode_tool_names",
        "tool_namespaces_info",
        "turn_started_at_unix_ms",
        "history_ingest_requested",
        "analytics_enabled",
        "history_initialization",
        "forked_from_thread_id",
        "forked_from_ordinal_exclusive",
        "parent_thread_id",
        "parent_turn_id",
        "root_turn_id",
        "subagent_kind",
        "thread_source",
        "turn_trigger",
        "sandbox",
        "sandbox_mode",
        "auto_review_enabled",
        "node_repl_auto_review_required",
        "node_repl_disabled",
        "workspaces",
    ];
    const BACKWARD_COMPATIBLE_RESERVED_METADATA_KEYS: &[&str] = &[
        "guardian_recorded_items",
        "window_number",
        "forked_from_ordinal_exclusive",
        "analytics_enabled",
    ];
    const MAX_EXTRA_METADATA_ENTRIES: usize = 16;
    const MAX_EXTRA_METADATA_KEY_BYTES: usize = 64;
    const MAX_EXTRA_METADATA_VALUE_BYTES: usize = 128;

    pub(crate) fn validate_extra_metadata<'a>(
        extra: impl IntoIterator<Item = (&'a String, &'a String)>,
    ) -> Result<(), &'static str> {
        let mut count = 0;
        for (key, value) in extra {
            count += 1;
            if count > MAX_EXTRA_METADATA_ENTRIES {
                return Err("responses_api_metadata may contain at most 16 entries");
            }
            if key.len() > MAX_EXTRA_METADATA_KEY_BYTES || !valid_extra_metadata_key(key) {
                return Err("responses_api_metadata keys must be short ASCII identifiers");
            }
            if RESERVED_METADATA_KEYS.contains(&key.as_str())
                && !BACKWARD_COMPATIBLE_RESERVED_METADATA_KEYS.contains(&key.as_str())
            {
                return Err("responses_api_metadata contains a reserved key");
            }
            if value.len() > MAX_EXTRA_METADATA_VALUE_BYTES {
                return Err("responses_api_metadata values may contain at most 128 bytes");
            }
        }
        Ok(())
    }

    fn valid_extra_metadata_key(key: &str) -> bool {
        let mut bytes = key.bytes();
        bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
    }
}

mod unified_exec {
    pub(crate) const MIN_EMPTY_YIELD_TIME_MS: u64 = 5_000;
    pub(crate) const DEFAULT_MAX_BACKGROUND_TERMINAL_TIMEOUT_MS: u64 = 300_000;
}

#[path = "../../core/src/config/mod.rs"]
pub mod config;

impl codex_rollout::RolloutConfigView for config::Config {
    fn codex_home(&self) -> &std::path::Path {
        self.codex_home.as_path()
    }

    fn sqlite_config(&self) -> &codex_state::SqliteConfig {
        self.sqlite_config()
    }

    fn cwd(&self) -> &std::path::Path {
        self.cwd.as_path()
    }

    fn model_provider_id(&self) -> &str {
        self.model_provider_id.as_str()
    }

    fn generate_memories(&self) -> bool {
        self.memories.generate_memories
    }
}
