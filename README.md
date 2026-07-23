# Termtrace

Termtrace is a Linux terminal flight recorder for debugging and automating
terminal applications. It preserves exact PTY events and reconstructs the
semantic screens that applications presented.

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

Record a command, inspect its final screen, browse its timeline, and verify the
evidence:

```sh
termtrace record --out ./app.termtrace --geometry inherit -- my-terminal-app
termtrace show ./app.termtrace
termtrace analyze timeline ./app.termtrace
termtrace integrity verify ./app.termtrace
```

Recordings are owner-only directories and existing output paths are never
replaced.

## Live sessions

```sh
termtrace start app --cols 120 --rows 36 -- my-terminal-app
termtrace wait app 'Ready' --stable-for 250ms
termtrace send app text:'help' enter
termtrace wait-idle app --stable-for 250ms
termtrace show app
termtrace stop app --wait
```

Use `termtrace tooling client python workflow.py` or the corresponding
`typescript workflow.ts` form for branching and repeated workflows.

## Artifacts and tests

```sh
termtrace analyze index --session ./app.termtrace
termtrace artifact screenshot ./app.termtrace --out ./app-screenshot
termtrace artifact capture --session ./app.termtrace
termtrace test SCENARIO.json
```

Use `termtrace tooling doctor --explain` to check optional native dependencies.

## Documentation

- [Installation](docs/installation.md)
- [Recording](docs/recording.md)
- [Live workflows](docs/agent-workflows.md)
- [Semantic replay](docs/semantic-replay.md)
- [Artifacts and verification](docs/formats.md)
- [Language clients](docs/language-clients.md)
- [Troubleshooting](docs/troubleshooting.md)

Run `termtrace --help` for the current command tree and options.

## License

[MIT License](LICENSE)
