# Mermaid migration: local performance and validation

Measured on 2026-11-12 on `omega`, Linux x86-64. The Ziran package and Rust
adapter rendered all measured inputs identically to the original Rust
renderer. These are component results; complete Xcode
performance and production integration remain unverified.

The standalone package lives in `ziranlang/packages/mermaid`. Xcode consumes
its exported module through Ziran's package resolver. Ordinary labels use
48 inline bytes with owned overflow for longer labels. Layout borrows label
lines, and the native adapter transfers an owned Unicode cell grid to Rust.
Rust constructs text and semantic spans directly from those cells.

## Performance

Ratios are Rust median divided by Ziran median wall time; values above one
favor Ziran. CPU time closely follows wall time in this run.

| Input | Text | Semantic spans |
| --- | ---: | ---: |
| Small flowchart | 2.25× | 1.03× |
| Flowchart with Unicode and branches | 2.03× | 1.19× |
| Sequence with fragments | 2.14× | 1.08× |
| State transitions | 2.88× | 1.37× |
| Classes and members | 2.78× | 1.36× |
| Entity relationships | 2.88× | 1.33× |
| Large multiline canvas | 4.40× | 1.55× |

The six ordinary inputs use nine samples of 50,000 calls per implementation
and API. The large input uses nine samples of 1,000 calls: two nodes, one
40-column label and one label containing 1,300 `x<br>` sequences. Its text
path has median process peak RSS of 3,292 KiB for Rust and 2,736 KiB for
Ziran; spans use 3,316 KiB and 3,200 KiB respectively. Small-input RSS varies
in both directions, so these results do not establish a general memory
improvement. The smallest span gain is narrow and should be remeasured on
the deployment host.

Both implementations run in fresh processes of the same release binary.
Each input first verifies full output equality, warms both implementations,
and alternates execution order across samples. Measurements include process
wall time, child user/system CPU time, process peak RSS from `/proc`, and
internal render time. Symmetric 1.5×IQR filtering uses inclusive interpolated
quartiles independently per metric. Raw samples, exclusions, source hashes
and the benchmark executable hash are retained in
[mermaid-performance.json](mermaid-performance.json).

The measured reference was the original Rust renderer in
[`codex-rs/mermaid/src`](https://git.waozi.xyz/waozixyz/xcode/src/commit/ba9d4a9e84bd34d9f2915fca36c9333a0eee8951/codex-rs/mermaid/src),
compiled unchanged into the benchmark executable. After these measurements
the duplicated Rust renderer and its benchmark harness were removed; parsing
and layout now exist only in the Ziran package. The raw samples and hashes in
the JSON report remain the evidence for these ratios.

## Local validation and remaining work

- Standalone C, Go and portable VM local tests passed on the latest package.
- AddressSanitizer and UndefinedBehaviorSanitizer passed with leak detection
  on the latest package's standalone tests.
- All 28 Rust local tests passed when rerunning the existing test executable,
  built after the latest package source change. These include original
  snapshots, exhaustive three-node edge routing, generated width boundaries,
  Unicode sampling, long labels, storage transitions and malformed inputs.
- The last retained upstream Ziran full-suite run reported 269/273 local
  tests passing. Its remaining failures are `direct_assembly.sh`,
  `explain.sh`, `site_pages.sh` and `site_targets.sh`; the logs remain under
  Ziran's `build/check-logs`. This full suite was not rerun for this report.
  The focused vector suite and bootstrap local tests passed in that run.

Full Xcode local tests and its production build are blocked: the required
`xcode-compilation.scope` cannot be created because the user-systemd bus
is unavailable. A fresh `build-queue -- python3
scripts/low-priority-build.py true` reproduces the missing-bus error. The
memory scope has not been disabled. An earlier desktop `make test` request
timed out without a command ID; its outcome remains unknown. Harmony session
and context requests currently return HTTP 403, preventing native-host
verification and routing of the bridge issue to its owning task.

The new package repository remains uncommitted and unpublished. Its pinned
published toolchain and Xcode's development dependency lock still require
finalization and locked-consumer validation after the upstream prerequisite
is integrated. Full application validation is required before committing or
pushing this feature set. No release or installation has been performed.
