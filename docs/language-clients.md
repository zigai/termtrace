# Python and TypeScript clients

The Python and TypeScript packages own the subprocess lifecycle and expose
matching `Terminal`, `Session`, `Screen`, `Keyboard`, `Pointer`, semantic,
event, and evidence namespaces. They communicate with the persistent
`termtrace agent` driver through versioned newline-delimited JSON.

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

Rust `termtrace-model::protocol::driver` owns driver request and response wire
contracts. The generated clients translate those responses into exported
host-language domain results; neither client owns a competing response schema.
The separate command-line envelope is owned by
`termtrace-model::protocol::cli_response` and is not a client transport type.

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

Direct `keyboard.press()` and `keyboard.paste()` use the generated key/paste
contracts and do not accept `caller_intent`. To retain intent with those inputs,
use `keyboard.sequence([KeyAtom("enter", caller_intent="submit")])` or a
`PasteAtom` with `caller_intent`; import these atoms from `termtrace`.
`keyboard.type()` continues to accept intent directly.

The Python package keeps immutable rich-screen behavior under
`termtrace/screen`: `frame`, `queries`, `locators`, `snapshot`, and wire
conversion. Session request/action workflows and sync/async session objects live
under `termtrace/session`, with session wire conversion in `session/wire`.
Binary discovery, request transport, protocol translation, and generated
request/response models live under `termtrace/transport`; generated code is
confined to `transport/generated`. Sync and async I/O remain separate. Only
pure request construction, validation, lifecycle, and evidence helpers are
shared.

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

The TypeScript package keeps generated protocol types and validators in
`transport/generated`, process and protocol-boundary code in `transport`,
session workflows in `session`, and rich immutable screen values and queries in
`screen`. `@termtrace/test` remains the curated public facade.
`testing/vitest.ts` is only the stable `@termtrace/test/vitest` entrypoint; its
fixture, lifecycle, matcher, matcher-input, and type implementations live under
`testing/vitest`.

## Testing API parity

Rust is the semantic authority, while Python and TypeScript expose first-class
host-language values rather than generated envelopes. All action and stable
capture APIs return the exact actor-selected rich frame; assertion helpers do
not follow those operations with a second observation.

| Capability | Rust | Python sync / async | TypeScript |
| --- | --- | --- | --- |
| Rich text, styled spans, bounded exact cells | `ScreenFrame` | `ScreenFrame` | `ScreenFrame` |
| Exact stable capture with quality and timing | `Screen::capture_stable` | `screen.capture_stable` | `screen.captureStable` |
| Atomic typed input plus none/idle/screen postcondition | `Session::act` | `session.act` / `await session.act` | `session.act` |
| Guard against an expected presented state | `ActOptions::if_state` | `ActionOptions.if_state` | `ActionOptions.ifState` |
| Literal substring/exact and native regex | `LocatorSpec` / `regex::Regex` | `str` / `re.Pattern` | `string` / `RegExp` |
| Row, region, style, and immutable composition | Yes | Yes | Yes |
| `all`, `count`, strict `single`, explicit `first` / `nth` | Yes | Yes | Yes |
| Exact satisfying-frame locator wait and unchanged-state interval | Yes | Sync and native async | `AbortSignal`-aware async |
| Exists/count/text/style/wrap/neutral runner assertions | Yes | Yes | Yes |
| Source colors versus normalized resolved RGB | Yes | Yes | Yes |

Termtrace locators match terminal cells in one viewport state. They are not
application widgets or accessibility roles. Coordinates are absolute,
zero-based terminal columns. Strings use row-local substring matching by
default; exact mode compares the complete selected text, and crossing linked
soft-wrap rows requires an explicit option. Host-native regex syntax and flags
remain language-specific, but empty/zero-width and partial-grapheme matches are
rejected consistently. `single` rejects both absence and ambiguity.

`fixtures/conformance/locators/locator.json` is consumed by the Rust, Python,
TypeScript suites. It fixes shared ordering, terminal-column ranges, combining
and wide-cell behavior, styled blank coalescing, region intersection,
soft-wrap crossing, state/frame identity, hidden-state labeling, and invalid
query categories.

## Run a persistent workflow without project scaffolding

For more than two or three actions, use one persistent driver or a managed
client instead of repeatedly invoking CLI processes. Release installations
include both client packages as managed assets. Check discovery and runtime
availability with:

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

Generation pins `datamodel-code-generator`, `json-schema-to-typescript`, and Ajv versions. CI regenerates both packages and rejects any diff. Shared valid and invalid responses in `fixtures/conformance/protocol/driver-responses.json` are exercised by both language test suites.

The generated Python request model uses a dictionary representation only for the Rust `NamedKey` union because that union mixes scalar strings with untagged object variants, which `msgspec` cannot decode as one typed union. The Rust driver still performs exact request validation, including the `NamedKey` variants.
