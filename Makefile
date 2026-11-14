LOCAL_RUST_BIN := $(CURDIR)/build/toolchain/bin
CARGO ?= $(if $(wildcard $(LOCAL_RUST_BIN)/cargo),$(LOCAL_RUST_BIN)/cargo,cargo)
RUSTC ?= $(if $(wildcard $(LOCAL_RUST_BIN)/rustc),$(LOCAL_RUST_BIN)/rustc,rustc)
ZIRAN ?= ziran
export CARGO_HOME := $(CURDIR)/build/cargo-home
export CARGO_TARGET_DIR := $(CURDIR)/codex-rs/target
export XCODER_RUNTIME_DIR := $(CURDIR)/build/native-runtime
export RUSTC ZIRAN
export CARGO_BUILD_JOBS ?= 1
# UI plugins retain their separate JavaScript sandbox; tool execution is native.
UI_FEATURES ?=
CARGO_FEATURE_FLAGS := $(if $(UI_FEATURES),--features $(UI_FEATURES),)
BACKGROUND_BUILD := python3 scripts/low-priority-build.py

.PHONY: all native-test native-test-build prebuild-tests test runtime-check install
all:
	mkdir -p build/bin
	$(BACKGROUND_BUILD) $(CARGO) build --manifest-path codex-rs/Cargo.toml --release -p codex-cli -p codex-code-mode-host $(CARGO_FEATURE_FLAGS)
	python3 scripts/package-xcoder.py

native-test-build:
	mkdir -p build/native-dispatch
	$(ZIRAN) build --project --target=c -o build/native-dispatch native/dispatch.zi
	$(CC) -O3 -std=c99 -c build/native-dispatch/dispatch.c -I build/native-dispatch -o build/native-dispatch/dispatch.o
	$(AR) rcs build/native-dispatch/libxcoder_dispatch.a build/native-dispatch/dispatch.o
	$(BACKGROUND_BUILD) $(RUSTC) --edition=2024 --test tests/native_abi.rs -L build/native-dispatch -l static=xcoder_dispatch -o build/native-dispatch/native-abi-test
	$(BACKGROUND_BUILD) $(RUSTC) --edition=2024 --test tests/native_allocator.rs -o build/native-dispatch/native-allocator-test
	rm -rf build/native-idle && mkdir -p build/native-idle
	$(ZIRAN) build --project --target=c -o build/native-idle native/idle.zi
	$(CC) -O3 -std=c99 -c build/native-idle/idle.c -I build/native-idle -o build/native-idle/idle.o
	$(AR) rcs build/native-idle/libxcoder_idle.a build/native-idle/idle.o
	$(BACKGROUND_BUILD) $(RUSTC) --edition=2024 --test tests/native_idle.rs -L build/native-idle -l static=xcoder_idle -o build/native-dispatch/native-idle-test

native-test: native-test-build
	build/native-dispatch/native-abi-test
	build/native-dispatch/native-allocator-test
	build/native-dispatch/native-idle-test
	XCODER_ZIRAN_TOOLCHAIN="$$($(ZIRAN) pkg path ziran)" python3 tests/native_cell_test.py

prebuild-tests: native-test-build
	$(BACKGROUND_BUILD) python3 scripts/prebuild-tests.py --cargo "$(CARGO)"

runtime-check:
	$(BACKGROUND_BUILD) $(CARGO) check --manifest-path codex-rs/Cargo.toml -p codex-code-mode-runtime

test:
	$(BACKGROUND_BUILD) python3 scripts/test-xcoder.py --cargo "$(CARGO)" --ui-features "$(UI_FEATURES)"

install:
	python3 scripts/install-xcoder.py

JAVASCRIPT_HOST ?= $(HOME)/.codex/packages/standalone/current/bin/codex-code-mode-host.javascript
JAVASCRIPT_CLI ?= $(HOME)/.codex/packages/standalone/current/bin/codex-upstream
.PHONY: benchmark-source benchmark-plan benchmark-fuzzy benchmark-memory
benchmark-json:
	python3 scripts/benchmark-runtime.py --native-host build/bin/codex-code-mode-host --javascript-host "$(JAVASCRIPT_HOST)" --diagnose-json
benchmark-source:
	python3 scripts/benchmark-runtime.py --native-host build/bin/codex-code-mode-host --native-cli build/bin/xcoder --javascript-host "$(JAVASCRIPT_HOST)" --javascript-cli "$(JAVASCRIPT_CLI)" --native-mode source
benchmark-plan:
	python3 scripts/benchmark-runtime.py --native-host build/bin/codex-code-mode-host --native-cli build/bin/xcoder --javascript-host "$(JAVASCRIPT_HOST)" --javascript-cli "$(JAVASCRIPT_CLI)" --native-mode plan
# A long offline session: memory per tool call, the idle footprint, and the
# footprint and wake time after the idle release.
benchmark-memory:
	python3 scripts/benchmark-memory.py --cli build/bin/xcoder --calls 150 --idle-seconds 5 --release-wait 30 --environment XCODER_IDLE_RELEASE_SECONDS=15 --output build/memory/benchmark.json
benchmark-fuzzy:
	$(CARGO) run --manifest-path codex-rs/Cargo.toml --release -p codex-utils-fuzzy-match --example fuzzy_bench

.PHONY: mermaid-test
MERMAID_LOCKED := $(if $(wildcard ziran.local.toml),,--locked)
mermaid-test:
	$(MAKE) -C "$$($(ZIRAN) pkg path mermaid $(MERMAID_LOCKED))" test ZIRAN="$(ZIRAN)"
	$(BACKGROUND_BUILD) $(CARGO) test --manifest-path codex-rs/Cargo.toml -p codex-mermaid --lib
