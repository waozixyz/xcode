# Xcode session memory

Measured on 2026-11-13 on `alpha` (physical host, Linux x86-64, 15 GiB RAM)
with `scripts/benchmark-memory.py`. The benchmark starts the interactive CLI
in a private terminal against a local model fixture, types one prompt, and
answers it with a chain of real shell tool calls that each print 30 KB. No
network or account is used. Sizes are resident memory of the CLI process.

## Where the memory goes

A session that has just started and is waiting for input holds about 195 MB:

| Part | Resident | Kind |
| --- | ---: | --- |
| Program code | 145 MB | file pages, shared between sessions of one binary |
| Program tables and relocated data | 34 MB | 26 MB file pages, 8 MB private |
| Heap and allocator arenas | 36 MB | private |
| Shared libraries | 9 MB | file pages |

From there it grows with the conversation, and it never shrank:

| Tool calls (30 KB each) | Raw output | Resident | Private memory |
| ---: | ---: | ---: | ---: |
| 0 | 0 | 195 MB | 36 MB |
| 150 | 4.5 MB | 283 MB | 104 MB |
| 600 | 18 MB | 428 MB | 250 MB |

Private memory grows about eleven times faster than the raw tool output. A
working day's session with 60–80 MB of output reaches a gigabyte, and keeps
it while it waits. Three things make up that factor.

1. **Three live copies of every command's output.** The live-allocation
   tracer (`scripts/memory/alloc-trace.c`) attributes them to the model
   history, the terminal's command cell, and a second clone in the terminal's
   replay buffer, which holds up to 32,768 events regardless of their size.
2. **Freed space the allocator keeps.** After 150 calls the allocator holds
   77 MB of heap, of which 38 MB is free. Each request serializes the whole
   history into one buffer; those megabyte-sized buffers are freed but leave
   the heap fragmented and unreturned.
3. **Caches and fixed buffers** of a few megabytes each: four SQLite page
   caches (about 9 MB together) and a 4 MB feedback log ring.

The program itself is the largest single part at startup. The binary carries
237 MB of code: 64 MB is drop glue, 54 MB async state machines and closures,
39 MB serde. About 54 MB of the drop glue is the same functions repeated in
each of the 256 code-generation units of the terminal crate, which is built
unoptimized with 256 units to fit constrained builders
(`[profile.release.package.codex-tui]`).

## What changed

**Idle sessions release their memory** (`native/idle.zi`, started from
`native/idle.rs`). A watcher thread samples the process's processor time.
After two minutes below two percent of one processor it runs one pass:

1. return the allocator's free pages;
2. read `/proc/self/smaps` with a fixed buffer and no allocation;
3. unmap private file mappings whose resident pages are all unmodified (code
   and read-only tables). They stay in the page cache and come back on use,
   whichever other sessions share them;
4. ask the kernel to move every other private mapping to swap.

Nothing is discarded. The next keypress or event pages back what it touches.
The pass runs again only after the session has worked, or has grown by 32 MB
while still idle. Without free swap space, steps 1 and 3 still apply and
private memory stays resident.

`XCODE_IDLE_RELEASE_SECONDS` sets the idle period; `0` disables the watcher.

**The replay buffer no longer doubles command output.** The copy of a
finished command kept for replay holds the first and last 8 KiB of output
above 16 KiB, with the number of omitted lines between them. The live cell
keeps the full output, so the transcript of the current view is unchanged; a
view rebuilt after switching threads shows the shortened form for large
outputs.

## Results

Build of 2026-11-13 with the watcher linked in (`build/bin/xcode`), 15-second
idle period for the run:

| Session | Idle before release | After release | In swap | Next keypress |
| --- | ---: | ---: | ---: | ---: |
| 150 calls of 30 KB | 301 MB | 28 MB | 76 MB | 44 ms |
| 600 calls of 30 KB | 415 MB | 23 MB | 141 MB | 79 ms |

Before the change the same keypress took 46 ms, and the session kept its full
size for as long as it stayed open.

Replay-buffer cap, 100 calls of 200 KB each, release disabled:

| Build | Private memory at idle | Copies of each output in memory |
| --- | ---: | ---: |
| Installed build of 2026-11-13 | 155 MB | 9 |
| This build | 111 MB | 6 |

With 30 KB outputs the cap saves little (104 MB to 101 MB at 150 calls),
because half of each output still fits the budget.

`tests/native_idle.rs` checks that a 64 MiB buffer survives a release
unchanged (70 MB resident to 0.3 MB) and that the watcher runs once per idle
period.

## Measuring

```sh
make benchmark-memory          # 150 calls, idle footprint, release, wake time
python3 scripts/benchmark-memory.py --cli build/bin/xcode --calls 600 \
    --release-wait 30 --environment XCODE_IDLE_RELEASE_SECONDS=15
```

The report gives resident memory after each call, the idle footprint by
mapping kind, the number of copies of each output left in memory, and, with
`--release-wait`, the footprint after the idle release and the time for the
next keypress to appear.

To attribute retained heap to code, build the tracer and symbolize its report
against an unstripped binary of the same build:

```sh
cc -O2 -fPIC -shared -o build/memory/alloc-trace.so scripts/memory/alloc-trace.c
python3 scripts/benchmark-memory.py --cli build/bin/xcode --calls 150 \
    --trace-library build/memory/alloc-trace.so --environment MALLOC_ARENA_MAX=2
python3 scripts/memory/alloc-report.py build/memory/alloc-trace.txt \
    --symbols codex-rs/target/release/xcode
```

The sandboxed shell tool cannot start inside an already confined shell, so
the benchmark launches its offline session with the sandbox bypassed; the
fixture only runs `printf` and `head`.

## Not done yet

- The history is still serialized whole for every request. Building the
  request without one contiguous buffer would remove most of the freed-space
  retention during work.
- The terminal crate's 256 unoptimized code-generation units cost about 54 MB
  of repeated code. Fewer units need a builder that can afford them.
- The command cell and the model history each keep their own copy of large
  output.
