<h1 align="center">Termtrace</h1>

<p align="center">
  <a href="https://github.com/zigai/termtrace/actions/workflows/ci.yml"><img alt="tests" src="https://img.shields.io/github/actions/workflow/status/zigai/termtrace/ci.yml?branch=main&label=tests&labelColor=30363d"></a>
  <a href="https://crates.io/crates/termtrace"><img alt="crates.io" src="https://img.shields.io/crates/v/termtrace?label=crates.io&color=blue&logo=rust&labelColor=30363d"></a>
  <a href="https://pypi.org/project/termtrace/"><img alt="PyPI" src="https://img.shields.io/pypi/v/termtrace?label=PyPI&color=blue&logo=pypi&logoColor=white&labelColor=30363d"></a>
  <a href="https://www.npmjs.com/package/@zigai/termtrace"><img alt="npm" src="https://img.shields.io/npm/v/@zigai/termtrace?label=npm&color=blue&logo=npm&labelColor=30363d"></a>
  <a href="Cargo.toml"><img alt="Rust" src="https://img.shields.io/badge/Rust-1.97.1+-blue?logo=rust&logoColor=white&labelColor=30363d"></a>
  <a href="LICENSE"><img alt="license" src="https://img.shields.io/github/license/zigai/termtrace?label=license&labelColor=30363d"></a>
</p>

**A flight recorder for terminal programs.**

Termtrace records a terminal program while it runs, keeping every key it received
and every screen it drew. Afterwards you can step through the session, compare
screens, take screenshots, or render a video.

- **Debug TUIs:** See what was on screen at any moment and what each key changed.
- **Script terminal programs:** Send keys and wait for text to appear, without `tmux send-keys` and `sleep`.
- **Give agents a terminal:** Coding agents start, type into, and read terminal programs through the MCP server.
- **Test in CI:** Write scenarios as JSON and get a recording of every failure.
- **Share what happened:** Turn one recording into a screenshot, a video, or a bug-report bundle.

## Installation

```sh
curl -LsSf https://github.com/zigai/termtrace/raw/main/install.sh | bash
```

Requires Linux with glibc 2.39 or newer. Run `termtrace doctor` to check the setup.

## Quick start

Record a program, then read its last screen and the timeline of what it did:

```sh
termtrace record --out app.termtrace -- my-app
termtrace screen app.termtrace
termtrace analyze timeline app.termtrace
```

Run a program in the background and drive it from a script:

```sh
termtrace record -d app --until-screen Ready -- my-app
termtrace send app text:hello enter --until-screen Saved
termtrace stop app --wait
```

Export a screenshot, a video, and a bug-report bundle:

```sh
termtrace screenshot app.termtrace -o app-screenshot
termtrace video app.termtrace -o app.mp4
termtrace bundle app.termtrace -o bug-report
```

## Commands

```text
$ termtrace --help
Record native terminal pixels and every semantic terminal state

Usage: termtrace [OPTIONS] <COMMAND>

Commands:
  record      Record a terminal command (use -d/--detach for a background session)
  screen      View the live or historical presented terminal viewport
  send        Send terminal input (keys, text, clicks, resize) and await screen state
  wait        Wait for screen text, output bytes, or idle stability
  status      Show live session status or list registered sessions (--gc to prune)
  stop        Request graceful termination of a running background session
  restart     Atomically stop and restart a background session
  analyze     Inspect session forensics (default: triage last action)
  screenshot  Capture a rendered PNG/SVG screenshot of a session
  video       Export a presentation MP4, GIF, or VP9 `WebM` from a completed session
  bundle      Create an evidence bundle (supports --profile debug and --screenshot)
  test        Execute strict evidence-native terminal scenario tests
  tmux        Record managed tmux panes for their full lifetime
  completion  Generate a shell autocompletion script from the current command graph
  doctor      Diagnose the installation, dependencies, state, and capabilities
  mcp         Serve the session workflow as Model Context Protocol tools and resources over stdio
  help        Print this message or the help of the given subcommand(s)

Options:
      --help       Print help
  -q, --quiet      Suppress standard output and communicate only via exit codes
      --log <LOG>  Diagnostic log level threshold (error, warn, info, debug, trace) [env:
                   TERMTRACE_LOG=] [default: warn]
  -V, --version    Print version
```

## MCP server

Add the server to Claude Code or Codex:

```sh
claude mcp add termtrace -- termtrace mcp
codex mcp add termtrace -- termtrace mcp
```

Other clients run `termtrace mcp` as a stdio server. Sessions an agent starts also appear in the CLI, so `termtrace screen <name>` shows what the agent sees.

## SDKs

Add the SDK to a Rust, Python, TypeScript, or Go project:

```sh
cargo add termtrace-sdk
uv add termtrace
npm install @zigai/termtrace
go get github.com/zigai/termtrace/go
```

The Python and npm packages bundle the `termtrace` binary. The Go module runs the installed CLI, so install it first.

Record a program and drive it from Python:

```python
import termtrace

with termtrace.Termtrace().record(["my-app"]) as rec:
    rec.screen().wait_for_text("Ready", timeout_secs=5)
    rec.actions().send_text("hello").send_key("enter").until_screen("Saved").run()
```

## License

[MIT](LICENSE)
