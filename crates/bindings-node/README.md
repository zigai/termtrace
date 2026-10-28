# @termtrace/core (Node.js / TypeScript)

Native Node.js and TypeScript bindings for Termtrace terminal recording and semantic debugging, powered by NAPI-RS.

## Screen observation and locators

`recording.screen()` returns frames, observations, and lazy locators. The shared Rust
engine does all matching. Data objects keep the Rust model's snake_case keys. Fields
that can exceed 2^53 are always `bigint`, and byte fields are `Uint8Array`. Cell
coordinates are zero-based columns and rows. Regions and match ranges are half-open.
Option durations are whole milliseconds (`timeoutMs`, `intervalMs`, `stableForMs`),
capped at one hour.

```ts
const screen = recording.screen()
const frame = await screen.frame({ detail: 'cells' })
await screen.getByText('Save').inRow(3).waitFor({ timeoutMs: 5000 })
await screen.getByRegex('(?i)error').waitFor({ condition: 'absent', signal: AbortSignal.timeout(2000) })
```

- Literal text matches overlap. Regex uses Rust `regex` syntax, not JavaScript
  `RegExp`, and its matches do not overlap.
- Match columns count terminal cells. JavaScript `text.length` counts UTF-16 units, so
  the two can differ.
- Bad argument types throw `TypeError`, and out-of-range values throw `RangeError`.
  Query failures reject with a `TermtraceError` whose `kind` names the failure. Timeouts
  and cancellation also carry the last `frame`.
- `waitFor` stops when its `signal` aborts or its `cancel` token is cancelled. The
  promise settles only after the native worker returns. Dropping a promise without
  aborting does not stop the worker.
- A wait notices cancellation within one capture quantum (250 ms) plus a round trip.
  If the recorder is busy, one capture can take up to about 5.25 s.

## Interaction and session control

`recording.actions()` builds one input request. Its `requestId` exists from the start.
`run()` freezes the request before it returns its promise, and resolves with an
`ActionResult` that has separate `input` and `postcondition` outcomes. Calling `run()`
again resends the same request; the recorder applies it at most once. Changing a frozen
builder throws `invalid_operation`. The legacy `execute()` sends a fresh request each
call.

```ts
const builder = recording.actions().sendText('y').sendKey('enter')
const result = await builder.untilScreenWhere({ predicate: 'contains', text: 'Saved' }, 5000).run()
await recording.pointer().activate('[OK]', { strict: true })
await recording.send([{ step: 'mark', label: 'before' }, { step: 'resize', cols: 100, rows: 30 }])
const limited = await tt.launch(['my-app'], { environment: { base: 'clear', set: { TOKEN: new Uint8Array([120]) } }, max_duration_ms: 60000 })
```

- Mutation errors carry `request_id`, `effect`, and `retry`. `postcondition_incomplete`
  also carries the applied `result`; a failed `send` carries `step` and
  `send_request_id`. Side effects are never rolled back.
- Mutations take no `AbortSignal`. Abandoning a promise does not stop the action; call
  `run()` on the same builder to learn its outcome.
- `pointer()` results say `applied`, `noop` (the application does not report that input),
  or `activated`. With `strict: true`, a no-op is an error instead.
- `status()`, `requestStop({ requestId, waitMs })`, and `waitForCompletion(timeoutMs)`
  report live and completed sessions.
- A cleared launch environment still gets the reserved terminal variables. Output the
  application prints is recorded as is, including any secret it prints.

## Test evidence and cleanup

`withTermtrace` from `@termtrace/core/testing` stops recordings created through its
`record` helper. Evidence is deleted only when both the test callback and cleanup
succeed. On a test or cleanup failure, it keeps the evidence directory and prints
its path to stderr. Extensible thrown errors also carry
`error.termtrace.evidenceRoot` and `error.termtrace.cleanupErrors`.

The original test failure is rethrown unchanged. Stop failures are reported and
trigger a force-close attempt; if only cleanup fails, the helper throws an
`AggregateError` instead of reporting a passing test. Frozen errors and primitive
throws are preserved too; their evidence location is still printed to stderr.

Run the helper's isolated regression tests with `npm test` in this directory.

## Ordinary Node/Vitest tests (no runner registration)

```js
const { withTermtrace, assertScreenText, normalizedSnapshot } = require('@termtrace/core/testing');

await withTermtrace(async ({ record }) => {
  const rec = await record(['/bin/sh', '-c', 'printf READY; cat'], 30, 96);
  await rec.screen().waitForText('READY', 5000); // milliseconds, explicit waiting
  const frame = await rec.screen().frame();
  assertScreenText(frame, 'READY');             // immutable capture, no hidden retry
  // Ordinary Vitest: expect(normalizedSnapshot(frame)).toMatchSnapshot();
}, { binary: 'target/debug/termtrace', evidencePolicy: 'on_failure' });
```

`testing` exports `assertScreenText`, `assertExists`, `assertNotExists`, `assertCount`,
`assertMatchText`, `assertMatchStyle`, `assertCursor`, `assertNoWrappedRows`, `assertNeutralRegion`,
`assertNoPresentedTransient`, `normalizedSnapshot`, `assertSnapshotFile`, `maskedSnapshot`,
`assertMaskedSnapshotFile`, and `ExpectationMismatchError`. Assertions accept captured `ScreenFrame`,
`QueryResult` or `ScreenMatch` values, not lazy locators. Unknown cursor coordinates remain `null`.
Presentation checks describe one frame, not absence of past/future transients.

`assertNot(() => ...)` catches **only** `ExpectationMismatchError`; query validation/strictness,
insufficient detail, invalid regex, capture and filesystem errors propagate. Its async callback
overload also propagates operational promise rejections. Mismatches carry bounded `assertion`,
`expected`, `actual`, `excerpt`, `frame` identity and `truncated` fields plus typed kind/code/effect/retry.

`assertSnapshotFile(frame, path, {profile: 'spans', update: false})` preserves existing unmasked
baseline bytes. Updates require `update: true`, never an environment variable. Invalid snapshot
names are validation errors, not negatable failures. Files are bounded to 4 MiB, require regular
no-follow targets/ancestors, and updates use an atomic private temporary file and directory lock
(competing publishers are rejected). Failed pre-publication validation leaves the baseline unchanged.

`maskedSnapshot(frame, {profile: 'cells', mask_regions: [{top:0,left:40,right:60,bottom:1}]})`
and `assertMaskedSnapshotFile(frame, path, options, update?)` use a **separate**, versioned
`termtrace-masked-snapshot-v1` comparison format. Masks require exact spans/cells width maps, wholly
contained nonempty absolute half-open rectangles, and cannot split wide graphemes/wrap-edge spacers.
Their union is canonical and redundant row text is omitted. **Masks are NOT redaction:** canonical
recordings, semantic screenshots and debug bundles remain unmasked. Frame identities are not changed.

`withTermtrace` keeps its original signature/context fields, thrown-value identity (including frozen
errors, primitive and undefined throws), and original cleanup objects when supplied by its transport.
It delegates real stop/completion/force-close, verification, artifact capture and retention to Rust;
there is no JavaScript cleanup engine. Admitted unawaited `record()` promises are joined before cleanup.
Only the supplied `record` function is tracked, not direct client launches.

Options: `evidencePolicy: 'on_failure' | 'always' | 'manual'`, `finalizationTimeoutMs` (default 10000).
Manual finalizes but never deletes. Context `finalize(outcome?)` is exactly once; `report` becomes the
shared `FinalizationReport` (`outcome`, nullable `evidence_directory`, `verified_artifacts`,
`secondary_failures: {phase,message,truncated}[]`, `retained`). Explicit finalization commits the
outcome: later assertions cannot undo deletion. Ordinary callback helpers cannot observe failures
after callback return; keep assertions inside or use Always. Reports describe only surviving paths;
partial deletion cannot be rolled back. The existing Rust-named evidence manifest format is preserved.

A failing body remains primary; secondary details go to `error.termtrace` when possible and stderr
otherwise, preserving `cleanupErrors`. A passing body with cleanup/verification failures throws
`AggregateError`. Every sibling is attempted. Fatal `SessionCompletion.failure` (for example
`spawn_failed`) fails even a passing callback and retains evidence; ordinary application exit 7
without such a failure is not itself a helper assertion failure.

Stop/completion workers share the original recorder authority, never a substitute attachment.
Finalization seals queued lifecycle-worker admission and drains admitted lifecycle I/O before
disposal; a drain timeout retains evidence and arbitrarily blocked filesystem I/O may finish later.
Per session, the configured graceful stop/completion deadline is followed by up to **2 seconds**
force/reap and **2.25 seconds** lifecycle-observer drain, plus polling/scheduling and artifact /
verification / filesystem work. The helper fallback briefly resumes an owned paused recorder to
reap its killed group (up to 250ms **inside** the force budget), then kills/reaps the recorder.
Ordinary `forceClose()` keeps its abrupt-stop semantics. On a force-reap timeout original authority
is retained, or its actual Child is transferred to an eventual reaper on last-handle release.
Recorded-group signals are refused when the original leader identity is no longer verifiable.
These bounds are not instant cancellation or hard wall-clock guarantees on stalled storage,
uninterruptible OS I/O or forced runner termination. Native launch/control has its own bounds.

Runnable ordinary-test smoke test (dev-only pinned Vitest, no runtime runner dependency):

```sh
npm --prefix tests/consumers/node_native ci
TERMTRACE_BIN="$PWD/target/debug/termtrace" \
  tests/consumers/node_native/node_modules/.bin/vitest run --root tests/consumers/node_native
```

If a direct client launch creates untracked canonical data in the attempt root, automatic root
removal is conservatively refused: those recordings remain caller-owned and must be stopped by
the caller. Await all direct/mutating operations inside the callback; operations deliberately left
running beyond its boundary need caller-managed cleanup and Always retention.

### Source-bound event and output pages (task05 increment)

`recording.output().page(undefined, 65536)` returns `EvidenceOutputPage` with exact
`Uint8Array` bytes, a source/chain-bound cursor, snapshot-relative `has_more`, and
`incomplete_tail`. Pass the retained cursor for the next page. `recording.events().page(cursor,
100, false, ['output'])` returns nested canonical event unions and exact hashes/timestamps.
`.events().follow()` retains actor start/durable positions or validated completion;
`.output().waitFor(Buffer.from('READY'), 5000)` returns the structured Rust wait outcome.

Every schema-identified wide integer is `bigint`, including optional observed timestamps;
application attributes stay bounded JSON and are not globally name-revived. `Events(bytesPath)`
and `Output(bytesPath)` accept exact Linux filesystem bytes. Display paths are non-authoritative.
`cursorToJson` / `cursorFromJson` explicitly use decimal strings/Base64 without a JS wide-number
JSON parse. Legacy numeric seeks now reject unsafe/fractional/overflow input instead of
truncating/saturating. Unknown verification levels reject instead of downgrading to content.
Normal unversioned native builds and the handwritten loader remain unchanged. This is not full
task05 diagnostics/evidence/management certification.

### Typed recovery metadata

`client.integrity(bytesPath).recover(destinationBytes)` returns the nested recovery report with
`bigint` counters and exact `Uint8Array` path bindings; `recoverLog` handles an individual event log.
The absent destination must be separate and outside the source session. Existing destinations,
in-place/inside-source destinations (including parent symlink aliases), invalid headers and failed
publication reject. Recovery retains a validated prefix, not a completion/no-output-loss claim.
Abandoning an asynchronous operation never implies rollback.

Persisted valid UTF-8 recovery paths retain their historical string serialization bytes. Non-UTF8
paths are serialized as their exact byte array. Canonical session/event encodings are unchanged.

### Typed investigation and management

Use `.analysis()` on Session/Recording (or the client's byte-path factory) for typed timeline,
diff, explain, diagnose, last-action, markers, cache and reviewer reports. `.evidence()` / `.integrity()`
provide all save formats/state selectors, explicit-profile bundles, caller-source/profile verification,
trust-aware verification/signing, recovery and asciicast. New methods return nested reports, never
JSON strings or booleans hiding operational errors. The old report methods remain deprecated migration
adapters for one compatibility interval; their return shapes and units are not silently changed.

Bundle verification never trusts a bundle's own claimed source. Unknown profiles/levels reject.
Screen/Debug are sensitivity policies, not redaction or secret-safety promises. Semantic PNG/SVG keep
their semantic truth labels and renderer/resource/source manifests; they are not original native capture.
An unsigned receipt, cryptographically valid but untrusted receipt and trusted signer are different
outcomes. Supply an explicit allowed-signers path for trust, and an explicit key path/identity for signing;
no user key is discovered or logged by these facades.

Client `.registry()` supports explicit byte roots, optional missing-alias resolution, lifecycle,
register/remove, dry-run/default GC and explicit apply, plus shared typed restart. Corruption,
permissions and invalid canonical sources propagate, never become optional absence. Registry removal
preserves recordings. Restart keeps completion/lineage/environment commitments and alias CAS; on a
failed successor admission inspect the predecessor alias and retained orphan/logs rather than assuming
rollback or blindly retrying. Deliberate environment changes need explicit permission.

`.tmux()` delegates real status/record/track/stop/history/catalog/bundles to the existing recorder and
shared track effects. Generated setup uses the selected Termtrace executable, not the Python/Node host;
old Rust/CLI defaults remain. Scope, coverage, gaps, rollback and capture policies are unchanged. Tests
use private sockets and verify actual completed track evidence, not merely a returned live label.

`.instrumentation()` emits explicit acknowledged events/spans with actual actor boundaries and
process/span identities. Attributes remain bounded JSON; unsafe integers reject and arbitrary field
names are not revived as counters. Dropping an emitter never ends a span. `.system()` exposes actual
capability readiness/remediation, versions and canonical schemas. Missing native dependencies are
blocked availability, not passing skips.

Python `aio` exposes all these namespaces off-loop, including attached `AsyncSession` via async
`attach` / `attach_session_target`; legacy `attach_target` retains its sync return. Cancellation does
not undo already-started signing, GC, recovery, restart or emission. Original Recording ownership,
finalizer admission/cache, primary failures and Stop mutation-effect behavior remain unchanged.

Local scoped tests are recorded in the Task05 handoff; the exposed-unverified inventory is not itself
independent parity certification or permission to start downstream work.

Application attributes remain bounded JSON (64 KiB encoded event limit). SDK conversion rejects cyclic/excessively nested input at the JSON parser’s 128-level bound and rejects oversized attribute work before emission; arbitrary attribute names never trigger integer/byte revival.

### Management failures are not actor retries

Narrow `error.scope` first. Actor operations retain the admitted `MutationEffect`/`Retry` mapping, including Unknown → same request ID. Registry restart and tmux management failures instead expose `scope: "management"`, null/None `effect` and `retry`, `guidance: "inspect_no_automatic_retry"`, and typed `management` progress facts. There is no management request ID or deduplication ledger. Inspect actual predecessor completion, successor readiness/orphan paths, alias ownership, committed aggregate stop and completed track operations before a deliberate new action. Workflow steps are never `Partial.completed_atoms`. Paths in these facts are exact bytes, and counters remain int/bigint. Rust callers use `Error::failure_context()` (`actor_effect()` is absent for management); legacy effect/retry accessors must not be composed into a management replay rule.

Nullable wide report fields preserve explicit null/None (and canonical absence where allowed); guard them before arithmetic. Typed map values, including `pane_tracks`, follow their value schema; identically named application attributes remain ordinary bounded JSON.

### Verification report paths and nonUTF8 bundles

Debug/forensic `verification.json` keeps the historical UTF8 path string byte-for-byte. A nonUTF8 session path is serialized as its exact Unix byte array, so only those bytes identify it. Source/profile/member comparisons remain strict, including after a changed verification member is rehashed.

### Original recordings

`recordOriginal` runs the command in a private Ghostty window on a private X11 display and records the window's pixels from before the first output byte. `session()` controls the canonical session as usual. A resize moves the window inside the canvas and rejects with `exceeds_native_canvas` when it does not fit. `grid` and `viewport` cannot be combined. `fps` is 24, 30 or 60.

```ts
const original = await tt.recordOriginal(['my-app'], {
  grid: { cols: 100, rows: 30 },
  canvas: { width: 1600, height: 900 },
  signal: AbortSignal.timeout(60000),
})
await original.session().waitForOutput('Ready', 10000)
try {
  const result = await original.finish(60000)
  console.log(result.canonical_success, result.capture.directory_bytes)
} catch (error) {
  if (error.kind !== 'original_capture_failed') throw error
  console.log(error.code, error.completion)
}
```

The application outcome and the capture are separate. `canonical_success` is false when the application failed, but its capture can still be published. A capture that does not publish rejects with `original_capture_failed` even when the application completed; `completion` then holds the canonical completion. `original_finalization_timeout` keeps the recording owned, so call `waitFinished` again or `forceClose()`. `captureStatus()` reads the supervisor status and reports `abandoned` when the supervisor is gone. Aborting startup tears the capture down before rejecting.

`attachOriginalTarget(target, stateRoot?)` attaches to a running original recording by path or alias, such as `'name:demo'`. Aliases resolve in the registry under `stateRoot`. A string or `Uint8Array` follows the `path:`/`name:` rules. `pathBytes` is the exact session directory. `session()` gives live control for any path, including one that is not UTF-8; `Session.path` is then a display string and `Session.pathBytes` is exact.

## Editing presentation videos

```js
const { Edit, Termtrace, scope, select } = require('@termtrace/core');

const edit = new Edit({ resource_base: 'assets' }) // relative subtitle paths need a base
  .clip(select.marker('start'), select.marker('done'), { speed: { numerator: 3, denominator: 2 }, hold_ms: 500 })
  .annotate({ text: 'Demo', scope: scope.wholeVideo() })
  .subtitles('captions.ass');
const tt = new Termtrace();
const plan = await tt.videoPlan(session, edit, { renderer: 'semantic', fps: 30 });
await tt.videoRender(session, edit, 'demo.gif', { renderer: 'semantic' });
```

64-bit selector values are `bigint` (a safe-integer `number` is accepted). `renderer: 'native'`
never falls back to semantic pixels. Renders refuse an existing output; `overwrite: true`
replaces only an MP4. `edit.normalize()` returns the canonical edit and its `edit_plan_hash`.
Rejected edits carry an edit `code` and `details` (`field`, `clip_index`, `marker`). Frame
rounding: clip start floors and end ceils to frames, the retimed count is
`ceil(frames * denominator / numerator)`, holds are `ceil(hold_ms * fps / 1000)` frames, output
frames map back with a floor, and `select.nativeFrame(n)` selects frame `n` exactly.
