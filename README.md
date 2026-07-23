# Termtrace

Termtrace is a Linux terminal flight recorder for debugging and automating terminal applications. It preserves the byte-exact PTY event stream while reconstructing every semantic screen state, so humans and agents can inspect what was visible, how it changed, and which events caused it.

## Features

- **Complete recordings** — run a command interactively or in the background and retain its output, input, resizes, signals, markers, lifecycle, and exit events.
- **Live TUI control** — wait for screen conditions, send text, keys, paste, focus, or pointer input, resize the PTY, and capture an atomic stable screen.
- **Agent-sized defaults** — atomic input sequences, compact live acknowledgements, bounded report pages, and opt-in full-state payloads keep routine automation concise.
- **Semantic replay** — inspect states by time, event, marker, or state ID, including changes that existed for less than a video frame.
- **Rendering diagnostics** — browse a timeline, compare states, connect VT sequences to screen changes, and detect flicker, stale cells, or unnecessary redraws.
- **Repeatable launches** — control geometry, working directory, environment inheritance, color behavior, output limits, durability, and provenance.
- **Screenshots and video** — render deterministic semantic screenshots or capture native Ghostty pixels; export edited video and asciinema casts.
- **Verifiable evidence** — check recording integrity, sign evidence roots, recover the valid prefix of a damaged log, and create sensitivity-labeled handoff bundles.
- **Test automation** — run JSON terminal scenarios or drive sessions through typed Python, TypeScript, and Rust APIs.

## Install

```sh
curl -LsSf https://github.com/zigai/termtrace/raw/main/install.sh | bash
```

## Record and inspect a command

Put the command and its arguments after the recording options:

```sh
termtrace record --out ./app.termtrace --geometry inherit -- my-terminal-app
```

The command runs in a real PTY with output and input forwarded normally. After it exits, inspect the final screen, browse every operation and state transition, or verify the saved evidence:

```sh
termtrace show ./app.termtrace
termtrace analyze timeline ./app.termtrace
termtrace integrity verify ./app.termtrace
```

Recordings are new owner-only directories. Termtrace never replaces an existing output path.

## Drive a live terminal

Start a named session in the background when another process needs to observe and control the application:

```sh
termtrace start app --cols 120 --rows 36 -- my-terminal-app
termtrace wait app 'Ready' --stable-for 250ms
termtrace send app text:'help' enter
termtrace wait-idle app --stable-for 250ms
termtrace show app
termtrace stop app --wait
```

`wait` matches the presented viewport rather than searching output history. `wait-idle` returns the exact semantic state that satisfied the stability condition, avoiding a race between waiting and inspecting.

For a branching or repeated workflow, run the installed typed SDK without adding project files:

```sh
termtrace tooling client status
termtrace tooling client python workflow.py
termtrace tooling client typescript workflow.ts
```

The managed runners use private temporary mappings and shared runtime caches; they leave the caller's working tree untouched.

Advanced workflows are grouped under `control`, `analyze`, `artifact`, `registry`, `integrity`, and `tooling`.

## Investigate and capture a recording

Build the semantic index, inspect an earlier moment, run rendering diagnostics, or create a deterministic screenshot:

```sh
termtrace analyze index --session ./app.termtrace
termtrace show ./app.termtrace --at 5s
termtrace show ./app.termtrace --state-hash HASH
termtrace analyze diagnose ./app.termtrace
termtrace artifact screenshot ./app.termtrace --out ./app-screenshot
```

When actual renderer pixels matter, replay the recording through Ghostty on a private display:

```sh
termtrace artifact capture --session ./app.termtrace
```

`termtrace tooling doctor --explain` reports whether the optional native screenshot and video dependencies are available. Semantic inspection does not require them.

## Automate terminal tests

Termtrace scenarios run in fresh sessions and can send input, wait for screen state, add markers, compare snapshots, and retain failure evidence:

```sh
termtrace test tests/scenarios/login.json --jobs 4 --reruns 1
```

The Python, TypeScript, and Rust clients expose the same screen, keyboard, pointer, event, completion, and evidence operations for programmatic workflows.

## Evidence model

The event log is canonical. Text views, semantic states, screenshots, videos, and exports are derived artifacts that can be regenerated and checked against the original recording.

## Documentation

- [Installation](docs/installation.md)
- [Recording sessions](docs/recording.md)
- [Deterministic test isolation](docs/test-isolation.md)
- [Live control and inspection](docs/agent-workflows.md)
- [Python and TypeScript clients](docs/language-clients.md)
- [Semantic replay](docs/semantic-replay.md)
- [Artifacts and verification](docs/formats.md)
- [Troubleshooting](docs/troubleshooting.md)
- [End-to-end demo](demos/README.md)

For the complete command reference, run `termtrace --help` or `termtrace COMMAND --help`.

## License

[MIT License](LICENSE).
