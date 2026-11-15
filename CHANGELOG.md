The changelog can be found on the [releases page](https://github.com/openai/codex/releases).

## Unreleased

- Parallel tool cells no longer fail at random with "Text file busy": starting a freshly compiled cell retries while another thread's fork still holds the file.
- The upstream Codex version check runs on its own thread, so a slow network never delays Xcoder exiting; the offline local tests turn it off.

## 0.1.0 — 2026-11-14

First Xcoder release: Codex with native Ziran tool execution, for Linux x86_64.

- Installing replaces the previous binaries in place; the installer no longer keeps copies of them.
- Shell commands that run before the session's shell snapshot is written also put Xcoder's `apply_patch` ahead of a login profile's `PATH`; only snapshot restores replayed it before.
- Tool cells no longer fail with "unexpected end of file" when they exit: the callback channels a cell leaves unused now close quietly.

- The version Xcoder reports to the Codex service follows the newest upstream Codex release, not Xcoder's own version: at least 0.169.0, and newer once GitHub's latest `openai/codex` release says so (checked in the background at most every 20 hours, cached in `CODEX_HOME/xcoder-upstream-version.json`; `XCODER_UPSTREAM_VERSION_CHECK=0` turns the check off). The model catalog, the User-Agent and the `version` header all use it, so newer models such as GPT-6.1-Sol are listed and accepted; while requests said `0.0.0`, the service hid GPT-6.1-Sol and refused it.
- `xcoder --version` shows Xcoder's own release number.

- Remove parts Xcoder never uses under Harmony: the `cloud`, `tcp-tunnel`, `responses-api-proxy`, `exec-server` and `debug app-server` subcommands with their seven crates, the Ollama and LM Studio providers, and the AWS SDK behind Amazon Bedrock sign-in. `--oss` now stops at startup with an explanation instead of probing for local servers, and Bedrock selection reports that it is unavailable. About 22,000 lines and the AWS SDK dependency tree are gone.

- An idle Xcoder session gives its memory back. After two minutes without work (`XCODER_IDLE_RELEASE_SECONDS`; `0` turns it off) a watcher written in Ziran returns the allocator's free pages, moves private memory to swap and unmaps unmodified program pages. The session stays intact and pages back what its next event touches. `make benchmark-memory` measures a long offline session before and after.

- The TUI's replay copy of a finished command keeps only the start and end of output above 16 KiB; the live cell still holds the full output. Every command's output was held twice for the life of the session.

- Identify the running Xcoder CLI process as `xcode` on Linux, including when
  compatibility launch paths retain their existing filenames.

- Shell commands run Xcoder's own `apply_patch`, even when a login profile resets `PATH` or puts another `apply_patch` first. A profile copy had dropped each added file's first line and written the rest twice.

- New standalone installations expose the `xcoder` command without adding a `codex` alias.
- Download tested Linux x86_64 Xcoder builds, including the native compiler, from the repository's Xcoder Linux workflow.

- Render Mermaid diagrams through the standalone Ziran `mermaid` package; the Rust crate is only its adapter, and the original renderer's snapshots and public-API tests run against it.

- Use the exact streamed custom tool input when a final snapshot disagrees with its deltas; reject mismatched completions and incomplete streams.

- Replace the TUI fuzzy matcher with a Ziran native implementation that streams the haystack and caches only the lowered needle.

- Bound GNU allocator arena retention while preserving owner settings; keep custom-cell compilation to one worker and support long private runtime paths.

- Execute ordinary tool plans through the precompiled Ziran executor and cache custom Zi2c cells; add matched offline JavaScript/native benchmarks.

- Xcoder executes tool programs natively from Ziran, with tool approvals, cancellation and saved session values.
- Certify installation only after the complete local test suite passes for the exact packaged executables; test pending parallel calls and recovery after callback errors.

- Fix missing command lifecycle events when unified exec cannot create a process, preserving approval rejection and sandbox retry behavior. Thanks @Marvinthebored ([#44557](https://github.com/openai/codex/issues/44557)).
