# Acceptance benchmarks

`acceptance-v1.json` is the versioned product acceptance contract. It pins the
comparison revisions, execution environment, fixtures, measurement policy,
hard gates, invariants, and artifact validators used for roadmap decisions.

Changing an absolute threshold, fixture, percentile method, durability policy,
or baseline requires a reviewed manifest diff accompanied by measured evidence.
A failed gate must remain visible; do not replace its denominator in generated
results.

Validate the manifest syntax with:

```sh
jq -e . benchmarks/acceptance-v1.json
```

Generate the exact million-event, one-GiB completed fixture with:

```sh
cargo run --release --bin termtrace-fixture -- large \
  --events 1000000 \
  --output-bytes 1073741824
```

The destination is owner-only and no-replace. Its completion is validated
before the generator reports success. Large-gate measurements remain
unavailable, not passing, until this fixture has been generated on the pinned
host and the relevant operation results have been recorded.

Run the durability comparison with the manifest's default 30 repetitions:

```sh
benchmarks/run-durability.sh
```

It records elapsed time, peak resident memory, configured policy, observed
uncommitted maxima, sync count, and final-sync result as JSONL under the ignored
`benchmarks/results/` directory. `REPETITIONS`, `PAYLOAD_BYTES`, and `OUTPUT`
may override the run without changing the acceptance contract.

Run the compact agent-facing workflow gates with:

```sh
cargo build --release
benchmarks/run-agent-workflows.sh
```

`agent-workflows-v1.json` caps default stdout for stable observation, atomic
input, status, and bounded timeline operations. The runner fails immediately
when a command crosses its byte budget and records timing plus byte counts as
JSONL.
