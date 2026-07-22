# Managed client workflows

Use a managed client runner when several operations belong to one control flow.
It keeps one driver process and avoids repeated CLI parsing and large shell
transcripts.

Check discovery without changing the project:

```sh
termtrace client status
```

Run a Python program from standard input:

```sh
termtrace client python - <<'PY'
from pathlib import Path
from termtrace import KeyAtom, Terminal, TextAtom

root = Path.home() / ".local/state/termtrace/agent-evidence"
with Terminal(evidence_root=root) as terminal:
    with terminal.launch(["my-app"], evidence="on-failure") as session:
        session.screen.wait_for_text("Ready")
        session.keyboard.sequence([TextAtom("help"), KeyAtom("enter")])
        frame = session.screen.frame()
        assert "Commands" in frame.viewport_text
PY
```

Run the corresponding TypeScript form:

```sh
termtrace client typescript - <<'TS'
import { Termtrace } from "@termtrace/test";

await using terminal = await Termtrace.create({
  evidenceRoot: `${process.env.HOME}/.local/state/termtrace/agent-evidence`,
});
await using session = await terminal.launch({
  command: ["my-app"],
  evidence: "on-failure",
});
await session.screen.waitForText("Ready");
await session.keyboard.sequence([
  { kind: "text", text: "help" },
  { kind: "key", key: "enter" },
]);
const frame = await session.screen.frame();
if (!frame.viewport_text.includes("Commands")) throw new Error("missing Commands");
TS
```

Passing a `.py` or `.ts` file instead of `-` preserves its arguments after `--`.
Python uses an isolated uv environment backed by uv's cache. TypeScript maps the
installed package through a private temporary symlink tree. Neither runner writes
`package.json`, lock files, virtual environments, generated source, or
`node_modules` into the caller's working tree.
