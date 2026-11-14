The changelog can be found on the [releases page](https://github.com/openai/codex/releases).

## Unreleased

- Report the tracked upstream Codex release (0.169.0) instead of the `0.0.0` development
  version when fetching the model catalog, so newer models such as GPT-6.1-Sol are listed.
  Model requests now identify the same way in their User-Agent and `version` header; a fresh
  ChatGPT-account session with GPT-6.1-Sol was refused while they still said `0.0.0`.

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
