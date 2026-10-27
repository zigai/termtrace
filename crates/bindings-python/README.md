# termtrace (Python)

Native Python bindings for Termtrace terminal recording and semantic debugging, powered by PyO3.

## Async operations

Use `termtrace.aio.AsyncTermtrace` in asyncio applications. All blocking recording
and client operations are awaitable and run off the event-loop thread, including
pointer input, saving artifacts, reading/waiting for output, diagnostics,
verification, recovery, and registry operations:

```python
from termtrace.aio import AsyncTermtrace

async def check_app():
    tt = AsyncTermtrace()
    async with await tt.record(["my-app"]) as rec:
        await rec.screen().wait_for_text("Ready")
        await rec.wait_for_output("Ready", timeout_secs=5.0)
        await rec.save("./ready-screen", formats=["png"])
```

Local properties and handle/builder creation (`path`, `session_id`, `is_active()`,
`screen()`, and `actions()`) remain synchronous. Run an action builder with
`await builder.run()`, or the legacy `await rec.execute_action(actions)`.

Code that previously called synchronous methods on an async wrapper must now
`await` them. The synchronous `termtrace.Termtrace` API is unchanged. Cancelling an
await does not forcibly interrupt an operation already running in a worker thread;
inspect the session before retrying a mutation whose outcome is uncertain. Locator
`wait_for` is the exception: cancelling it stops the worker, as described below.

## Screen observation and locators

`rec.screen()` returns frames, observations, and lazy locators. The shared Rust engine
does all matching. Data shapes are the TypedDicts in `termtrace.types`, generated from the
Rust model. They use its keys, exact `int` values (including u64 fields), and `bytes` for byte
fields. Cell coordinates are zero-based columns and rows. Regions and match ranges
are half-open. Durations are `*_secs` floats, capped at one hour.

```python
from termtrace import CancelToken

screen = rec.screen()
frame = screen.frame(detail="cells")
save = screen.get_by_text("Save").in_row(3)
save.wait_for(timeout_secs=5.0)
screen.get_by_regex(r"(?i)error").wait_for("absent", timeout_secs=5.0)
screen.get_by_text("Item").wait_for({"count": 2}, cancel=CancelToken())
```

- Literal text matches overlap. Regex uses Rust `regex` syntax, and its matches do not
  overlap.
- `single()`, `nth()`, and failed waits raise `QueryError`, `LocatorTimeoutError`, or
  `CancelledError`. Each carries `kind`, `query`, `state_hash`, and, for timeouts and
  cancellation, the last `frame`.
- `CancelToken.cancel()` stops a waiting `wait_for` from another thread. Cancelling an
  `AsyncLocator.wait_for` task does the same, then waits for the worker to return.
- A wait notices cancellation within one capture quantum (250 ms) plus a round trip.
  If the recorder is busy, one capture can take up to about 5.25 s.
- `wait()` and `wait_stable()` keep their old behavior and cannot be cancelled.

## Interaction and session control

`rec.actions()` builds one input request. Its `request_id` exists from the start.
`run()` freezes the request and returns an `ActionResult` with separate `input` and
`postcondition` outcomes. Calling `run()` again resends the same request; the recorder
applies it at most once. Changing a frozen builder raises `invalid_operation`. The
legacy `execute()` sends a fresh request each call and returns a `ResolvedState`.

```python
builder = rec.actions().send_text("y").send_key("enter")
builder.until_screen_where({"predicate": "contains", "text": "Saved"}, timeout_ms=5000)
result = builder.run()
rec.pointer().activate("[OK]", strict=True)
rec.send([{"step": "mark", "label": "before"}, {"step": "resize", "cols": 100, "rows": 30}])
limited = tt.launch(["my-app"], {"environment": {"base": "clear", "set": {"TOKEN": b"x"}}, "max_duration_ms": 60000})
```

- Mutation errors carry `request_id`, `effect`, and `retry`. `postcondition_incomplete`
  also carries the applied `result`; a failed `send` carries `step` and `send_request_id`.
  Side effects are never rolled back.
- `pointer()` results say `applied`, `noop` (the application does not report that input),
  or `activated`. With `strict=True`, a no-op is an error instead.
- `status()`, `request_stop(request_id, wait_ms)`, and `wait_for_completion(timeout_ms)`
  report live and completed sessions. Option durations are whole milliseconds.
- A cleared launch environment still gets the reserved terminal variables. Output the
  application prints is recorded as is, including any secret it prints.
- In `termtrace.aio`, cancelling `await builder.run()` does not stop the action. Call
  `await rec.wait_pending()`, then `await builder.run()` to learn its outcome.

## Ordinary tests: immutable assertions and owned evidence

```python
from termtrace.testing import terminal_test, assert_screen_text, assert_snapshot_file

with terminal_test(binary="target/debug/termtrace") as test:
    rec = test.record(["/bin/sh", "-c", "printf READY; cat"])
    rec.screen().wait_for_text("READY", 5)  # seconds, explicit waiting
    frame = rec.screen().frame()
    assert_screen_text(frame, "READY")      # this exact capture; no hidden retry
    assert_snapshot_file(frame, "reviewed/ready.json", profile="spans", update=False)
```

Create/review a file baseline deliberately with `update=True`. No environment variable enables
updates. Existing text/spans/cells bytes are unchanged. Missing/different files raise
`ExpectationMismatchError(AssertionError)` with bounded `assertion`, `expected`, `actual`, `excerpt`,
`frame` identity and `truncated` fields. Invalid names/options, insufficient detail, query strictness,
invalid regexes and filesystem failures are **not** mismatches. `assert_not(lambda: ...)` catches
only that mismatch class. Rust now classifies missing row/style detail as an `AssertionError::Query`
(the text, wrap and style functions return `AssertionError`, not just a mismatch).

Other captured-value assertions: `assert_exists`, `assert_not_exists`, `assert_count`,
`assert_match_text`, `assert_match_style`, `assert_cursor`, `assert_no_wrapped_rows`,
`assert_neutral_region`, `assert_no_presented_transient`. Use `frame.query(spec)` to assert against
an immutable query result/selected match. Optional cursor coordinates remain `None`; presentation
checks describe one frame, not absence of past/future transients.

`normalized_snapshot(frame, profile="spans")` returns a deterministic string for runner-native
diffs. `masked_snapshot(frame, {"profile": "cells", "mask_regions": [...]})` and
`assert_masked_snapshot_file(...)` use a **separate** `termtrace-masked-snapshot-v1` comparison
format. Masks use absolute zero-based half-open rectangles inside the returned region. They require
exact spans/cells width maps, reject split wide graphemes (including wrap-edge spacers), canonicalize
the union and omit redundant row text. **Masks are stability exclusions, NOT redaction:** recordings,
semantic screenshots and debug bundles remain unmasked. They do not rewrite frame identities.

Retention is `evidence_policy="on_failure"` (default), `"always"`, or `"manual"`. Manual finalizes
but never deletes. Only `test.record(...)` launches are owned; direct client launches are not.
`test.finalize(outcome="passed")` returns the cached `FinalizationReport` (also `test.report`):
`outcome`, nullable `evidence_directory`, `verified_artifacts`, bounded `secondary_failures`
(`phase`, `message`, `truncated`), `retained`. Explicit finalization commits that outcome; later
assertions cannot undo disposal. Put assertions **inside** the context or use Always.

The Rust engine stops, waits, force-closes on failure, verifies, and only then considers disposal.
It attempts every sibling. Body exceptions, including `BaseException`, remain primary; secondary
reports are attached as `error.termtrace` or logged when attachment is impossible. Passing-body
cleanup/verification failures raise `FinalizationError`. Reports include only actual surviving paths;
partial deletion is not rollback. The existing `termtrace-rust-test-evidence-v1` manifest name/format
is retained. Multiple sessions have separate `.test-artifacts-<session-id>` directories. Finalization
is cached on both the adapter and recordings, so layered recording contexts do not repeat capture.

For asyncio/ordinary pytest async tests:

```python
from termtrace.testing import async_terminal_test, assert_screen_text

async def test_ready():
    async with async_terminal_test(evidence_policy="always") as test:
        rec = await test.record(["/bin/sh", "-c", "printf READY; cat"])
        ready = await rec.screen().wait_for_text("READY", 5)
        assert_screen_text(ready.frame, "READY")
```

Teardown is off-loop and shielded through repeated cancellation; admitted late launches remain
owned. Pending worker joins use the configured timeout once before per-session cleanup. A stop or
completion worker shares the original recorder authority; cancellation cannot replace it with an
ownerless attachment or restore a finalized handle. Shared finalization seals late lifecycle-worker
admission and drains admitted lifecycle I/O before disposal. A drain timeout retains evidence;
arbitrarily blocked filesystem I/O can finish later. Cancellation never implies input rollback.

Each session gets one configured graceful stop/completion deadline, then up to **2 seconds** for
force/reap and **2.25 seconds** for lifecycle-observer drain, plus polling/scheduling and artifact /
verification / filesystem work. The helper fallback gives an owned paused recorder up to 250ms
**within** the force budget to reap its killed child group, then kills/reaps the recorder. Ordinary
`force_close()` keeps its abrupt-stop semantics. A force-reap timeout retains original authority;
last-handle release transfers the actual Child to an eventual reaper, never a reconstructed PID.
A recorded-group signal is refused if its original leader identity is no longer verifiable.

These are not hard wall-clock guarantees on stalled storage, uninterruptible OS I/O, forced runner
termination, or arbitrarily slow custom launch functions. Native launches and other control reads
have their own bounds (a legacy busy read cancellation can take about 5.25 seconds). Legacy
`Recording` / `AsyncRecording` contexts finalize with Manual retention and preserve body exceptions.
A passing body with fatal `SessionCompletion.failure` (for example `spawn_failed`) is a cleanup
failure and retains evidence; an ordinary application exit 7 without such a failure is not itself a
helper assertion failure.

The optional pytest plugin keeps `termtrace` and callable `termtrace_session` fixtures. It observes
setup, call **and teardown** reports before cleanup/disposal, publishes cleanup errors as teardown
failures without replacing original failures, and retains on skip/xfail/xpass. No required runner
is imported by the ordinary helper.

If direct client launches leave untracked canonical data in the attempt root, automatic root
removal is conservatively refused; such recordings remain caller-owned and must be stopped by
the caller. Legacy async input, stop and completion offloads also participate in the shared
pending-worker joins, without changing their request identities or call arguments.

### Source-bound event and output pages (task05 increment)

`recording.output().page(limit=65536)` returns a typed dictionary containing exact `bytes`, a
source/chain-bound cursor, snapshot-relative `has_more`, and `incomplete_tail`. Retain and pass
that cursor unchanged. `recording.events().page(limit=100, metadata_only=False,
event_types=["output"])` returns nested typed canonical event unions, hashes, timestamps and
byte summaries. `.events().follow()` returns actor-authenticated durable positions or validated
completion. `.output().wait_for(b"READY", timeout_secs=5)` returns the structured Rust wait outcome.
`aio.AsyncRecording` exposes the same namespaces off-loop; cancelled reads may continue.

All wide values are Python `int` and byte payloads are `bytes`. Paths are `str`; a non-UTF-8 path
is decoded with `os.fsdecode`, so `os.fsencode` gives the exact bytes. `Events(path)` and
`Output(path)` accept `str`, `bytes` or `os.PathLike`. Cursor `cursor_to_json` / `cursor_from_json`
round-trip the canonical cursor JSON and never write evidence. Unknown verification levels reject.
This is a paging increment, not full task05 diagnostics/evidence/management certification.

### Typed recovery metadata

`client.evidence(path).recover(destination)` returns a nested typed recovery report, with exact
paths and Python integers; `recover_log` recovers an individual event-log prefix.
The destination must be separate, outside the source session, and absent. Existing destinations,
in-place/inside-source destinations (including parent-directory symlink aliases), invalid headers
and failed publication are errors, not partial-success reports. Recovery never supplies a completion
claim and does not establish that no output was lost. Cancellation of an off-loop recovery does not
undo an already-running operation.

Persisted recovery metadata stores a UTF-8 path as a string and any other path as its byte array.
This is not a change to canonical session/event encodings.

### Typed investigation and management

Use `.analysis()` on Session/Recording (or the client's byte-path factory) for typed timeline,
diff, explain, diagnose, last-action, markers, cache and reviewer reports. `.evidence()` provides all save formats/state selectors, explicit-profile bundles, caller-source/profile verification,
trust-aware verification/signing, recovery and asciicast. These methods and the client and session
report methods return nested typed reports, never JSON strings.

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

Narrow `error.scope` first. Actor operations retain the admitted `MutationEffect`/`Retry` mapping, including Unknown → same request ID. Registry restart and tmux management failures instead expose `scope: "management"`, null/None `effect` and `retry`, `guidance: "inspect_no_automatic_retry"`, and typed `management` progress facts. There is no management request ID or deduplication ledger. Inspect actual predecessor completion, successor readiness/orphan paths, alias ownership, committed aggregate stop and completed track operations before a deliberate new action. Workflow steps are never `Partial.completed_atoms`. Paths in these facts are exact, and counters remain int/bigint. Rust callers use `Error::failure_context()` (`actor_effect()` is absent for management); legacy effect/retry accessors must not be composed into a management replay rule.

Nullable wide report fields preserve explicit null/None (and canonical absence where allowed); guard them before arithmetic. Typed map values, including `pane_tracks`, follow their value schema; identically named application attributes remain ordinary bounded JSON.

### Verification report paths and nonUTF8 bundles

Debug/forensic `verification.json` stores a UTF-8 session path as a string and any other path as its byte array, so the exact Unix bytes always round-trip. Native verification reports give the path as `str`, decoded with `os.fsdecode`. Source/profile/member comparisons remain strict, including after a changed verification member is rehashed.

### Original recordings

`record_original` runs the command in a private Ghostty window on a private X11 display and records the window's pixels from before the first output byte. `session()` controls the canonical session as usual. A resize moves the window inside the canvas and fails with `exceeds_native_canvas` when it does not fit. `grid` and `viewport` cannot be combined. `fps` is 24, 30 or 60.

```python
from termtrace import OriginalCaptureFailedError

original = tt.record_original(["my-app"], {"grid": {"cols": 100, "rows": 30}, "canvas": {"width": 1600, "height": 900}})
original.session().wait_for_output("Ready", timeout_secs=10.0)
try:
    result = original.finish(timeout_ms=60_000)
except OriginalCaptureFailedError as error:
    print(error.code, error.completion)
else:
    print(result["canonical_success"], result["capture"]["directory_bytes"])
```

The application outcome and the capture are separate. `canonical_success` is false when the application failed, but its capture can still be published. A capture that does not publish raises `OriginalCaptureFailedError` even when the application completed; `completion` then holds the canonical completion. `OriginalFinalizationTimeoutError` keeps the recording owned, so call `wait_finished` again or `force_close()`. `capture_status()` reads the supervisor status and reports `abandoned` when the supervisor is gone. Cancelling startup tears the capture down before raising. `AsyncTermtrace.record_original` has the same behavior.

`attach_original_target(target, state_root=None)` attaches to a running original recording by path or alias, such as `"name:demo"`. Aliases resolve in the registry under `state_root`. `str` and `bytes` follow the `path:`/`name:` rules, and an `os.PathLike` is an exact path. `path_bytes` is the exact session directory. `session()` gives live control for any path, including one that is not UTF-8; `Session.path` is then a display string and `Session.path_bytes` is exact.
