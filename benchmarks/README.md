# Benchmarks

Use the single benchmark entrypoint:

```sh
benchmarks/run.sh check    # validate contracts
benchmarks/run.sh agent    # agent-facing output limits
benchmarks/run.sh durability
benchmarks/run.sh render   # semantic export and preview
benchmarks/run.sh all
```

Each run creates an owner-only directory under `benchmarks/results/` containing
`run.json` and JSONL results. Set `RUN_DIR` for an exact location,
`RESULTS_ROOT` for its parent, or `TERMTRACE_BINARY` to use an existing binary.

The committed contracts are:

- [`acceptance-v1.json`](acceptance-v1.json): host-pinned fixtures, latency
  gates, invariants, and artifact validators.
- [`agent-workflows-v1.json`](agent-workflows-v1.json): output-size and
  one-command limits for common agent workflows.

Generate the large acceptance fixture only when needed:

```sh
cargo run --release --bin termtrace-fixture -- large \
  --events 1000000 --output-bytes 1073741824
```

Override defaults with `REPETITIONS`, `PAYLOAD_BYTES`, `RENDER_WARMUPS`, and
`RENDER_REPETITIONS`.
