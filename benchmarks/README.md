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

The large-fixture generator and executable benchmark harness are roadmap
deliverables. Until they exist, their gate results are explicitly unavailable,
not passing.

Run the durability comparison with the manifest's default 30 repetitions:

```sh
benchmarks/run-durability.sh
```

It records elapsed time, peak resident memory, configured policy, observed
uncommitted maxima, sync count, and final-sync result as JSONL under the ignored
`benchmarks/results/` directory. `REPETITIONS`, `PAYLOAD_BYTES`, and `OUTPUT`
may override the run without changing the acceptance contract.
