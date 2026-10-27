# SDK capability contract v1 (Linux / Ghostty)

This is the implemented **task 01 foundation**, not full SDK parity certification.
`fixtures/conformance/sdk/matrix.json` is the authoritative leaf inventory. Each
row identifies core source, canonical model, Rust feature, sync/async Python and
TypeScript exposure, units, runtime requirements, positive/negative/lifecycle
vectors and closure owner. `exposed_unverified` means a name exists, **not** that
its guarantee has passed. `partial` includes reduced frames and JSON-string
legacy reports. `missing` describes the SDK facade, not absence of core code.
`core_status` separately distinguishes implemented, partial and reserved work.

The table below renders the baseline inventory; evidence-specific verification
lives in the separate report, never inferred from a family's happy path. The 450
pending vectors in `cases/pending.json` are owner-assigned acceptance slots, not
executable scenarios and not passing skips. Task 11 replaces/extends these with
fixed SDK test vectors as downstream capabilities close. `foundation.json` and
`portable.json` are executable fixed vectors today. There is only one aggregator:
`scripts/sdk-conformance.py`; adapters execute ordinary public SDK calls.

## Public names and transport

* Rust lightweight `termtrace_sdk::contracts` reexports `MediaArtifactResult`,
  `SdkArtifactMember`, `ArtifactManifestRef`, `ArtifactSourceRef`,
  `SdkCanonicalBoundary`, `ArtifactProvenance`, `ArtifactSourceKind`,
  `ArtifactProductKind`, `DecimalU64`, `DecimalU128` without enabling `local`.
  With `local`, it also exposes `decode_media_artifact`, `encode_media_artifact`,
  `timeout_from_ms`, `validate_region`, the control option and result types, and
  the SDK type catalog in `contracts::types`.
* Python and Node bindings convert Rust values with serde alone. Python model types
  are the TypedDicts in the generated `termtrace.types`; the native stub
  `_termtrace.pyi` and `termtrace.aio` are generated too. Node model types are the
  interfaces in the generated `types.d.ts`, and NAPI generates `index.d.ts`.
  `scripts/sdk-types.py` builds both type files from the catalog; `just generate`
  regenerates every generated file.

Canonical Rust models remain authoritative and serialize the same everywhere.
Native hosts receive exact values: every Python integer is an `int`; Node gets
`bigint` for 64- and 128-bit integers and `number` for smaller ones. Integers
inside arbitrary JSON values, such as application attributes, are always Node
numbers and reject outside the safe-integer range. `DecimalU64` and `DecimalU128`
fields (`members[].byte_length`, `source.boundary.next_event_seq`,
`source.boundary.output_bytes`) keep canonical decimal strings in JSON files and
reach hosts as integers. Numeric JSON is rejected for them; signs, leading zeros
(except `0`), fractions, overflow and non-decimal encodings fail.

Action results (`ActionResult.to_json` / `toJson`) use the canonical `ActOutcome`
JSON. `ScreenFrame.to_json` and canonical evidence keep their numeric model form.

Byte fields use standard padded Base64 in JSON, `ByteString` in Rust, `bytes` in
Python and `Buffer` in Node. Paths serialize as a string when they are UTF-8 and as
a byte array otherwise; Python receives `os.fsdecode` text and Node a `string`, or
a `Buffer` for a non-UTF-8 path. The envelope directory is exact Linux filesystem
bytes. NUL/empty directory values are invalid. Do not reinterpret or rewrite
canonical manifests, event logs, receipts or hashes.

`version` is exactly 1. `schema_type` is required and nullable, not silently dropped.
Other fields in this new envelope are required. Legacy owning models retain their
own absent/null rules. Unknown fields, escaping/absolute/backslash/colon paths,
non-lowercase BLAKE3, duplicate members and contradictory provenance are rejected.
Member/manifest references are normalized paths below the artifact directory;
this data-only decoder does not open files, verify the referenced hash contents,
certify a provenance claim against manifest truth, or authorize symlink traversal. Export
owners must enforce no-replace publication and filesystem confinement themselves.
The manifest is not repeated in `members`; it has its own exact hash reference.

## Source and provenance (no inflated capture guarantee)

`source` requires canonical `session_id`, exact `session_manifest_hash` and an
exact prefix `boundary`: `next_event_seq`, `event_hash`, `output_bytes`. The hash
at sequence zero is the canonical **event header hash**, not null or a made-up
empty-event hash. `SdkCanonicalBoundary::from(CanonicalBoundary)` copies these
facts without inventing correlation. Source/output mappings, timing, capture
outcomes and profiles remain authoritative in the referenced manifest, not a
second mapping engine inside this envelope.

`provenance.kind` has these disjoint meanings:

* `semantic_rendering`: pixels/text reconstructed from semantic terminal state.
* `native_replay`: native pixels rendered while replaying evidence, not original
  execution pixels.
* `original_execution`: capture during the source execution; no frame-exact
  linkage to a semantic state is implied by this envelope alone.
* `presentation_output`: explicit derivative, requiring `source_kind` from the
  preceding three. Nested presentation truth is not an original execution claim.

`product_kind` is `snapshot`, `video`, `playback`, `presentation` or `preview`.
Only the latter two use `presentation_output` provenance. Preview is explicit,
not inferred from an extension. Native errors never trigger semantic fallback.
Reserved native/media rows are pending until 07–10/11 run their actual runtime
verification. Schema/data support alone is never capture support.

## Units, errors and ownership

Existing Python timeouts remain seconds, Node milliseconds, Rust `Duration`.
New portable conversions require explicit `timeout_ms` / `timeoutMs` naming;
1250 ms is tested as 1.25 seconds in Python and Rust, 1250 ms in Node. Non-finite,
negative and unrepresentable durations fail. Regions are zero-based half-open
validated u16 cell bounds; empty/reversed/out-of-grid regions fail.

Mutation semantics remain owned by `protocol/mutation.rs`: `not_applied` →
`new_request`, `partial` → `inspect_first`, `applied` → `do_not_retry`, `unknown` →
`same_request_id`. Preserve kind/code, request identity where known, effect/retry.
Postcondition failure is not rollback. Cancelling an off-thread await does not
undo potentially continuing work. Assertion mismatches remain distinct from
infrastructure failures. Existing Node callback+cleanup primary-error tests and
Python cancellation/delegation regressions are run, not weakened or replaced.

## Running and extending the single foundation engine

```
python3 scripts/sdk-conformance.py --profile inventory
python3 scripts/sdk-conformance.py --profile foundation --output target/sdk-run-UNIQUE
```

Prerequisites: built `target/debug/termtrace`, built Python/Node native bindings,
consumer Python environment with pytest, `npm ci` in the Node consumer, and the
Python binding dev environment with mypy. The TypeScript consumer is exactly
locked to 5.9.3. Missing prerequisites fail required checks, never passing skips.
`TERMTRACE_BIN`, `CARGO_BUILD_JOBS` (default 4), and
`SDK_CONFORMANCE_TIMEOUT_SECONDS` (default 600 per command) are configurable.
Every Cargo/native integration command is serialized with `flock`. The default
lock is `<repository>/target/sdk-conformance.lock`; its parent directory is created
as needed. Set `TERMTRACE_VERIFICATION_LOCK` to select a shared lock across runs or
checkouts. Absolute overrides are used directly; relative overrides are resolved
from the repository root. An unset or empty override selects the repository-local
default. Coordinate all native build/test jobs on the same selected lock. For
example, with an already-selected absolute `SHARED_VERIFICATION_LOCK`:

```sh
TERMTRACE_VERIFICATION_LOCK="$SHARED_VERIFICATION_LOCK" \
  python3 scripts/sdk-conformance.py --profile foundation --output target/sdk-run-UNIQUE
```

Other native build/test commands must use `flock "$SHARED_VERIFICATION_LOCK" ...`
with that same path (create its parent directory first). The engine itself must
**not** be invoked inside its selected non-reentrant lock: it acquires the lock
per native command. The output directory must be new.

The Rust external consumer, Python sync/async pytest adapters and Node test
adapter write observations and retain independent evidence directories. Shared
expectations verify 30x96 geometry, READY text, exact PTY `52 45 41 44 59 00 ff`,
stale-state rejection, no canonical input intent/result, completion and absent
child and recorder PIDs after stop. The engine reads canonical evidence itself: an adapter's
success message without observations/evidence cannot certify anything. It runs
positive/negative TypeScript and Python compilation, codec edge vectors, cleanup
and cancellation regressions. Adapter crashes, timeouts, missing observations,
missing executions and numeric wide JSON all fail aggregation. Timeouts terminate
and reap the adapter process group. No raw-control substitute counts as facade
coverage. Runtime failures preserve evidence and logs.

`report.json` v1 is separate from scenario/JUnit. Its `entries` cover matrix rows,
`checks` cover codec/type/regression checks, and `pending` names unverified owners.
It lists exact commands, tool
versions, row/case/language, implementation status, execution status, runtime
availability (`available`/`unavailable`/`unknown`), verdict, observed result path,
evidence references and diagnostics. Nonzero adapter exits do not automatically
mean a capability is missing: their runtime availability remains unknown and the
required verdict fails. Missing execution is explicitly missing/unavailable. The
only successful aggregate claim is `verified_foundation_only`; pending owner rows
remain separately listed. Fault injection: `--without-adapter node` must fail.

Focused tests: `cargo test --locked -p termtrace-service sdk_contract`,
`cargo test --locked -p termtrace-schema`, `cargo test --locked --test cli sdk_contract`,
`node --test tests/consumers/node_native/test/contracts.test.js`,
`python3 -m unittest discover -s scripts/tests -p test_sdk_conformance.py`.
Resource-intensive commands require the shared lock above. To regenerate only
this task's schema, generate the catalog into a temporary directory and copy only
`sdk-media-artifact-result.schema.json`; never overwrite downstream-owned schemas.

### Sequential downstream handoff

01 retains inventory IDs, media artifact codec, new schema and conformance engine until
lead-coordinated transfer. Binding/generated facade ownership transfers sequentially
02 → 03 → 04 → 05 → 07 → 09 → 10; task 08 owns export internals, not facades. 02
closes rich frames/locators; 03 interaction/lifecycle; 04 assertions/helpers; 05
diagnostics/evidence/management; 07 original-native live capture; 08 codecs; 09
profiles; 10 editing; 11 workflows and full certification. Task 06 exclusively owns
scenario/verdict/JUnit correctness. Do not add another conformance engine, mutate
canonical evidence, or mark reserved rows verified from foundation results.

## Baseline rendered inventory

<!-- sdk-matrix:start -->
| Capability | Rust | Python | TypeScript | Owner |
|---|---|---|---|---|
| session.launch | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| session.environment | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| session.geometry | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| session.resource_limits | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| session.readiness | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| session.attach | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| session.target | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| session.status | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| screen.observe | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| screen.resolve | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| screen.provenance | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| screen.hidden | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| screen.presented | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| screen.text | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| screen.spans | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| screen.cells | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| screen.bounds | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| screen.cursor | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| screen.modes | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| screen.colors | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| screen.wraps | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| locators.text | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| locators.regex | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| locators.style | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| locators.regions | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| locators.strictness | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| waits.text | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| waits.stable | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| waits.reaction | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| waits.idle | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| input.text | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| input.key | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| input.bytes | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| input.paste | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| input.pointer | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| input.focus | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| input.pacing | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| input.resize | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| input.signal | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| input.markers | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| input.ordered_send | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| mutation.stale_guard | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| mutation.request_identity | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| mutation.deduplication | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| mutation.input_outcome | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| mutation.postcondition | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| mutation.effects | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| mutation.retry | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| lifecycle.stop | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| lifecycle.completion | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| lifecycle.detach | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| lifecycle.force_close | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| lifecycle.reaping | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| lifecycle.cancellation | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| testing.assertions | exposed_unverified | exposed_unverified | exposed_unverified | 04 |
| testing.snapshots | exposed_unverified | exposed_unverified | exposed_unverified | 04 |
| testing.update_policy | exposed_unverified | exposed_unverified | exposed_unverified | 04 |
| testing.rust_helper | exposed_unverified | exposed_unverified | exposed_unverified | 04 |
| testing.pytest | missing | exposed_unverified | missing | 04 |
| testing.node_helper | missing | missing | exposed_unverified | 04 |
| testing.cleanup_evidence | exposed_unverified | exposed_unverified | exposed_unverified | 04 |
| output.bytes | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| output.cursor | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| output.wait | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| events.filtering | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| events.follow | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| events.pagination | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| analysis.timeline | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| analysis.diff | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| analysis.explain | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| analysis.diagnose | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| analysis.last_action | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| instrumentation.events | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| instrumentation.spans | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| integrity.verification | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| integrity.recovery | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| integrity.receipts | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| integrity.signing | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| integrity.sensitivity | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| bundles.create | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| bundles.profiles | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| bundles.verify | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| registry.discovery | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| registry.register | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| registry.remove | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| registry.restart | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| registry.gc | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| tmux.management | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| tmux.aggregation | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| tmux.verification | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| system.probes | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| system.versions | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| system.schemas | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| system.diagnostics | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| artifacts.png | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| artifacts.svg | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| artifacts.html | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| artifacts.txt | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| artifacts.json | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| artifacts.deep_json | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| artifacts.vt | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| artifacts.asciicast | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| native.original_recording | exposed_unverified | exposed_unverified | exposed_unverified | 07 |
| native.live_control | exposed_unverified | exposed_unverified | exposed_unverified | 07 |
| native.replay_screenshot | exposed_unverified | missing | missing | 07 |
| native.replay_video | exposed_unverified | missing | missing | 07 |
| native.playback | exposed_unverified | missing | missing | 07 |
| delivery.gif | missing | missing | missing | 08 |
| delivery.webm | missing | missing | missing | 08 |
| delivery.codec_availability | missing | missing | missing | 08 |
| delivery.no_replace | missing | missing | missing | 08 |
| visual.profiles | missing | missing | missing | 09 |
| visual.geometry | missing | missing | missing | 09 |
| visual.fonts | missing | missing | missing | 09 |
| visual.dpi | missing | missing | missing | 09 |
| visual.config_identity | missing | missing | missing | 09 |
| presentation.edit_plans | missing | missing | missing | 10 |
| presentation.selectors | missing | missing | missing | 10 |
| presentation.clips | missing | missing | missing | 10 |
| presentation.speeds | missing | missing | missing | 10 |
| presentation.holds | missing | missing | missing | 10 |
| presentation.annotations | missing | missing | missing | 10 |
| presentation.subtitles | missing | missing | missing | 10 |
| presentation.transforms | missing | missing | missing | 10 |
| presentation.dry_run | missing | missing | missing | 10 |
| presentation.mapping | missing | missing | missing | 10 |
| presentation.preview | missing | missing | missing | 10 |
| presentation.export | missing | missing | missing | 10 |
| presentation.verification | missing | missing | missing | 10 |
| workflows.parallel | exposed_unverified | missing | missing | 11 |
| workflows.sharded | exposed_unverified | missing | missing | 11 |
| workflows.matrix | exposed_unverified | missing | missing | 11 |
| workflows.snapshots | exposed_unverified | missing | missing | 11 |
| workflows.aggregate | exposed_unverified | missing | missing | 11 |
| workflows.junit | exposed_unverified | missing | missing | 11 |
| workflows.ci_verdict | exposed_unverified | missing | missing | 11 |
| input.scroll | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| session.working_directory | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| screen.frame | exposed_unverified | exposed_unverified | exposed_unverified | 02 |
| testing.cursor | exposed_unverified | exposed_unverified | exposed_unverified | 04 |
| testing.exists | exposed_unverified | exposed_unverified | exposed_unverified | 04 |
| testing.not_exists | exposed_unverified | exposed_unverified | exposed_unverified | 04 |
| testing.count | exposed_unverified | exposed_unverified | exposed_unverified | 04 |
| testing.match_text | exposed_unverified | exposed_unverified | exposed_unverified | 04 |
| testing.style | exposed_unverified | exposed_unverified | exposed_unverified | 04 |
| testing.wraps | exposed_unverified | exposed_unverified | exposed_unverified | 04 |
| testing.neutral_region | exposed_unverified | exposed_unverified | exposed_unverified | 04 |
| testing.presented_transient | exposed_unverified | exposed_unverified | exposed_unverified | 04 |
| instrumentation.end_span | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| registry.resolve | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| analysis.cache_status | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| analysis.cache_prune | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| analysis.markers | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| analysis.review_summary | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| analysis.review_search | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| analysis.review_export | exposed_unverified | exposed_unverified | exposed_unverified | 05 |
| lifecycle.discard_completed | exposed_unverified | exposed_unverified | exposed_unverified | 03 |
| output.pagination | exposed_unverified | exposed_unverified | exposed_unverified | 05 |

<!-- sdk-matrix:end -->


## Task05 typed diagnostics, evidence and management

Task05 now provides additive typed shared-service namespaces. The matrix's `exposed_unverified`
status deliberately does not mean independent lead admission or full Task11 certification. Legacy
string/boolean methods remain for one compatibility interval; do not silently swap their return types.

- `Session` and `Recording`: `.analysis()` returns Diagnostics; `.evidence()` / `.integrity()`
  return Evidence; `.events()`, `.output()` and `.instrumentation()` are source-bound namespaces.
  Client factories accept explicit exact byte paths. Rust retains typed service APIs and full
  client conveniences; Python's `aio` namespaces run blocking work off-loop. `AsyncSession` is an
  attached reference, never a PID-reconstructed owner. Existing `attach_target` keeps its old sync
  return; new async `attach` / `attach_session_target` return an async attached facade.
- Analysis retains bounded timeline/ranges, diff, VT explanation, conservative findings/native
  limitations, last action, markers, disposable cache inventory/prune and review summary/search/export.
  Nested optional facts, source hashes, confidence and resource/provenance fields are not JSON strings.
- Saves support all existing formats and StatePoint variants. PNG/SVG stay semantic renderings,
  never original native pixels. Manifests retain state/source hashes, renderer, font resources,
  sensitivity and exact counts. Existing destinations do not get overwritten.
- New bundle creation requires explicit Screen/Debug/Forensic policy; verification requires the
  caller's source and exact profile. Wrong source/profile/tampered members are errors, not false
  success. Screen/Debug are sensitivity policies, **not advanced redaction or secret-safety promises**.
- Integrity exposes content/structure/durability/signature verification, explicit allowed-signers,
  temporary/caller-owned signing-key paths and signer identity, plus separate session/log recovery.
  Cryptographic validity, receipt presence, signer trust and session/artifact authenticity remain
  distinct. Unknown levels/profiles reject; signing neither logs keys nor establishes signer trust.
- Explicit-root registry operations distinguish missing aliases from corrupt/permission/invalid-source
  failures. Removal preserves source evidence. GC defaults to dry-run; apply is explicit and protects
  named/live/incomplete evidence. Shared restart preserves keep/set/clear, environment commitments,
  original predecessor completion, immutable lineage, readiness/orphan diagnostics and alias CAS.
  Failed admission is not rollback or permission to clobber another owner's alias.
- Shared tmux management uses the existing recorder, scope, coverage, gap/history catalog and rollback.
  The CLI and native SDKs reuse ServiceTracks. Additive explicit-binary entrypoints ensure generated
  pane/guard/watchdog commands use the selected Termtrace binary, never Python/Node's current_exe.
  Existing public Rust setup/record signatures and CLI defaults remain. No new capture backend.
- Instrumentation returns the actual actor acknowledgement and process/span identities. JSON
  attributes are bounded ordinary JSON; arbitrary same-named keys are never counter/byte-revived.
  Unsupported unsafe integers reject rather than round. Spans end explicitly; GC never invents an
  end. Cancelling/abandoning off-thread signing, GC, recovery, restart or event emission is not rollback.
- System namespaces expose real typed capability probes, component versions, canonical schema catalog
  and schema JSON. Blocked dependencies retain diagnostics/remediation, not passing-skip certification.

### Exact transports, immutable sources and recovery

Python uses int/bytes; TypeScript uses bigint/Buffer for 64- and 128-bit integers and bytes,
including u128 registry timestamps and optional observed event times. Small bounded geometry/limits
remain numbers. Full event unions, metadata-only models, hashes and byte summaries use the owning
schemas, never heuristics over arbitrary JSON keys. Cursor JSON adapters use the canonical cursor
JSON; canonical numeric/Base64 encodings and hashes remain unchanged. Legacy numeric seeks reject unsafe/fractional/negative/nonfinite/overflow values.

Paged readers bind exact source path bytes, session UUID, device/inode/header and chain hashes;
filters advance scan positions with snapshot-relative lookahead and no omitted first match. Torn tails
are explicit, complete corruption/replaced sources reject. Follow reports actual actor durable/start
positions or independently validated completion, not a promise that a live prefix is complete.

NonUTF8 recovery metadata paths are serialized as their exact byte array. Existing valid UTF8
strings retain identical serialization bytes
and hashes. Source canonical bytes are never rewritten. Recovery publishes a validated prefix only
into an absent separate destination outside the source (also checking parent symlink aliases), with
no completion/no-output-loss claim; failures leave destinations absent or preserve prior valid data.

Disposable endpoint checkpoints lacking source-position provenance are conservative cache misses;
existing authenticated canonical replay establishes both present and genuinely absent facts. Cache
presence, lateness, pruning or old producer metadata cannot erase known positions and make bundle
verification disagree with immutable canonical evidence. Authentication, no-replace publication and
full verifier equality are not weakened.

### Verification and ownership

Nested declarations come from `scripts/sdk-types.py` and the native stub generator plus pinned Ruff fix/format; schemas
are generated into a temporary catalog and only owned outputs copied. Existing catalog entries remain.
The SDK consumer suites use `fixtures/conformance/sdk/evidence-pages.json` and
`diagnostics-evidence.json`, strict positive/negative TS/Mypy vectors, actual temporary OpenSSH keys,
isolated registry roots and private tmux sockets. Rust engine/service regressions additionally cover
late/older/tampered caches, initial absent facts, failed readiness/lineage/CAS and orphan retention.
Actual sources, hash observations and command logs are in the Task05 handoff, not inferred from matrix
labels or old extension binaries. Native builds remain sequential and normal unversioned builds.
No task05 code creates another report/terminal/scenario engine, changes the original04 owner/finalizer
or Stop-effect contract, or admits downstream work without independent lead review.
