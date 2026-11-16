## Xcoder

For automatic Linux builds and downloads, see
[Xcoder Linux](https://github.com/waozixyz/xcoder/actions/workflows/xcoder-linux.yml).

Xcoder is Waozi's source fork of OpenAI Codex, under the original Apache 2.0
license. Tool orchestration uses Ziran compiled through Zi2c. Ordinary plans run
through an executor linked into Rust, without a compiler or child process.
Custom Ziran cells compile on a cache miss and reuse private cached executables.
The Rust session
and approval protocols remain the authority for host operations. JavaScript
tool cells have no execution path in this fork. Sessions share the Rust process;
only custom native cells start sandboxed children. Ordinary tool calls do not
start a separate code-mode host. The optional JavaScript UI plugin renderer is
excluded from the default build (`UI_FEATURES=codex-code-mode-runtime/ui` enables
it when the corresponding upstream V8 artifacts are available).

The initial native backend targets Linux with bubblewrap, a C compiler, Rust
1.95 and a built Ziran toolchain. `ziran lock` resolves the pinned toolchain;
local development uses an ignored `ziran.local.toml` override.
The Make targets run Rust work at nice 19 and the lowest best-effort I/O priority, with one
Cargo job by default (set `CARGO_BUILD_JOBS` to use more). They add no memory limit of their
own; run them under a build queue or scope that sizes memory for the machine. Completed
artifacts are reused when a build is resumed.

Run `make test` for Rust/C ABI, Ziran-backed Mermaid and fuzzy-match, native
sandbox, callback, host transport and offline full-CLI local tests. The CLI fixture runs a real shell tool through the
embedded supervisor without contacting an external model service.
`make` builds the CLI at `build/bin/xcoder` with its adjacent tool host.
`make test` writes an installation receipt only after the full local suite passes;
a failed rerun removes the earlier receipt. `make install` verifies the exact binaries and preserves
Harmony's existing launcher, accounts and histories. It replaces the launcher's
Codex executable for new invocations; running sessions are not restarted.
Local interactive launches use the embedded server so an older stock Codex
daemon cannot supply their tool runtime. Explicit remote endpoints keep their
normal behavior.
Update this fork from its source with `make test` and `make install`; the stock
Codex package updater cannot replace the installed native fork.

Prefer a JSON tool plan for ordinary calls:

```json
{"calls":[{"tool":"exec_command","input":{"cmd":"pwd"}}]}
```

Plans validate structure, tool names and freeform input types before execution;
each tool validates its own arguments. Results include the call index and tool
name. Add `"parallel":true` for up to four independent calls at
once. A plan supports up to 128 calls; dependent steps use a later cell or custom
code. This path works without the compiler installed.

Custom native cells import `xcoder/runtime`. `Call(name, json)` returns caller-owned
storage: inspect `Body(*reply)` and release it with `defer Release(*reply)`.
`Text`, `Image`, `Audio`, `Notify`, `Store`, `Load`, `Catalog` and `Yield` provide
output and session state. `CallBatch` runs arrays of `BatchCall` on up to four
explicit threads, then joins them; release each result after use. A native cell has only its executable and callback
socket; the compiler has only its binaries, headers, standard library and cell
sources. Tool errors never fall back to JavaScript.

The custom-cell cache holds at most 64 executables and 256 MiB on disk.
Files default to `~/.local/state/xcoder/runtime/cache`; an explicit
`XCODER_RUNTIME_DIR` keeps its selected location. Cache keys include source,
the runtime API, sandbox/compiler settings, toolchain inputs and the C compiler.
Executables are verified before reuse; corruption triggers recompilation.

`scripts/benchmark-runtime.py --help` describes the offline comparison with
preserved JavaScript binaries. It records raw samples, hashes, CPU time,
process-tree proportional RAM and latency under ignored `backup/runtime-benchmark/`.
Use `--native-mode source` for custom cells and `--native-mode plan` for ordinary
calls. Compilation and sandbox startup are included; model inference is excluded.

The fork retains upstream documentation below.

---

<p align="center"><strong>Codex CLI</strong> is a coding agent from OpenAI that runs locally on your computer.
<p align="center">
  <img src="https://github.com/openai/codex/blob/main/.github/codex-cli-splash.png" alt="Codex CLI splash" width="80%" />
</p>
</br>
If you want Codex in your code editor (VS Code, Cursor, Windsurf), <a href="https://developers.openai.com/codex/ide">install in your IDE.</a>
</br>If you want the desktop app experience, run <code>codex app</code> or visit <a href="https://chatgpt.com/codex?app-landing-page=true">the Codex App page</a>.
</br>If you are looking for the <em>cloud-based agent</em> from OpenAI, <strong>Codex Web</strong>, go to <a href="https://chatgpt.com/codex">chatgpt.com/codex</a>.</p>

---

## Quickstart

### Installing and running Codex CLI

Run the following on Mac or Linux to install Codex CLI:

```shell
curl -fsSL https://chatgpt.com/codex/install.sh | sh
```

The standalone installers download from `https://releases.openai.com/codex` by default and fall back to GitHub Releases if a metadata or asset download is unavailable. To force GitHub Releases, set `CODEX_INSTALLER_USE_RELEASES_OPENAI_COM` to `false` (`0` and `no` are also accepted):

```shell
curl -fsSL https://chatgpt.com/codex/install.sh | CODEX_INSTALLER_USE_RELEASES_OPENAI_COM=false sh
```

Codex CLI can also be installed via the following package managers:

```shell
# Install using npm
npm install -g @openai/codex
```

```shell
# Install using Homebrew
brew install --cask codex
```

Then simply run `codex` to get started.

<details>
<summary>You can also go to the <a href="https://github.com/openai/codex/releases/latest">latest GitHub Release</a> and download the appropriate binary for your platform.</summary>

Each GitHub Release contains many executables, but in practice, you likely want one of these:

- macOS
  - Apple Silicon/arm64: `codex-aarch64-apple-darwin.tar.gz`
  - x86_64 (older Mac hardware): `codex-x86_64-apple-darwin.tar.gz`
- Linux
  - x86_64: `codex-x86_64-unknown-linux-musl.tar.gz`
  - arm64: `codex-aarch64-unknown-linux-musl.tar.gz`

Each archive contains a single entry with the platform baked into the name (e.g., `codex-x86_64-unknown-linux-musl`), so you likely want to rename it to `codex` after extracting it.

</details>

### Using Codex with your ChatGPT plan

Run `codex` and select **Sign in with ChatGPT**. We recommend signing into your ChatGPT account to use Codex as part of your Plus, Pro, Business, Edu, or Enterprise plan. [Learn more about what's included in your ChatGPT plan](https://help.openai.com/en/articles/11369540-codex-in-chatgpt).

You can also use Codex with an API key, but this requires [additional setup](https://developers.openai.com/codex/auth#sign-in-with-an-api-key).

## Docs

- [**Codex Documentation**](https://developers.openai.com/codex)
- [**Contributing**](./docs/contributing.md)
- [**Installing & building**](./docs/install.md)
- [**Open source fund**](./docs/open-source-fund.md)

This repository is licensed under the [Apache-2.0 License](LICENSE).
