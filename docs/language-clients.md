# Python and TypeScript clients

The Python and TypeScript packages own the subprocess lifecycle and expose matching `Terminal`, `Session`, `Screen`, `Keyboard`, `Pointer`, semantic, event, and evidence namespaces. They communicate with `termtrace tooling driver` through versioned newline-delimited JSON.

## Contract architecture

Rust is the only wire-contract authority:

```text
Rust serde + Schemars types
          │
          ├── driver-request.schema.json
          └── driver-response.schema.json
                         │
             ┌───────────┴───────────┐
             ▼                       ▼
      generated Python         generated TypeScript
      msgspec models           types + Ajv validators
             │                       │
             └───────────┬───────────┘
                         ▼
              handwritten public façades
```

Every request and response is validated at the process boundary. Protocol envelopes and anonymous tagged-union variants remain internal. Stable domain results such as frames, observations, diffs, completion manifests, verification reports, and artifacts are deliberately exported with concrete types.

Validation diagnostics never contain the raw message because terminal output, typed input, paths, and evidence metadata can be sensitive.

## Python

```python
from termtrace import Terminal

with Terminal(evidence_root=".termtrace-evidence") as terminal:
    with terminal.launch(["my-terminal-app"], evidence="on-failure") as session:
        session.screen.wait_for_text("Ready")
        session.keyboard.type("help")
        session.keyboard.press("enter")
        frame = session.screen.frame()
        assert "Commands" in frame.viewport_text
```

`AsyncTerminal` exposes the same domain results through native asyncio methods. Sync and async transports share the generated models, request encoding, response decoding, correlation checks, and error translation.

## TypeScript

```typescript
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

The package ships precompiled standalone Ajv validators. Ajv is a development dependency and is not initialized at client startup.

## Run a client script without project scaffolding

Release installations include both client packages as managed assets. Check discovery and runtime availability with:

```bash
termtrace tooling client status
```

Run a reviewable script file for branching, retries, loops, or multi-session
workflows:

```bash
termtrace tooling client python workflow.py -- --workflow-argument
termtrace tooling client typescript workflow.ts -- --workflow-argument
```

The `-` source reads a program from standard input, but is intended only for
short disposable snippets. Use `termtrace test` for repeatable assertions and a
normal package dependency for permanent application integrations.

Python uses an isolated uv environment backed by uv's cache, with a Python/PYTHONPATH fallback for already provisioned hosts. TypeScript uses Bun with a private temporary package mapping. Neither path writes dependency metadata, generated source, a virtual environment, or `node_modules` into the current working directory. `TERMTRACE_CLIENT_ASSETS` can point development builds at an explicit asset root.

Driver operation failures expose `retry_safety`, `uncertain`, evidence, and typed control `details`. In particular, partial input sequences retain exact applied-prefix and suffix-resume accounting instead of reducing the failure to a message string.

Both SDKs expose `keyboard.sequence(...)` for one guarded, journaled atom list.
Atom operation IDs are derived deterministically from the sequence request ID,
so rebuilding the same typed sequence with the same request ID produces the
identical payload required by `safe_same_request_id` recovery.

## Regenerate contracts

After changing a Rust driver type, regenerate the checked-in schemas and clients:

```bash
termtrace tooling schemas --out schemas
scripts/generate-client-protocol.sh
bun run --cwd packages/typescript build
```

Schema generation replaces existing files with known generated schema names and
leaves unrelated files in the destination untouched.

Generation pins `datamodel-code-generator`, `json-schema-to-typescript`, and Ajv versions. CI regenerates both packages and rejects any diff. Shared valid and invalid responses in `fixtures/protocol/driver-responses.json` are exercised by both language test suites.

The generated Python request model uses a dictionary representation only for the Rust `NamedKey` union because that union mixes scalar strings with untagged object variants, which `msgspec` cannot decode as one typed union. The Rust driver still performs exact request validation, including the `NamedKey` variants.
