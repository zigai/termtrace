# Termtrace for TypeScript

Termtrace is a strict Node.js client for evidence-backed terminal tests on Linux. It uses `binaryPath`, `TERMTRACE_BINARY`, or a `termtrace` executable on `PATH`. The root `@termtrace/test` package is runner-neutral; Vitest support is available only from the optional `@termtrace/test/vitest` subpath.

## Runner-neutral client

```ts
import { Duration, Termtrace } from "@termtrace/test";

await using terminal = await Termtrace.create({
  evidenceRoot: ".termtrace-evidence",
});
await using session = await terminal.launch({
  command: ["my-terminal-app"],
  evidence: "on-failure",
});

const action = await session.act(
  [
    { kind: "text", text: "help" },
    { kind: "key", key: "enter" },
  ],
  {
    postcondition: {
      kind: "screen",
      predicate: { predicate: "contains", text: "Commands" },
      timeout: Duration.seconds(5),
    },
    frame: { detail: "spans" },
  },
);

const command = action.frame.query({ kind: "text", query: "Commands" });
if (command.length !== 1) {
  throw new Error(`expected one Commands match, received ${command.length}`);
}
await session.finish(true);
```

`act` returns the frame captured at the exact actor ordering point; do not follow it with another observation to determine whether its postcondition matched. `Screen.frame()` and `Screen.cells()` expose immutable rich frames. Regions are absolute, zero-based, nonempty, and normalized to half-open bounds; invalid or oversized requests fail instead of clipping or silently changing detail.

Termtrace locators match terminal cells in one viewport state. They are not application widgets or accessibility roles. `single()` rejects both absence and ambiguity; use `first()` or `nth()` only when choosing among multiple matches is intentional. Text queries account for terminal columns, combining graphemes, and wide-cell heads and tails. Style filters distinguish source default/palette/RGB values from resolved RGB values.

## Vitest adapter

Install Vitest 4.1.3 or later in the same major version, then import the explicit subpath:

```sh
bun add --dev vitest@^4.1.3
```

```ts
import { expect } from "vitest";
import { Duration } from "@termtrace/test";
import { createTerminalTest } from "@termtrace/test/vitest";

const terminalTest = createTerminalTest({
  termtrace: {
    ...(process.env.TERMTRACE_BINARY === undefined
      ? {}
      : { binaryPath: process.env.TERMTRACE_BINARY }),
    evidenceRoot: ".termtrace-evidence",
  },
  launch: ({ task }) => ({
    command: ["my-terminal-app", "--case", task.name],
    viewport: { cols: 100, rows: 30 },
    evidence: "on-failure",
  }),
});

terminalTest("renders a stable screen", async ({ session, screen }) => {
  const action = await session.act(
    [
      { kind: "text", text: "status" },
      { kind: "key", key: "enter" },
    ],
    {
      postcondition: {
        kind: "screen",
        predicate: { predicate: "contains", text: "Ready" },
        timeout: Duration.seconds(5),
      },
      frame: { detail: "spans" },
    },
  );

  await expect(screen.getByText("Ready")).toExist();
  await expect(screen.getByText("Ready")).toHaveText("Ready");
  await expect(action.frame).toMatchTerminalSnapshot(
    "spans",
    {
      includeCursor: true,
      includeModes: true,
    },
    "ready terminal",
  );
});
```

Importing `@termtrace/test/vitest` registers typed async matchers. Always await them:

- `toHaveScreenText` and `toHaveCursor` for a session, screen, or immutable frame
- `toExist`, `toHaveCount`, `toHaveText`, and `toHaveStyle` for a locator
- `toHaveNoWrappedRows` for a session, screen, or frame
- `toHaveNeutralRegion` for a region locator
- `toHaveNoPresentedTransient` for a session
- `toMatchTerminalSnapshot` for a session, screen, frame, or locator

Only assertion mismatches become matcher failures. Protocol, transport, invalid-query, resource, and timeout errors remain errors, including under `.not`.

### Lifecycle and evidence

Every runnable test attempt owns a fresh Termtrace driver and a fresh launched session. The fixture launches only when the test body starts, so skipped and todo tests launch nothing. Teardown calls the session's passing or failing finish path once and closes the driver once. A cleanup failure after a passing body fails the test. If the body already failed, artifact and cleanup errors are attached as secondary failures without replacing the original `Error`.

Evidence stays under the configured root. The adapter derives bounded path components from the adapter identity, Vitest worker/process, module hash, test hash, retry, and repeat; the canonical session adds its immutable session identity. Raw test titles and file paths are never used directly as directory names.

The default `on-failure` policy discards canonical evidence after successful session finalization, then closes the per-test driver. A driver-close failure still fails the test, but the current public `Session.finish(true)` combines completion and discard, so the adapter cannot defer discard until after driver close. Teardown stops and completes the session before it captures failure artifacts. On failure, the adapter retains and content-verifies the canonical session, requests a driver-verified semantic PNG, and creates a driver-verified debug bundle under per-operation deadlines. Artifact paths are appended to the primary failure diagnostic. If an artifact cannot be generated or verified, canonical evidence is retained and that error is secondary. The public bundle API does not accept a generation-time byte budget, so the adapter can reject an oversized committed bundle but cannot prevent its initial allocation.

Terminal evidence and failure artifacts contain exact application output and may contain secrets. They are not automatically redacted. Store the evidence root accordingly.

Vitest snapshots use Vitest's own snapshot engine. Create, review, reject, and update them with normal Vitest behavior, including `vitest --update`; Termtrace does not create a competing snapshot format or silently rewrite snapshots. The `text`, `spans`, and bounded `cells` profiles exclude session IDs, filesystem paths, timestamps, actor boundaries, dirtiness, and native capture paths while retaining geometry, regions, wrap facts, styles, and requested cursor/mode data.

Vitest exposes its cancellation signal as the ordinary test-context `signal`. Pass it to cancellable Termtrace waits where supported. The current launch method has no `AbortSignal` parameter; the adapter checks cancellation before launch and owns stop/completion/driver cleanup once launch returns.

## Capability summary

| Capability                                     | Runner-neutral TypeScript       | `@termtrace/test/vitest`                     |
| ---------------------------------------------- | ------------------------------- | -------------------------------------------- |
| Rich frames, exact cells, stable/action frames | Yes                             | Same public values                           |
| Pure queries and lazy strict locators          | Yes                             | Async matcher family                         |
| Deterministic text/spans/cells normalization   | Yes                             | Vitest snapshot engine                       |
| Session ownership                              | Explicit `finish`/`close`       | Fresh session and driver per attempt         |
| Passing `on-failure` evidence                  | Explicit `finish(true)`         | Discarded by successful session finalization |
| Failure verification/screenshot/bundle         | Explicit evidence APIs          | Automatic best-effort, secondary on error    |
| Supported runner/platform                      | Runner-neutral Node.js on Linux | Vitest 4.1.3–4.x on Linux                    |

The public session facade is handwritten. Rust-owned JSON Schemas remain the wire authority; generated request/response types and Ajv validators under `src/generated/` are not edited by hand.
