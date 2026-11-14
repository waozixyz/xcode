# Exec Ziran scripts without bwrap

Owner, 2026-11-13: "we should not nede bwrap", "or fix it prorely please",
"plesa enalyze and fulyl fix it this ihigh prority and bencamrk it".
Owner, later: "do notu sue subtasks leran what they are donig and then fulyl take over".
The Ziran coordinator session finishes this itself; the Codex worker that wrote it is closed.

## Failure

In Codex sessions the `exec` tool's Ziran-script mode fails before running anything:
`Zi2c compilation failed: bwrap: Failed to make / slave: Operation not permitted`
(`codex-rs/code-mode-runtime/src/runtime/cache.rs`). The JSON tool-plan mode still works, so
agents degrade silently. Counted in `~/.codex/sessions`: 0 of 167 sessions on 2026-11-11,
5 of 80 on 2026-11-12, 40 of 52 on 2026-11-13.

Cause found by the research pass (reproduced host-wide, not yet re-checked by this session):
the kernel upgrade to 7.2.12 broke mount propagation setup in unprivileged user and mount
namespaces, so `bwrap --ro-bind / / true` fails everywhere on Omega. Harmony's agent launch
did not change.

## Change (uncommitted, in the working tree on Omega and Alpha, same hashes)

`native_sandbox.rs` no longer spawns bwrap. The compile step and the cell run keep their
rlimits, scrubbed environment and private scratch directory, and add:
- Landlock rules for the filesystem (read-only toolchain and system libraries, writes only in
  the cell directory) and for the network (no bind or connect);
- the existing seccomp cell filter (no fork, mount, unshare, setns or ptrace).

Alpha's kernel has Landlock ABI 8, which cannot restrict pathname UNIX-socket connects. So cell
callbacks moved to four inherited, pre-connected channels, and socket creation is denied.

Files: `codex-rs/code-mode-runtime/{Cargo.toml,build.rs,src/native_sandbox.rs,
src/native_dispatch.rs,src/runtime/mod.rs,src/runtime/cache.rs,src/runtime/plan.rs}`,
`native/runtime.zi`, `tests/native_cell_test.py`, `tests/native_host_test.py`,
`scripts/benchmark-runtime.py`.

## Status

| Step | State |
|------|-------|
| Replacement written | done |
| Omega: 64 runtime tests, before the channel redesign | passed (worker's run) |
| Alpha: runtime crate compiles after the channel redesign | compiled, no errors |
| Alpha: full release build | running, started 2026-11-13 20:48 by the Ziran session, detached; log `build/bwrap-build.log` on Alpha |
| Alpha: full suite (`make test`) | not run |
| Regression test for the nested-namespace case | not written |
| Benchmark: cold, warm, JSON plan; 20+ runs; median and p95; old vs new | not run |
| Package back to Omega, install, live exec-script proof | not done |
| Commit and push | not done |

Builds run on Alpha only (owner's routing for xcoder). The owner's xcoder session is running
memory benchmarks on Alpha, so the release build waits for a free hour there. Any release
build of this tree includes the change above, whose channel redesign has not run the suite.
