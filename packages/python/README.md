# Termtrace for Python

Typed synchronous and asynchronous clients for the versioned Termtrace driver. The wheel includes the matching `termtrace` executable on supported platforms.

```python
from pathlib import Path

from termtrace import (
    ActionOptions,
    Duration,
    KeyAtom,
    ScreenPostcondition,
    Terminal,
    TextAtom,
    text,
)

with Terminal(evidence_root=Path(".termtrace-evidence")) as terminal:
    with terminal.launch(["my-terminal-app"], evidence="on-failure") as session:
        action = session.act(
            [TextAtom("help"), KeyAtom("enter")],
            ActionOptions(
                postcondition=ScreenPostcondition(
                    {"predicate": "contains", "text": "Commands"},
                    Duration.seconds(5),
                )
            ),
        )
        command = action.frame.query(text("Commands")).single()
        assert command.text == "Commands"
```

`Frame`, `Observation`, `SemanticDiff`, `DiagnosticReport`, `Completion`, `VerificationReport`, and related return values are concrete generated models. Invalid or version-skewed driver messages raise `TermtraceProtocolError` without including the raw payload.

Use `AsyncTerminal` for the equivalent native-asyncio lifecycle.

`Session.act()` and `AsyncSession.act()` apply one guarded atom sequence and
evaluate its postcondition without releasing actor ordering. The returned
`ActionResult.frame` is the exact frame selected at that ordering point; do not
issue a second observation to infer whether the postcondition matched.
`screen.capture_stable()` likewise returns the exact stable frame together with
its timing and quality. Deadline, child-completed, and output-closed results are
rejected unless `allow_incomplete=True`; protocol, stale-state, busy, partial
input, and resource-limit failures remain distinct and are not retried
automatically.

## Rich frames and locators

`screen.frame()` returns immutable styled spans by default.
`screen.cells(Region(...))` returns exact bounded cells; regions are absolute,
zero-based, nonempty, and half-open. Invalid or oversized regions fail instead
of clipping.

Termtrace locators match terminal cells in one viewport state. They are not
application widgets or accessibility roles. String queries are row-local
substrings unless `exact=True`; `re.Pattern[str]` uses Python regex semantics,
and `cross_soft_wraps=True` is the explicit opt-in for linked native soft-wrap
rows. `single()` rejects absence and ambiguity; use `first()` or `nth()` only
when choosing among duplicates is intentional. Style `#RRGGBB` values compare
resolved RGB, while `source_*` filters compare default, palette, or source RGB
selectors. Every result retains state/projection identity and presented/capture
quality through its immutable frame.
`keyboard.sequence` admits all typed text, byte, key, and paste atoms under one
optional `if_state` guard and caller-owned `request_id`.

The public session façade is handwritten. Internal request and response models are generated from the Rust-owned JSON Schemas and decoded with `msgspec`; do not edit files under `termtrace/_generated/`.

## Pytest integration

Pytest support is optional. The root `termtrace` package does not import pytest.
Install the synchronous adapter with:

```console
pip install 'termtrace[pytest]'
```

Install native async fixtures, which are awaited by pytest-asyncio and never run a
hidden event loop, with:

```console
pip install 'termtrace[pytest-async]'
```

Each factory call owns a fresh driver, session, and collision-resistant evidence
directory for one test attempt:

```python
def test_help(termtrace_test_factory):
    test = termtrace_test_factory(["my-terminal-app"])
    test.screen.wait_for_text("Ready")
    test.assertions.screen_text("Ready")
    test.assertions.snapshot("help-screen")
```

For a single declarative launch, use
`@pytest.mark.termtrace(command=["my-terminal-app"])` with the `termtrace_test`
fixture. Async tests use `async_termtrace_test_factory` or
`async_termtrace_test` and must await terminal operations. Fixtures are
function-scoped; filtered and statically skipped tests start no process. A skip
raised after fixture setup is finalized as an owned attempt.

The default `on_failure` policy discards evidence only after the test and all
finalization phases succeed. `always` retains every attempt. `manual` retains
evidence while the adapter still closes its owned process. Set the policy with
`--termtrace-evidence`, the evidence root with
`--termtrace-evidence-root`, and supported finalization phase bounds with
`--termtrace-finalization-timeout`. Retained evidence can contain terminal
secrets and is not redacted.

On failure the adapter first stops and completes the owned session, then requests
a semantic screenshot, a bounded screen bundle, and canonical content
verification through the public SDK. Screenshot and bundle responses are
independently verified by the Rust driver before the SDK accepts them. The
synchronous transport applies explicit per-request deadlines to artifact and
verification phases; native-async finalization bounds the corresponding awaits.
Cleanup and artifact failures fail a passing test. When the test already failed,
they remain secondary and do not replace its primary error.

Snapshots require a stable logical name and reject absolute paths, traversal,
and path separators. They are read-only by default. Pass
`--termtrace-update-snapshots` to create or update them explicitly. Reusing one
logical name with a different normalization mode in the same test is rejected.
Locators exposed by the public session may be used for assertions; locator-driven
actions are intentionally unsupported.
