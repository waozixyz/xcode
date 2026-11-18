# Next Ziran ports (2026-11-15)

Goal: replace the remaining large Rust dependencies in Xcode with Ziran modules that
belong to the ziranlang org, so Kryon apps (notably the GitHub client at
`kryonlabs/github`) reuse them. Each port keeps the old Rust code as a `#[cfg(test)]`
oracle and compares outputs on generated and real inputs, as the `apply_patch`,
exec-policy and shell ports did.

Building blocks that already exist: `packages/crypto` (SHA-1, SHA-256), `packages/compression`
(pure-Ziran inflate), `std/zlib_linux.zi` (zlib bindings, deflate), `std/file_tree*.zi`,
`std/hash_map.zi`, `std/sort.zi`, `std/text*.zi`, `std/unicode*.zi`, `packages/syntax`
(token spans for Ziran, C and Make; spans match Kryon's `TextAreaColorSpan`).

## Ranked plan

| # | Rust crate(s) | Xcode users | New ziranlang module | Also useful for |
|---|---|---|---|---|
| 1 | `similar`, `diffy` | `core/src/turn_diff_tracker.rs`, `tui/src/diff_render.rs`, `apply-patch/src/file_update.rs`, `git-utils/src/baseline.rs` | `std/diff.zi`: Myers line diff (with patience anchors), unified-diff writer and parser | GitHub client patch view, any Kryon editor |
| 2 | `gix` (39 crates, ~12 MB rlib), `gix-url` | `git-utils/src/baseline.rs` (private snapshot repo: init, write tree, read HEAD tree, blob hashes, unified diff); `protocol/src/sanitized_git_url.rs` (URL parsing) | `packages/git`: object read/write (loose + pack, deltas), refs, index read, tree walk, commit write, URL parser | GitHub client offline history, blame-free diffs, local branch view |
| 3 | `syntect`, `onig` (C), `two-face` (~14 MB of rlibs, bundled grammars and themes loaded at startup) | `tui/src/render/highlight*.rs`, `markdown_render.rs`, `diff_render.rs`, themes | extend `packages/syntax` with Rust, Python, JS/TS, shell, JSON, TOML, YAML, Markdown, diff, Go, C++ | GitHub client source view, Kryon TextArea |
| 4 | `tree-sitter`, `tree-sitter-bash` | `shell-command/src/shell_snapshot_literals.rs`, `shell_snapshot_capture.rs` (zsh tied arrays) | extend Xcode `native/shell.zi` | Xcode only |
| 5 | `serde_yaml` (deprecated) | `skills/src/parser.rs`, `skills/src/metadata.rs`, `core-plugins`, `external-agent-migration` | `std/yaml_document.zi` (block maps, sequences, scalars, quoting; the frontmatter subset) | any config/frontmatter reader |
| 6 | `pulldown-cmark` | TUI markdown rendering, copy mode, streaming | `packages/markdown`: CommonMark-subset event parser, streaming-safe | GitHub client README and comment rendering |
| 7 | `nucleo`, `nucleo-matcher` | `file-search/src/lib.rs`, `snapshot.rs` | finish Xcode `native/fuzzy.zi`, then lift into `std/fuzzy.zi` | GitHub client repository and file search |

## Build speed (owner, 2026-11-15: "focus on having faster builds")

Measured on Alpha: `codex-cli` builds 939 crates on Linux. Two crates dominate the critical
path: `codex-tui` (~500k lines) and `codex-core` (~270k lines). Rust compiles each mostly on
one thread, and a release build of each needs 2-3 GiB. The release profile is thin LTO with 4
codegen units.

Findings, 2026-11-15:
- Done: Ziran build scripts no longer watch the missing `ziran.local.toml`. Cargo reruns a script
  while a watched file is missing, so every build recompiled most of the workspace.
- Done: the Xcode build wrapper no longer caps builds at 2 GiB RAM. A release build needs ~6 GiB
  for the final link alone, so it spent hours in swap.
- Dependencies compile once per profile and stay cached. What a rebuild pays for is the
  workspace crates above the change, plus the final link.
- `aws-lc-rs` stays. `rama-tls-rustls` hard-codes it, and Xcode installs it deliberately because
  `ring` cannot verify ECDSA P-521 certificates, which enterprise TLS proxies use.

1. **Local install profile** (done: `[profile.local]`, `XCODE_CARGO_PROFILE`). `make all`/`make install` build a `local` profile (no LTO, 16
   codegen units, no debug info). The GitHub release keeps the full `release` profile.
   Measure wall time and binary size before and after.
2. **Drop C-built and unused dependencies.**
   - `aws-lc-rs`: pick one TLS crypto provider; `ring` is already present.
   - `opentelemetry`/`tonic`/`prost` export: check whether Xcode needs OTLP at all.
   - `sqlx` macros: check whether the state stores can use plain `rusqlite` or Ziran's `sqlite` package.
   - `zstd`: check what still needs it.
3. **Ziran ports that remove whole dependency trees.**
   - `gix`: 43 crates. Ported, pending.
   - `syntect` + `onig` + `two-face`.
   - `tree-sitter`.
   - `serde_yaml`.
   - `pulldown-cmark`.
   - `nucleo`.
4. **Shrink the two giant crates.** Move self-contained TUI and core subsystems into Ziran
   modules or separate crates, so the slowest crates compile less and the rest compile in parallel.

## Why this order

1. **Diff first.** It's small, self-contained and easy to test against `similar` exactly. Git's baseline diff and the TUI both need it, so it's a prerequisite for 2.
2. **Git next.** Xcode needs only a narrow slice: an isolated snapshot repository with loose objects. Start there (SHA-1, zlib, tree/blob/commit encoding, HEAD), replacing `gix` in `git-utils` without packfile support. Packfile and index reading come second; the GitHub client needs those to read real clones. URL sanitizing is a separate small parser.
3. **Syntax highlighting.** Biggest binary and memory win, but the most behavior to reproduce. Port language by language, comparing token classes against syntect on corpora; keep syntect as a fallback for unported languages until coverage is complete.
4. **tree-sitter last bits.** Removes the last C grammar from the binary.
5–7. Smaller cleanups with clear oracles.

## First increment: `std/diff.zi`

- API: `DiffLines(old, new, ops: []DiffOp) -> DiffResult` (ops are equal/insert/delete runs with
  line ranges, no allocation by the caller beyond the op buffer), `WriteUnified(old, new, ops,
  context, output) -> s64`, `ParseUnified(text, hunks)`.
- Xcode adapter: `codex-rs/utils/diff` crate (new) wrapping the FFI like `seek_sequence.rs`;
  switch `core/src/turn_diff_tracker.rs` and `git-utils/src/baseline.rs` first, then
  `tui/src/diff_render.rs` (parsing) and `apply-patch` (unified output).
- Tests: generated line edits (insert/delete/replace/move, CRLF, missing final newline,
  empty files, 50k-line files) compared against `similar::TextDiff` unified output byte for byte;
  `#[ignore]` benchmark against `similar`.

## Measurements to take before each port

Size of each crate's code in the final `xcode` binary (`cargo bloat`-style or symbol sizes
from the linked binary), startup RSS with and without the feature (syntect's syntax sets are
loaded lazily or eagerly?), and per-call time on realistic inputs.
