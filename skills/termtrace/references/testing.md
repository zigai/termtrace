# Repeatable testing

Prefer `termtrace test` scenarios to sleeps or human timing. Every attempt,
including a rerun, must launch a fresh canonical session.

Run the installed example without copying it into the project:

```sh
termtrace test skills/termtrace/examples/ready.termtrace.json \
  --artifacts "${TMPDIR:-/tmp}/termtrace-scenario-artifacts"
```

When the skill is installed outside the repository, resolve the example path
relative to this skill directory. Treat it as a shape to adapt, not a reason to
add generated scaffolding to the application repository.

## Isolation

- Prefer `--clear-env --copy-env PATH --color-policy never` or a typed client
  environment object.
- Pass secret environment specifications through standard input or driver APIs,
  never command-line values.
- Use `--output-byte-budget` to terminate runaway output without truncating
  canonical evidence.
- Use the framework-neutral fixtures under `fixtures/terminal/` for line input,
  alternate-screen behavior, and terminal-query responses.

## Completion criterion

A test run is complete only when every attempt has a distinct canonical path. For
each failure, report the failed step, canonical path, final observation, and
atomic failure directory.
