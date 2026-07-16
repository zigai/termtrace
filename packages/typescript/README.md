# Termtrace for TypeScript

A strict Node.js client for the versioned Termtrace driver. It uses `binaryPath`, `TERMTRACE_BINARY`, or a `termtrace` executable on `PATH`.

```ts
import { Termtrace } from "@termtrace/test";

await using terminal = await Termtrace.create({
  evidenceRoot: ".termtrace-evidence",
});
await using session = await terminal.launch({
  command: ["my-terminal-app"],
  evidence: "on-failure",
});

await session.screen.waitForText("Ready");
await session.keyboard.type("help");
await session.keyboard.press("enter");

const frame = await session.screen.frame();
if (!frame.viewport_text.includes("Commands")) throw new Error("commands missing");
```

`ObservationFrame`, `Observation`, `SemanticDiff`, `DiagnoseReport`, `SessionCompletion`, `VerificationReport`, and related results are concrete generated types. Invalid or version-skewed driver messages raise `TermtraceProtocolError` without including the raw payload.

The public session façade is handwritten. Internal request and response types and standalone Ajv validators are generated from the Rust-owned JSON Schemas; do not edit files under `src/generated/`.
