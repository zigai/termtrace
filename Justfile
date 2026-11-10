INTEGRATION_FILTER := "test(answers_and_audits_terminal_queries) | test(attached_mirror_suppresses_answered_split_queries_and_preserves_display_sequences) | test(completed_output_burst_refreshes_the_pty_drain_deadline) | test(control_server_accepts_a_second_request_while_the_first_is_pending) | test(fatal_actor_exit_writes_durable_failure_completion) | test(graceful_control_shutdown_drains_an_inflight_request) | test(idle_timeout_limit_terminates_session) | test(incomplete_request_read_is_interrupted_promptly_on_shutdown) | test(max_duration_limit_terminates_session) | test(output_budget_retains_observed_bytes_and_records_termination) | test(poisoned_terminal_exit_append_preserves_incomplete_failure_without_completion) | test(preserve_policy_keeps_the_application_alive_after_a_mirror_failure) | test(preserves_complete_osc_query_when_response_precedes_st) | test(records_exact_output_and_exit_after_output_close) | test(spawn_and_provenance_share_the_effective_child_path) | test(spawn_failure_after_manifest_creation_writes_valid_fatal_closure) | test(terminates_descendants_that_retain_the_slave_pty)"

# Install repository dependencies
setup:
    @rustup component add clippy rustfmt
    @cargo fetch
    @scripts/bootstrap.sh

# Build a debug version
build: setup
    cargo build --locked

# Install Termtrace from this checkout
install: setup && (report-install home_directory() / ".local/bin/termtrace")
    TERMTRACE_BUILD_VERSION="$(git describe --tags --dirty --always)" cargo install --locked --path . --force --root "$HOME/.local"

# Regenerate public schemas and the generated Python and TypeScript SDK sources
generate:
    cargo run --locked --quiet -- schema --out schemas
    cargo run --locked --quiet -p termtrace-python --bin generate
    cargo run --locked --quiet -p termtrace-node --bin generate
    uv run --project crates/bindings-python ruff check --fix --quiet crates/bindings-python/python/termtrace/types.py crates/bindings-python/python/termtrace/aio.py crates/bindings-python/python/termtrace/_termtrace.pyi
    uv run --project crates/bindings-python ruff format crates/bindings-python/python/termtrace/types.py crates/bindings-python/python/termtrace/aio.py crates/bindings-python/python/termtrace/_termtrace.pyi

# Enforce the complete allowlist of direct internal Cargo dependencies
architecture:
    cargo test --locked --test architecture

# Compile every Rust target and feature without running tests
validate:
    cargo check --locked --workspace --all-targets --all-features

# Run fast Rust and tooling unit tests; filter Rust with arguments
[positional-arguments]
test *args:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo nextest run --locked --workspace --lib --bins --test execution_lifecycle --test architecture "$@"
    if (( $# == 0 )); then cargo test --locked -p termtrace-sdk --no-default-features; fi
    if (( $# == 0 )); then PYTHONPATH=benchmarks uv run --project benchmarks python3 -m unittest benchmarks/test_acceptance.py; fi
    if (( $# == 0 )); then bash tests/install.sh; fi

# Run CLI/tmux integration on each pull request (requires bootstrap, tmux, and Ghostty on PATH)
integration:
    cargo test --locked --test cli
    scripts/test-rust-consumer.sh
    cargo nextest run --locked --workspace --lib --bins --test execution_lifecycle --test architecture --ignore-default-filter -E '{{ INTEGRATION_FILTER }}'

# Build native Python and Node.js language bindings
bindings-build: build-python build-node

# Build the native Python binding into its project environment
build-python:
    uv run --project crates/bindings-python --with maturin maturin develop --manifest-path crates/bindings-python/Cargo.toml

# Build the native Node.js binding
build-node:
    bunx @napi-rs/cli build --cwd crates/bindings-node

# Test native Python language bindings and pytest consumer suite
test-python:
    cargo build --locked --bin termtrace
    TERMTRACE_BIN={{ justfile_directory() }}/target/debug/termtrace uv run --project tests/consumers/python_native pytest -v tests/consumers/python_native

# Test native Node.js/TypeScript bindings and consumer suite
test-node:
    cargo build --locked --bin termtrace
    node --test crates/bindings-node/test/*.test.js
    TERMTRACE_BIN={{ justfile_directory() }}/target/debug/termtrace npm --prefix tests/consumers/node_native test

# Type-check and lint the native Python bindings
bindings-lint:
    uv run --project crates/bindings-python ruff check crates/bindings-python/python/termtrace
    uv run --project crates/bindings-python mypy crates/bindings-python/python/termtrace

# Test all native language bindings
bindings-test: bindings-build bindings-lint test-python test-node

# Run every configured linter on the whole project, stopping on failure
lint:
    @cargo clippy --locked --workspace --all-targets --all-features
    @cargo clippy --locked -p termtrace-sdk --no-default-features --all-targets
    @cargo machete
    @set -e; for file in install.sh benchmarks/*.sh scripts/*.sh; do bash -n "$file"; done
    @bash scripts/check-github-action-pins.sh
    @uv run --project crates/bindings-python --group dev ruff check --extend-exclude vendor/ghostty .

# Format every supported first-party file type
format:
    @ruff format --extend-exclude '*.md,vendor/ghostty' .
    @cargo statement-spacing fix --format-first
    @git ls-files -co --exclude-standard -z | while IFS= read -r -d '' file; do if [[ -f $file ]]; then printf '%s\0' "$file"; fi; done | xargs -0 -r oxfmt --no-error-on-unmatched-pattern '!**/*.md' '!schemas/**' '!**/package.json' '!crates/bindings-node/types.d.ts' '!**/fixtures/**' '!**/snapshots/**' '!**/corpus/**' '!**/package-lock.json' '!crates/bindings-node/dts-header.d.ts' '!crates/bindings-node/index.d.ts' '!**/*-lock.json'
    @git ls-files -co --exclude-standard -z | while IFS= read -r -d '' file; do if [[ -f $file ]]; then printf '%s\0' "$file"; fi; done | xargs -0 -r shfmt -f | xargs -r -d '\n' shfmt -w
    @git ls-files -co --exclude-standard -- Justfile justfile "*.just" | while IFS= read -r file; do just --justfile "$file" --fmt; done
    @just markdown fmt --exclude vendor/ghostty

# Run the complete native recording, replay, video, and screenshot verification test
native-check:
    cargo test --locked --test cli native_capture:: -- --ignored --test-threads=1

# Exercise native capability failures, retained export, and original capture (requires Ghostty, Xvfb, dbus-run-session, ImageMagick, and FFmpeg)
native-test:
    cargo test --locked --test cli native_capabilities:: -- --ignored --test-threads=1
    cargo test --locked --test cli retained_native_video_exports_without_native_capture_tools -- --ignored
    cargo test --locked --test cli top_level_video_auto_captures_native_and_overwrites_output -- --ignored
    cargo test --locked --test cli original_native -- --ignored
    cargo test --locked -p termtrace-service --test original_recording -- --ignored --test-threads=1
    TERMTRACE_BIN={{ justfile_directory() }}/target/debug/termtrace cargo test --locked --manifest-path tests/consumers/rust/Cargo.toml --features local --tests -- --ignored --test-threads=1

# Run performance and resource benchmarks on demand (requires native capture tooling)
bench:
    @benchmarks/run-agent-workflows.sh
    @benchmarks/run-durability.sh
    @benchmarks/run-render.sh

# Apply safe lint autofixes, then format every supported file type
fix: && format
    cargo clippy --locked --workspace --all-targets --all-features --fix --allow-dirty --allow-staged
    cargo clippy --locked -p termtrace-sdk --no-default-features --all-targets --fix --allow-dirty --allow-staged
    cargo machete --fix
    uv run --project crates/bindings-python --group dev ruff check --fix --extend-exclude vendor/ghostty .

# Measure all local test suites and print coverage
coverage:
    #!/usr/bin/env bash
    set -euo pipefail
    export CARGO_TARGET_DIR="$PWD/target"
    cargo llvm-cov clean --workspace
    eval "$(cargo llvm-cov show-env --sh)"
    export TERMTRACE_BIN="$CARGO_TARGET_DIR/debug/termtrace"
    export TERMTRACE_BINARY="$TERMTRACE_BIN"
    cargo build --locked --workspace
    cargo llvm-cov --no-report --no-clean nextest --locked --workspace --ignore-default-filter
    cargo llvm-cov --no-report --no-clean nextest --locked --workspace --run-ignored only --test-threads=1 -E 'not test(/noisy_/)'
    eval "$(cargo llvm-cov show-env --sh)"
    cargo build --locked --workspace
    export TERMTRACE_BIN="$CARGO_TARGET_DIR/debug/termtrace"
    export TERMTRACE_BINARY="$TERMTRACE_BIN"
    cargo test --locked -p termtrace-sdk --no-default-features
    scripts/test-rust-consumer.sh
    bash tests/install.sh
    (
        cd benchmarks
        export COVERAGE_FILE="$PWD/.coverage"
        uv run --group dev coverage erase
        uv run --group dev coverage run -m unittest test_acceptance.py || [[ $? -eq 5 ]]
        uv run --group dev coverage combine
        uv run --group dev coverage report
    )
    uv run --project crates/bindings-python --with maturin maturin develop --manifest-path crates/bindings-python/Cargo.toml
    (
        cd tests/consumers/python_native
        export COVERAGE_FILE="$PWD/.coverage"
        uv run --group dev coverage erase
        uv run --group dev coverage run -m pytest -c pyproject.toml -v . || [[ $? -eq 5 ]]
        uv run --group dev coverage combine
        uv run --group dev coverage report
    )
    bunx @napi-rs/cli build --cwd crates/bindings-node
    npm --prefix tests/consumers/node_native run coverage
    cargo llvm-cov report --package '*'

# Replay native Ghostty VT seeds; pass libFuzzer arguments to explore (requires cargo-fuzz and nightly-2026-08-20 with rust-src)
[positional-arguments]
fuzz-vt *args="-runs=0":
    cargo +nightly-2026-08-20 fuzz run ghostty_vt fuzz/corpus/ghostty_vt -- -timeout=5 -max_len=4096 "$@"

# Run one-minute fuzz campaigns and replay native VT seeds (requires cargo-fuzz and nightly-2026-08-20 with rust-src)
fuzz: fuzz-vt
    cargo +nightly-2026-08-20 fuzz run output_projection fuzz/corpus/output_projection -- -max_total_time=60 -timeout=5 -max_len=4096
    cargo +nightly-2026-08-20 fuzz run control_protocol fuzz/corpus/control_protocol -- -max_total_time=60 -timeout=5 -max_len=16384
    cargo +nightly-2026-08-20 fuzz run event_log fuzz/corpus/event_log -- -max_total_time=60 -timeout=5 -max_len=262144
    cargo +nightly-2026-08-20 fuzz run bundle_archive fuzz/corpus/bundle_archive -- -max_total_time=60 -timeout=5 -max_len=262144

# Check model undefined behavior under Miri (requires nightly-2026-08-20 with miri)
miri:
    cargo +nightly-2026-08-20 miri test --locked -p termtrace-model --lib

# Run the terminal boundary under AddressSanitizer against a sanitized libghostty-vt (requires nightly-2026-08-20 with rust-src)
sanitize:
    TERMTRACE_SANITIZE=address TERMTRACE_GHOSTTY_PREFIX={{ justfile_directory() }}/target/ghostty-asan scripts/bootstrap-ghostty-vt.sh
    ASAN_OPTIONS=detect_leaks=1 RUSTFLAGS=-Zsanitizer=address TERMTRACE_GHOSTTY_PREFIX={{ justfile_directory() }}/target/ghostty-asan cargo +nightly-2026-08-20 test --locked -Zbuild-std --target x86_64-unknown-linux-gnu -p termtrace-terminal --lib

# Run concurrency checks under ThreadSanitizer (requires nightly-2026-08-20 with rust-src)
race:
    RUSTFLAGS="-Zsanitizer=thread" cargo +nightly-2026-08-20 test --locked -Zbuild-std --target x86_64-unknown-linux-gnu -p termtrace-engine connection_limiter

# Audit Rust dependencies for security advisories
audit:
    cargo audit --file Cargo.lock
    cargo audit --file fuzz/Cargo.lock

# Kill mutants in the streaming control stripper and screen boundary
mutation:
    TERMTRACE_GHOSTTY_PREFIX={{ justfile_directory() }}/vendor/ghostty/zig-out cargo mutants --cargo-arg=--locked -p termtrace-terminal -f crates/terminal/src/output_projection.rs -f crates/terminal/src/screen.rs --timeout 60 --jobs 2

# Type-check every Rust target and feature
typecheck: validate

# Run the commit checks
check: (run-checks "formatcheck" "lockcheck" "audit" "lint" "bindings-lint" "typecheck" "bindings-build" "test")

# Remove generated output, caches, and dependencies
clean: (remove "node_modules" "vendor/ghostty/zig-out" "vendor/ghostty/.zig-cache" ".venv" "dist" "build" ".pytest_cache" ".pyrefly" ".ruff_cache" ".coverage" "htmlcov" ".coverage.*" "coverage.xml" "coverage" ".tools" ".zig-cache" "zig-cache" "zig-out" "fuzz/target" "fuzz/coverage" "tests/consumers/rust/target" "crates/bindings-python/.venv" "crates/bindings-python/dist" "crates/bindings-python/build" "crates/bindings-python/.pytest_cache" "crates/bindings-python/.pyrefly" "crates/bindings-python/.ruff_cache" "crates/bindings-python/.mypy_cache" "crates/bindings-python/.coverage" "crates/bindings-python/htmlcov" "crates/bindings-python/python/termtrace/*.so" "crates/bindings-node/node_modules" "crates/bindings-node/dist" "crates/bindings-node/coverage" "crates/bindings-node/*.node" "tests/consumers/python_native/.venv" "tests/consumers/python_native/.pytest_cache" "tests/consumers/node_native/node_modules" "tests/consumers/node_native/dist" "tests/consumers/node_native/coverage" "benchmarks/.venv" "benchmarks/.pytest_cache" "mutants.out" "mutants.out.old" "**/__pycache__" "crates/bindings-node/**/*.tsbuildinfo" "tests/consumers/node_native/**/*.tsbuildinfo" "target/llvm-cov-target" "benchmarks/.coverage" "benchmarks/.coverage.*" "tests/consumers/python_native/.coverage" "tests/consumers/python_native/.coverage.*")
    @cargo clean

# Check the same files and tools without writing
formatcheck:
    @ruff format --check --extend-exclude '*.md,vendor/ghostty' .
    @cargo fmt --check
    @cargo statement-spacing check
    @git ls-files -co --exclude-standard -z | while IFS= read -r -d '' file; do if [[ -f $file ]]; then printf '%s\0' "$file"; fi; done | xargs -0 -r oxfmt --check --no-error-on-unmatched-pattern '!**/*.md' '!schemas/**' '!**/package.json' '!crates/bindings-node/types.d.ts' '!**/fixtures/**' '!**/snapshots/**' '!**/corpus/**' '!**/package-lock.json' '!crates/bindings-node/dts-header.d.ts' '!crates/bindings-node/index.d.ts' '!**/*-lock.json'
    @git ls-files -co --exclude-standard -z | while IFS= read -r -d '' file; do if [[ -f $file ]]; then printf '%s\0' "$file"; fi; done | xargs -0 -r shfmt -f | xargs -r -d '\n' shfmt -d
    @git ls-files -co --exclude-standard -- Justfile justfile "*.just" | while IFS= read -r file; do just --justfile "$file" --fmt --check; done
    @just markdown check --exclude vendor/ghostty
    @git ls-files -co --exclude-standard -z | while IFS= read -r -d '' file; do if [[ -f $file ]]; then printf '%s\0' "$file"; fi; done | xargs -0 -r editorconfig-checker -disable-indent-size -disable-indentation -disable-max-line-length -disable-charset -exclude '^fuzz/corpus/'

# Run the commit checks and slow suites
checkfull: (run-checks "formatcheck" "lockcheck" "audit" "lint" "bindings-lint" "validate" "bindings-build" "test" "integration" "bindings-test" "native-check" "native-test" "sanitize" "race" "miri" "fuzz" "mutation" "bench")

# Synchronize dependency locks without upgrading dependencies
[no-exit-message]
lock:
    @cd "benchmarks" && uv lock
    @cd "crates/bindings-python" && uv lock
    @cd "tests/consumers/python_native" && uv lock
    @cargo update --workspace
    @cd "fuzz" && cargo update --workspace
    @cd "tests/consumers/rust" && cargo update --workspace
    @cd "tests/consumers/node_native" && npm install --package-lock-only --ignore-scripts

# Verify dependency manifests and locks without changing files
[no-exit-message]
lockcheck:
    cd "benchmarks" && uv lock --check
    cd "crates/bindings-python" && uv lock --check
    cd "tests/consumers/python_native" && uv lock --check
    cargo metadata --locked --format-version 1 > /dev/null
    cd "fuzz" && cargo metadata --locked --format-version 1 > /dev/null
    cd "tests/consumers/rust" && cargo metadata --locked --format-version 1 > /dev/null
    cd "tests/consumers/node_native" && npm ci --dry-run --ignore-scripts

# List available recipes
[default]
help:
    @just --list

alias fmt := format
alias cov := coverage

[positional-arguments]
[private]
remove +paths:
    #!/usr/bin/env bash
    set -euo pipefail
    shopt -s nullglob globstar
    size() { LC_ALL=C numfmt --from-unit=1024 --to=iec-i --suffix=B --format=%.1f "$1"; }
    rows=() total=0
    for pattern in "$@"; do
        found=()
        for path in $pattern; do
            if [[ -e $path || -L $path ]]; then found+=("$path"); fi
        done
        if (( ${#found[@]} == 0 )); then continue; fi
        kib=$(du -sck -- "${found[@]}" | tail -n1 | cut -f1)
        rm -rf -- "${found[@]}"
        label=$pattern
        if (( ${#found[@]} > 1 )); then label+=" (${#found[@]})"; fi
        rows+=("removed  $label|$(size "$kib")")
        total=$(( total + kib ))
    done
    if [[ -n ${CLEAN_TOTAL_FILE:-} ]]; then echo "$total" >> "$CLEAN_TOTAL_FILE"; fi
    if (( ${#rows[@]} == 0 )); then echo "nothing to clean"; exit 0; fi
    printf '%s\n' "${rows[@]}" "total freed|$(size "$total")" | column -t -s '|' -R 2

[positional-arguments]
[private]
report-install +paths:
    #!/usr/bin/env bash
    set -euo pipefail
    rows=() warnings=() status=0
    for path in "$@"; do
        name=$(basename "$path") real=$(realpath "$path")
        sized=$real venv=$(dirname "$(dirname "$real")")
        [[ -f $venv/pyvenv.cfg ]] && sized=$venv
        if ! version=$("$path" --version 2>&1); then
            "$path" --help >/dev/null 2>&1 || { echo "failed     $path does not start" >&2; status=1; continue; }
            version=-
        fi
        rows+=("installed|$name|${path/#$HOME/\~}|$(du -sk "$sized" | cut -f1 | LC_ALL=C numfmt --from-unit=1024 --to=iec-i --suffix=B --format=%.1f)|${version%%$'\n'*}")
        first=$(command -v "$name")
        [[ $(realpath "$first") == "$real" ]] || warnings+=("warning    $name: PATH finds ${first/#$HOME/\~} first")
    done
    if (( ${#rows[@]} )); then printf '%s\n' "${rows[@]}" | column -t -s '|' -R 4; fi
    if (( ${#warnings[@]} )); then printf '%s\n' "${warnings[@]}"; fi
    exit "$status"

[positional-arguments]
[private]
run-checks +steps:
    #!/usr/bin/env bash
    set -uo pipefail
    LC_NUMERIC=C
    log_dir="${XDG_CACHE_HOME:-$HOME/.cache}/just-check"
    mkdir -p "$log_dir"
    log=$(mktemp "$log_dir/step.XXXXXX")
    trap 'rm -f "$log"' EXIT
    width=0 failed=()
    for step in "$@"; do (( ${#step} > width )) && width=${#step}; done
    for step in "$@"; do
        read -ra command <<< "$step"
        printf 'check  %-*s  ' "$width" "$step"
        start=$EPOCHREALTIME
        run=(just --justfile {{ quote(justfile()) }} "${command[@]}")
        if [[ -t 1 ]]; then run=(script -qefc "${run[*]@Q}" /dev/null); fi
        if "${run[@]}" < /dev/null > "$log" 2>&1; then status=ok; else status=FAIL; failed+=("$step"); fi
        printf '%-4s  %7.1fs\n' "$status" "$(LC_NUMERIC=C awk -v a="$start" -v b="$EPOCHREALTIME" 'BEGIN { print b - a }')"
        if [[ "$status" == FAIL ]]; then sed 's/\r$//; s/^/    /' "$log"; fi
    done
    if (( ${#failed[@]} )); then printf '%d of %d failed: %s\n' "${#failed[@]}" "$#" "${failed[*]}"; exit 1; fi
    printf 'all %d passed\n' "$#"

[private]
markdown *args:
    rumdl {{ args }} --disable MD001,MD013,MD024,MD025,MD026,MD031,MD032,MD033,MD034,MD036,MD041,MD058 --config 'MD060.enabled=true' --config 'MD060.style="aligned"' --config 'MD003.style="atx"' --config 'MD004.style="dash"' --config 'MD029.style="ordered"' --config 'MD046.style="fenced"' --config 'MD048.style="backtick"' --config 'MD049.style="asterisk"' --config 'MD050.style="asterisk"' --config 'MD055.style="leading_and_trailing"'
