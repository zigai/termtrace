# Managed client workflows

Use individual CLI commands (`termtrace act`, `termtrace screen --json`, etc.) when a workflow will finish in
two or three actions. Beyond that, use the Python SDK (`import termtrace` / `from termtrace import Terminal`)
so repeated observations and actions do not pay process startup and discovery costs. Use `termtrace test`
when assertions should be repeatable. A managed client script is appropriate for branching, loops,
multiple sessions, or caller-owned retry IDs.

## Python SDK for Stateful Workflows

When orchestrating multi-step interactive workflows, import the SDK:

```python
from termtrace import Terminal

terminal = Terminal.start(["my-app", "--arg"])
terminal.wait_screen(contains="Ready", timeout_ms=5000)
terminal.act("text:help", "enter", until_screen="Commands")
screen = terminal.screen()
print(screen.text)
terminal.stop()
```

## Run a script file via managed runners

Keep substantial workflows in reviewable `.py` or `.ts` files:

```sh
termtrace client python workflow.py -- WORKFLOW_ARGUMENTS
termtrace client typescript workflow.ts -- WORKFLOW_ARGUMENTS
```

Prefer the language already used by the project; use Python when neither is a
natural fit. Use `termtrace client status` when checking SDK asset
or runtime availability.

## Use standard input sparingly

Standard input is an escape hatch for a short, disposable, trusted snippet that
does not deserve a file:

```sh
termtrace client python - <<'PY'
from termtrace import Terminal
# Short one-off workflow.
PY
```

Do not place large workflows in heredocs or interpolate untrusted terminal
content into executable source. Move anything worth reviewing, debugging, or
rerunning into a script file.

The managed runners use installed SDK assets and private temporary runtime
mappings. Python uses an isolated uv environment backed by uv's cache;
TypeScript uses Bun with `--no-install`. Neither runner creates dependency
metadata, generated source, virtual environments, or `node_modules` in the
caller's working tree.
