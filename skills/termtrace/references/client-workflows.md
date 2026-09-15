# Programmatic SDK workflows

Use individual CLI commands (`termtrace send`, `termtrace screen --json`, etc.) when a workflow will finish in
two or three actions. Beyond that, use the canonical Rust SDK (`termtrace_sdk`)
so repeated observations and actions do not pay process startup and discovery costs. Use `termtrace test`
when assertions should be repeatable scenarios. Programmatic workflows in Rust are appropriate for branching, loops,
multiple sessions, or caller-owned retry IDs.

## Rust SDK for Stateful Workflows

When orchestrating multi-step interactive workflows, use `termtrace_sdk`:

```rust
use std::time::Duration;
use termtrace_sdk::{
    InputAtom, NamedKey, ScreenPredicate, StableCaptureOptions, Terminal,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let terminal = Terminal::new("termtrace", ".termtrace-evidence");
    let mut session = terminal
        .launch(["my-app", "--arg"])
        .start()?;

    // Wait for text or readiness predicate
    session
        .screen()
        .wait(
            ScreenPredicate::Contains { text: "Ready".to_owned() },
            Duration::ZERO,
            Duration::from_secs(5),
        )?;

    // Send input
    session.keyboard().text("help\n")?;

    // Wait for updated screen
    session
        .screen()
        .wait(
            ScreenPredicate::Contains { text: "Commands".to_owned() },
            Duration::ZERO,
            Duration::from_secs(5),
        )?;

    let frame = session.screen().frame(Default::default())?;
    println!("{}", frame.viewport_text);

    session.stop(Some(Duration::from_secs(5)))?;
    Ok(())
}
```

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
