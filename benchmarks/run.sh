#!/usr/bin/env bash
set -Eeuo pipefail

readonly root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly results_root="${RESULTS_ROOT:-$root/benchmarks/results}"

usage() {
    cat <<'EOF'
Run Termtrace's committed benchmark suite.

Usage: benchmarks/run.sh COMMAND

Commands:
  check       Validate committed benchmark contracts and required tools
  agent       Run compact agent-facing workflow checks
  durability  Compare strict, bounded, and throughput durability modes
  render      Measure semantic video export and preview rendering
  all         Run check, agent, durability, and render benchmarks

Environment:
  TERMTRACE_BINARY  Existing release binary; otherwise build one
  RESULTS_ROOT      Parent directory for ignored benchmark result runs
  RUN_DIR           Exact result directory; otherwise create a timestamped one
  REPETITIONS      Durability repetitions per mode (default: 30)
  PAYLOAD_BYTES    Durability payload size (default: 1048576)
  RENDER_WARMUPS   Render warmups (default: 1)
  RENDER_REPETITIONS
                    Render measurements per operation (default: 3)
EOF
}

require_tools() {
    local missing=()
    local command
    for command in jq cargo date mktemp; do
        if ! command -v "$command" >/dev/null 2>&1; then
            missing+=("$command")
        fi
    done
    if ((${#missing[@]})); then
        printf 'missing benchmark tool(s): %s\n' "${missing[*]}" >&2
        exit 2
    fi
}

validate_contracts() {
    jq -e '
        .format == "termtrace-acceptance-benchmark" and
        .version == 1 and
        (.baseline.termtrace_commit | strings | length == 40) and
        (.fixtures | length > 0) and
        (.gates | length > 0) and
        (.invariants | length > 0) and
        (.artifact_validators | length > 0)
    ' "$root/benchmarks/acceptance-v1.json" >/dev/null
    jq -e '
        .format == "termtrace-agent-workflow-benchmark" and
        .version == 1 and
        (.workflows | length > 0) and
        all(.workflows[]; .maximum_commands == 1)
    ' "$root/benchmarks/agent-workflows-v1.json" >/dev/null
}

ensure_binary() {
    if [[ -n "${TERMTRACE_BINARY:-}" ]]; then
        binary=$TERMTRACE_BINARY
    else
        binary="$root/target/release/termtrace"
        if [[ ! -x "$binary" ]]; then
            cargo build --release --all-features --manifest-path "$root/Cargo.toml"
        fi
    fi
    if [[ ! -x "$binary" ]]; then
        printf 'Termtrace binary is not executable: %s\n' "$binary" >&2
        exit 2
    fi
}

create_run_dir() {
    if [[ -n "${RUN_DIR:-}" ]]; then
        run_dir=$RUN_DIR
        if [[ -e "$run_dir" ]]; then
            printf 'benchmark run directory already exists: %s\n' "$run_dir" >&2
            exit 2
        fi
        mkdir -m 700 -p "$run_dir"
    else
        mkdir -m 700 -p "$results_root"
        run_dir=$(mktemp -d "$results_root/run-XXXXXX")
    fi
}

write_run_manifest() {
    local command=$1
    local started_unix_ns=$2
    local revision
    revision=$(git -C "$root" rev-parse HEAD 2>/dev/null || printf 'unknown')
    jq -n \
        --arg command "$command" \
        --arg revision "$revision" \
        --arg binary "$binary" \
        --arg started_unix_ns "$started_unix_ns" \
        --arg host "$(hostname)" \
        '{
          format: "termtrace-benchmark-run",
          version: 1,
          command: $command,
          termtrace_commit: $revision,
          binary: $binary,
          host: $host,
          started_unix_ns: ($started_unix_ns | tonumber)
        }' >"$run_dir/run.json"
}

run_one() {
    local command=$1
    local started_unix_ns
    started_unix_ns=$(date +%s%N)
    create_run_dir
    write_run_manifest "$command" "$started_unix_ns"
    case "$command" in
        agent)
            TERMTRACE_BINARY="$binary" \
                CONTRACT="$root/benchmarks/agent-workflows-v1.json" \
                OUTPUT="$run_dir/agent-workflows.jsonl" \
                "$root/benchmarks/run-agent-workflows.sh"
            ;;
        durability)
            TERMTRACE_BINARY="$binary" \
                OUTPUT="$run_dir/durability.jsonl" \
                "$root/benchmarks/run-durability.sh"
            ;;
        render)
            TERMTRACE_BINARY="$binary" \
                OUTPUT="$run_dir/render.jsonl" \
                "$root/benchmarks/run-render.sh"
            ;;
        *)
            printf 'unsupported benchmark command: %s\n' "$command" >&2
            exit 2
            ;;
    esac
    printf 'benchmark run: %s\n' "$run_dir"
}

require_tools
command=${1:-}
case "$command" in
    check)
        validate_contracts
        printf 'benchmark contracts: OK\n'
        ;;
    agent | durability | render)
        validate_contracts
        ensure_binary
        run_one "$command"
        ;;
    all)
        validate_contracts
        ensure_binary
        create_run_dir
        run_dir_root=$run_dir
        write_run_manifest all "$(date +%s%N)"
        for benchmark in agent durability render; do
            RUN_DIR="$run_dir_root/$benchmark" \
                TERMTRACE_BINARY="$binary" \
                "$root/benchmarks/run.sh" "$benchmark"
        done
        printf 'benchmark run: %s\n' "$run_dir_root"
        ;;
    -h | --help | help)
        usage
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
