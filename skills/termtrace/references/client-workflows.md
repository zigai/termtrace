# Programmatic SDK workflows

Use individual CLI commands (`termtrace send`, `termtrace screen --json`, etc.) when a workflow will finish in
two or three actions. Beyond that, use the canonical Rust SDK (`termtrace_sdk`)
so repeated observations and actions do not pay process startup and discovery costs. Use `termtrace test`
when assertions should be repeatable scenarios. Programmatic workflows in Rust are appropriate for branching, loops,
multiple sessions, or caller-owned retry IDs.

## Rust SDK for Stateful Workflows

When orchestrating multi-step interactive workflows, use `termtrace_sdk`. One `Session`
type carries every operation the CLI, agent driver, and Python/Node bindings use:
`Terminal::start`/`attach`, `act`, `wait_screen`, `capture_stable`, `observe`, `frame`,
`stop`, `complete`, and `detach`. Defaults (geometry, timeouts, idle interval) live in
`termtrace_sdk::defaults`.

```rust
use std::time::Duration;
use termtrace_sdk::{
    screen::frame_text, ActRequest, KeyChord, LaunchSpec, ScreenPredicate, StopRequest,
    Terminal,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let terminal = Terminal::new("termtrace", ".termtrace-evidence");
    let mut session = terminal.start(LaunchSpec::new(["my-app", "--arg"]))?;

    // Wait for a readiness predicate; `satisfied()` is the one match rule.
    let ready = session.wait_screen(
        ScreenPredicate::Contains { text: "Ready".to_owned() },
        Duration::ZERO,
        Duration::from_secs(5),
    )?;
    assert!(ready.satisfied());

    // Apply input and its postcondition under one actor ordering point.
    let enter = KeyChord::parse("enter")?.ok_or("enter is a key")?;
    let outcome = session.act(
        ActRequest::new()
            .text("help")
            .key(enter)
            .if_state(ready.frame.state_hash)
            .until_screen(
                ScreenPredicate::Contains { text: "Commands".to_owned() },
                Duration::from_secs(5),
            ),
    )?;
    println!("{}", outcome.frame().map(frame_text).unwrap_or_default());

    session.stop(StopRequest::default(), Some(Duration::from_secs(5)))?;
    Ok(())
}
```

Every failure states what it did to the session: `error.effect()` is a `MutationEffect`
and `error.retry()` derives whether a new request, the same request ID, or inspection
comes next. `error.code()` keeps the actor's rejection code (`stale_observation`,
`action_busy`, ...) and `error.details()` its actor-consistent context.

## Native Python SDK (`termtrace`)

For Python test suites (`pytest`), agent loops, or automation scripts, use the native PyO3 binding:

```python
from termtrace import Termtrace

tt = Termtrace()

# Synchronous context manager ensures child processes are reaped on exit
with tt.record(["bash", "--norc"], rows=24, cols=80) as rec:
    screen = rec.screen()
    state = screen.wait_for_text("$", timeout_secs=5.0)

    # State-guarded action sequence
    actions = rec.actions()
    actions.if_state(state.hash)
    actions.send_text("echo hello\n")
    actions.until_screen("hello", timeout_secs=5.0)
    actions.execute()

    frame = screen.frame()
    assert "hello" in frame.text
```

Keys use the same chord grammar as `termtrace send`: `send_key("ctrl+shift+F5")`,
`send_key("page-up")`. Every exception derives from `TermtraceError` and carries `kind`,
`code`, `effect` (a dict such as `{"kind": "not_applied"}`), and `retry`; a stale
`if_state` guard raises `StateGuardFailedError` with `code == "stale_observation"`, and an
unmet wait raises `TimeoutError` with `kind == "wait_unsatisfied"`.

For `asyncio` workflows:

```python
import asyncio
from termtrace.aio import AsyncTermtrace

async def main():
    tt = AsyncTermtrace()
    async with await tt.record(["bash"]) as rec:
        screen = rec.screen()
        state = await screen.wait_for_text("$")
        print("Prompt ready:", state.hash)

asyncio.run(main())
```

## Native Node.js & TypeScript SDK (`@termtrace/core`)

For Node.js and TypeScript applications, use the native NAPI-RS binding:

```typescript
import { Termtrace } from "@termtrace/core";

const tt = new Termtrace();

// Explicit Resource Management with await using
await using rec = await tt.record(["bash", "--norc"], 24, 80);
const screen = rec.screen();
const state = await screen.waitForText("$", 5000);

const actions = rec.actions();
actions.ifState(state.hash);
actions.sendText("echo from-node\n");
actions.untilScreen("from-node", 5000);
await actions.execute();

const frame = await screen.frame();
console.log("Output:", frame.text);
```

Thrown errors are `TermtraceError`s with `kind`, `code`, `effect`, and `retry`:

```typescript
import type { TermtraceError } from "@termtrace/core";

try {
  await actions.execute();
} catch (error) {
  const { code, retry } = error as TermtraceError;
  if (code === "stale_observation" && retry === "new_request") {
    // observe again, then rebuild the action
  }
}
```

For test suites:

```typescript
const { withTermtrace } = require("@termtrace/core/testing");

await withTermtrace(async ({ record }) => {
  const rec = await record(["my-cli", "--help"]);
  const state = await rec.screen().waitForText("Usage:");
  assert.ok(state.text.includes("Usage:"));
});
```

## Repeatable Scenarios

For assertion-heavy test workflows across CI or test matrices, use `termtrace test` with scenario JSON files or programmatic scenarios assembled with typed actions and predicates.
