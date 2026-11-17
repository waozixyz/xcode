The changelog can be found on the [releases page](https://github.com/openai/codex/releases).

## Unreleased

- Local builds are faster. `make` builds a `local` profile (no cross-crate LTO, 16 codegen units, incremental, no debug info) whose final link spreads over many cores and needs a fraction of the memory; published releases keep the optimized `release` profile (`XCODER_CARGO_PROFILE=release`, set in CI).
- `codex-protocol`, which nearly every crate depends on, no longer waits for the network proxy to compile: the policy values it shares with the proxy live in a small `codex-network-policy` crate. The proxy was the slowest crate in the build (about 140 s), so its compile time now overlaps the rest.
- Line diffs, unified diff text and unified-diff parsing come from the Ziran `diff` package (`native/diff.zi`, `codex-utils-text-diff`) instead of `similar` and `diffy`; the turn diff, `apply_patch` results, the git baseline and the TUI diff view use it. Output matches `similar` byte for byte and parsing matches `diffy` on generated and damaged patches. The 100 ms diff deadline is now a deterministic work budget.
- The private git baseline repository is written and read with the Ziran `git` package instead of `gix`, and `SanitizedGitUrl` removes remote credentials with the package's port of gix-url's rules. `gix` and `gix-url` (about 40 crates) are no longer in the build, only in tests: the `git` CLI checks the objects (`git fsck --strict`), and gix-url is the oracle for 200,000 generated remotes.
- Builds no longer recompile most of the workspace every time: the Ziran build scripts watched the optional `ziran.local.toml`, and Cargo reruns a script on every build while a watched file is missing. The override is now watched only while it exists; touch `ziran.toml` after adding one.
- Make builds no longer cap themselves at 2 GiB of RAM in their own `xcoder-compilation.scope`: a release build that needs more spent hours in swap. They still run at nice 19 and the lowest I/O priority; memory limits come from the surrounding build queue. `XCODER_BUILD_SCOPE` is gone.
- The TUI header, `/status`, the session banner of `xcoder exec`, analytics and turn metadata show the Codex version Xcoder tracks (at least 0.169.0, following the newest upstream release) instead of `v0.0.0`; `xcoder --version` still shows Xcoder's own number.
- Windows support is removed. Gone: the Windows sandbox crates (restricted-token, elevated and MXC backends, their setup service and `codex sandbox windows`/`uninstall`), PowerShell and `cmd` shell support (including the tree-sitter PowerShell grammar), Windows-only code paths, the `[windows]` config and managed requirements, Windows hook commands and managed hook directories, Windows computer-use app rules, the Windows sandbox app-server requests and TUI prompts, the Windows standalone updater, and the Windows CI and installer scripts. Old config keys for these features are ignored.
- `apply_patch` recognizes its heredoc form (`apply_patch <<'EOF' ... EOF`, optionally after `cd <path> &&`) with Ziran instead of a tree-sitter query; tree-sitter is now only a test dependency of apply-patch. Heredoc bodies end at the exact delimiter line, as in Bash, so a patch line such as `EOFX` no longer cuts the patch short.
- Dangerous-command detection finds the commands in a shell script with Ziran (`native/shell.zi`): every command in lists, pipelines, subshells, groups, if/while/until/for/case and function bodies, `$(...)`, backticks, process substitutions, double quotes and unquoted heredoc bodies. It never misses a dangerous command tree-sitter found in a script Bash would run (40,000 generated scripts, 1,005 real ones), still ignores scripts Bash rejects, and also catches dangerous commands in real scripts tree-sitter could not parse.
- The check that a shell script is only plain commands joined by `&&`, `||`, `;`, `|` and newlines (the proof exec policy and the safe-command lists rely on) runs in Ziran (`native/shell.zi`) instead of walking a tree-sitter parse. It is never looser than tree-sitter: on 50,000 generated scripts it accepts nothing tree-sitter rejects and returns the same words, and on 1,005 real agent commands the two agree exactly.
- Streaming an `apply_patch` call no longer clones and reformats the whole patch for every few characters the model sends: the progress snapshot is rebuilt only when a delta completes a line, and the patch parser reads deltas line by line instead of one character at a time.
- Rules files (`.rules` exec policies) are parsed in Ziran (`native/execpolicy.zi`) instead of by a Starlark interpreter. The declarative subset they use still works: `prefix_rule`, `network_rule` and `host_executable` calls, `NAME = value` assignments, strings with Starlark escapes, lists, tuples, `None`/`True`/`False`, names and `+`. Functions, loops, conditions, `load` and f-strings are reported as unsupported with their line. The Starlark interpreter and about 60 crates it pulled in are gone.
- `apply_patch` applies an update in Ziran from start to end: it splits the file into line offsets, finds every hunk, and writes the new contents once, keeping each line's ending, instead of copying every line into a string several times. A 20-hunk update of a 50,000-line file takes 3.2 ms instead of 7.7 ms; results and error messages match the Rust code on 3,000 generated patches.
- `apply_patch` finds where each hunk goes with a Ziran matcher (`native/apply_patch.zi`) that borrows the file's lines instead of copying them. Answers are identical to the Rust matcher it replaces (checked against it on thousands of generated cases); on a 50,000-line file the whitespace- and punctuation-tolerant searches and searches that find nothing are 4-5x faster, exact matches cost about 0.4 ms more. `make benchmark-apply-patch` measures it.
- Parallel tool cells no longer fail at random with "Text file busy": starting a freshly compiled cell retries while another thread's fork still holds the file.
- The upstream Codex version check runs on its own thread, so a slow network never delays Xcoder exiting; the offline local tests turn it off.
- GPT-6.2-Sol (`gpt-6.2-sol`) is in the bundled model table, first, so it is the default when no model is configured even if the live catalog cannot be fetched. The entry is the provider's own (500,000-token context, efforts low to ultra, freeform `apply_patch`, code-mode-only tools, the `ultrafast` service tier, audio input). Before, a session whose catalog request timed out (the request has 5 s, including DNS) started on GPT-6.1-Sol, or ran GPT-6.2-Sol with fallback metadata and the warning "Model metadata for `gpt-6.2-sol` not found".
- The `new-model` skill (`skills/new-model/SKILL.md`) lists the steps for adopting a new model, with the checks that save time: logins with quota and DNS speed first, builds only through `make`, real turns started as soon as the binary exists. `scripts/model-turns.py` runs the 12 real turns (a plain answer, a shell call and a patch edit at each effort). `scripts/install-xcoder-remote.sh omega|delta` installs the locally installed build on Omega or Delta, renaming each file into place.

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
