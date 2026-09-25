# Termtrace

Termtrace is a Linux terminal flight recorder for debugging and automating
terminal applications. It preserves canonical PTY input and output while
reconstructing the presentation-eligible screens that applications displayed.
Those are separate truth sources: output history is not proof that text was
presented in the viewport.

## Features

- Exact PTY event recording and semantic screen reconstruction.
- Live screen waits, input, markers, resizing, and process control.
- Semantic replay, timelines, diffs, diagnostics, and review.
- Deterministic semantic images and native Ghostty screenshots/video.
- Repeatable terminal tests, typed clients, and verified evidence bundles.

## Install

```sh
curl -LsSf https://github.com/zigai/termtrace/raw/main/install.sh | bash
```

## Quick start

Record a command, inspect its final presented screen, browse its timeline, and
verify the evidence:

```sh
termtrace record --out ./app.termtrace --geometry inherit -- my-terminal-app
termtrace screen ./app.termtrace
termtrace analyze timeline ./app.termtrace
termtrace analyze verify ./app.termtrace
```

Recordings are owner-only directories and existing output paths are never
replaced.

## Live sessions

```sh
termtrace record -d app --cols 120 --rows 36 --until-screen Ready -- my-terminal-app
termtrace send app text:help enter --until-idle=250ms
termtrace screen app --since STATE
termtrace stop app --wait
```

`record -d NAME --until-*` starts a named background session and synchronizes
its initial screen. `send` applies input and checks its postcondition in one
request; the reported input application remains authoritative even if the
postcondition fails. Pass the state returned by one observation to
`screen --since` to avoid retransmitting an unchanged screen. Use a
[language client](skills/termtrace/references/client-workflows.md) when more
than two or three actions are expected.

## Record a complete tmux session

Record the composite tmux client screen and its individual pane tracks under one
aggregate manifest:

```sh
termtrace tmux record work --new -- tmux new-session -s work
termtrace tmux status work
termtrace tmux stop work --wait
termtrace tmux video work --out work.mp4
```

The composite track retains borders, status lines, layouts, zoom, copy mode, and
window or pane changes for faithful video. Pane tracks retain clean canonical
PTY evidence for inspection. Coverage gaps warn by default; select
`--coverage strict` only when Termtrace is explicitly permitted to terminate an
uncovered pane.

For automatic pane recording outside an aggregate workflow, generate and source
`termtrace tmux config`, or use `termtrace tmux enable` for future panes in an
already-running server. Run `termtrace tmux --help` for scope selection and
aggregate inspection commands.

## Debugging, artifacts, and tests

```sh
termtrace analyze ./app.termtrace
termtrace analyze diagnose ./app.termtrace
termtrace screenshot ./app.termtrace -o ./app-screenshot
termtrace bundle ./app.termtrace -o ./bug-report --profile debug --screenshot
termtrace video ./app.termtrace -o ./app.mp4 --preset docs --renderer semantic
termtrace test SCENARIO.json
```

`termtrace analyze` summarizes the last input action; its subcommands cover
timelines, diffs, state inspection, exact output, and offline verification. Use
`termtrace doctor --explain` to check optional native dependencies.

## Documentation

- [Live control](skills/termtrace/references/live-control.md)
- [Language clients](skills/termtrace/references/client-workflows.md)
- [Artifacts](skills/termtrace/references/artifacts.md)
- [Repeatable testing](skills/termtrace/references/testing.md)

Run `termtrace --help` for the normal workflow and direct command help for exact
options. Avoid recursively traversing deep help or shell-filtering it.

## License

[MIT License](LICENSE)
