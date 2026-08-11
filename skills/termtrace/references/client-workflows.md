# Managed client workflows

Use individual `--response agent` CLI commands when a workflow will finish in
two or three actions. Beyond that, keep one `termtrace agent` JSONL driver
process so repeated observations and actions do not pay process startup and
discovery costs. Use `termtrace test` when assertions should be repeatable. A
managed client script is appropriate for branching, loops, multiple sessions,
or caller-owned retry IDs.

The `agent` command is the public alias for the persistent driver; `rpc` is an
equivalent compatibility alias. Begin the protocol with `hello`. Driver handles
are process-local, while canonical session IDs and paths remain valid after a
driver restart.

## Run a script file

Keep substantial workflows in reviewable `.py` or `.ts` files:

```sh
termtrace tooling client python workflow.py -- WORKFLOW_ARGUMENTS
termtrace tooling client typescript workflow.ts -- WORKFLOW_ARGUMENTS
```

Prefer the language already used by the project; use Python when neither is a
natural fit. Use `termtrace tooling client status` only when checking SDK asset
or runtime availability.

## Use standard input sparingly

Standard input is an escape hatch for a short, disposable, trusted snippet that
does not deserve a file:

```sh
termtrace tooling client python - <<'PY'
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
