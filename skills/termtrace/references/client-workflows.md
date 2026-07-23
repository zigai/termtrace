# Managed client workflows

Prefer individual CLI commands for straightforward observations and mutations.
Use `termtrace test` when assertions should be repeatable. A managed client
script is appropriate when one control flow genuinely needs branching, loops,
repeated observations, multiple sessions, or caller-owned retry IDs.

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
