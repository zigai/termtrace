# Deterministic test isolation

Termtrace resolves one immutable child environment before it writes the manifest or spawns the PTY child. The same map drives executable lookup, provenance, and `exec`, so host `PATH` and recorded `PATH` cannot disagree.

## Policy and precedence

Resolution order is:

1. snapshot the recorder environment;
2. inherit it or begin empty;
3. apply `unset`, then named `copy`, then explicit `set`;
4. apply the color policy;
5. set recorder-owned `TERM` and `COLORTERM`.

A name cannot appear in conflicting operations. Recorder-owned terminal/color names cannot be set, copied, or unset directly. `auto` preserves inherited color variables, `always` requests color through common variables, and `never` sets `NO_COLOR` plus zero-valued force-color variables.

Shared executable programs used to exercise terminal behavior live under
`fixtures/programs/terminal/`. Cross-SDK atomic action data lives at
`fixtures/conformance/actions/atomic-action.json`; it is data rather than an
executable test program, so every caller applies the same request and expected
result while retaining its own public API path.

## Rust

```rust,no_run
use std::{ffi::OsString, time::Duration};
use termtrace::{client::Terminal, environment::ColorPolicy};

let terminal = Terminal::new("termtrace", "test-evidence");
let mut session = terminal
    .launch([OsString::from("/usr/bin/my-tui")])
    .clear_environment()
    .copy_environment("PATH")
    .set_environment("API_TOKEN", b"secret".to_vec())
    .color_policy(ColorPolicy::Never)
    .output_byte_budget(4 * 1024 * 1024)
    .start()?;

let exact = session
    .screen()
    .wait_for_idle(Duration::from_millis(100), Duration::from_secs(10))?;
session.stop(Some(Duration::from_secs(5)))?;
# Ok::<(), termtrace::Error>(())
```

For ordinary Rust tests, `testing::run_terminal_test` creates a collision-free
attempt root and owns exactly one launch, completion, and retention decision:

```rust,no_run
use std::ffi::OsString;
use termtrace::{
    client::Terminal,
    testing::{TerminalTestConfig, run_terminal_test},
};

let terminal = Terminal::new("termtrace", "test-evidence");
let config = TerminalTestConfig::new(
    terminal,
    [OsString::from("/usr/bin/my-tui")],
);
let result = run_terminal_test(config, |test| {
    test.assert_screen_text("Ready")?;
    Ok::<(), termtrace::testing::TerminalAssertionError>(())
});
assert!(result.is_ok());
```

Passing attempts are discarded only after canonical content verification.
Failures retain a driver-verified semantic PNG, debug bundle, canonical session,
and verified attempt manifest. Repeated finalization returns the first report;
panic payloads and body errors remain primary while cleanup errors stay bounded
and secondary. Snapshot updates require `TerminalTestConfig::update_snapshots =
true`; normal assertion runs never create or replace reviewed snapshots.

Environment JSON is sent over a private stdin pipe. It is not written beneath the evidence root and values do not enter recorder arguments.

## TypeScript

```ts
await using terminal = await Termtrace.create({ binaryPath: "termtrace" });
await using session = await terminal.launch({
  command: ["/usr/bin/my-tui"],
  evidenceRoot: "test-evidence",
  environment: {
    inheritEnv: false,
    copy: ["PATH"],
    set: { API_TOKEN: process.env.API_TOKEN! },
    color: "never",
  },
  outputByteBudget: 4 * 1024 * 1024,
});

const exact = await session.screen.waitForIdle({ stableForMs: 100, timeoutMs: 10_000 });
```

## Python

```python
from termtrace import EnvironmentOptions, Terminal

with Terminal(binary_path="termtrace", evidence_root="test-evidence") as terminal:
    with terminal.launch(
        ["/usr/bin/my-tui"],
        environment=EnvironmentOptions(
            inherit_env=False,
            copy=["PATH"],
            set={"API_TOKEN": b"secret"},
            color="never",
        ),
        output_byte_budget=4 * 1024 * 1024,
    ) as session:
        exact = session.screen.wait_for_idle(stable_for=0.1, timeout=10)
```

The async Python client exposes the same launch fields and `await session.screen.wait_for_idle(...)`.

## Test framework capability matrix

| Capability | Rust built-in harness | pytest | Vitest |
| --- | --- | --- | --- |
| Ownership | Fresh driver session per helper call | Fresh sync or native-async driver and session per fixture factory call | Fresh driver and session per runnable attempt |
| Exact stable/action frame | Yes | Sync and native async | Yes |
| Locator assertions | Runner-neutral typed results | Pytest-friendly typed assertion façade | Typed async matchers |
| Semantic snapshots | Termtrace JSON; explicit config update | Termtrace JSON; `--termtrace-update-snapshots` | Vitest snapshot engine; `vitest --update` |
| Failure evidence | Verified canonical session, semantic PNG, debug bundle | Verified canonical session, semantic PNG, screen bundle | Verified canonical session, semantic PNG, debug bundle |
| Failure precedence | Body error or panic remains primary | Test failure remains primary | Test failure remains primary |
| Parallel isolation | UUID attempt roots | Worker, node ID hash, and attempt | Worker, module/test hash, retry, repeat, and session |
| Skip behavior | Ignored test never calls the helper | Filtered/static skip launches nothing | Filtered/skipped/todo test launches nothing |
| Timeout integration | No cancellation hook in the standard harness; callers can finalize as `TimedOut` | Framework outcome classification plus bounded teardown | Vitest cancellation signal plus bounded teardown |
| Supported platform | Linux | Linux | Vitest 4.1.3–4.x on Linux |

All three use `on-failure` by default: a fully successful attempt is discarded,
while assertion, exception, panic, or timeout outcomes retain bounded artifacts.
Teardown first requests stop and completion, so owned recorder and child process
groups are reaped before the adapter returns. Artifact paths are contained under
the caller's evidence root. Evidence contains exact terminal output, can contain
secrets, and is not automatically redacted.

## Scenario

```json
{
  "version": 2,
  "name": "isolated TUI",
  "launch": {
    "command": ["/usr/bin/my-tui"],
    "environment": {
      "base": "clear",
      "set": {},
      "copy": ["PATH"],
      "unset": [],
      "color": "never"
    },
    "outputByteBudget": 4194304,
    "timeoutMs": 10000
  },
  "steps": [{ "matchScreenSnapshot": { "name": "ready", "stableForMs": 100 } }]
}
```

Failure copies replace explicit environment values with `<redacted>`. The canonical manifest contains names and commitments, not secret values.

Restarts compare the reconstructed map with the predecessor commitment. Resupply explicit secrets through `restart --environment-spec`; add `--allow-environment-change` only for an intentional successor override.

## Limits and typed idle outcomes

Output budgets retain every observed byte and then terminate the process group. This avoids the false evidence created by transcript truncation.

Atomic idle capture can return `stable`, `child_completed`, `output_closed`, or `deadline`. CLI and clients reject every non-`stable` result by default. Opt in to incomplete outcomes only when the caller will inspect the quality explicitly.
