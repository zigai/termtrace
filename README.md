# Termtrace

A flight recorder for terminal programs. Run something under Termtrace and you get
every byte that went in and out of the terminal, plus every screen the program
drew. Afterwards you can step through it, diff screens, take screenshots, or render
a video.

[![tests](https://img.shields.io/github/actions/workflow/status/zigai/termtrace/ci.yml?branch=main&label=tests&logo=githubactions&logoColor=white)](https://github.com/zigai/termtrace/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/termtrace?label=crates.io&color=blue&logo=rust)](https://crates.io/crates/termtrace)
[![PyPI](https://img.shields.io/pypi/v/termtrace?label=pypi&color=blue&logo=pypi&logoColor=white)](https://pypi.org/project/termtrace/)
[![npm](https://img.shields.io/npm/v/@zigai/termtrace?label=npm&color=blue&logo=npm)](https://www.npmjs.com/package/@zigai/termtrace)
[![license](https://img.shields.io/github/license/zigai/termtrace?label=license&logo=opensourceinitiative&logoColor=white)](LICENSE)

- **Debug TUIs:** See what was on screen at any point, and what your last key changed.
- **Drive programs from scripts:** Send keys and wait for text to appear, instead of `tmux send-keys` and `sleep`.
- **Give agents a terminal:** The MCP server lets coding agents start, type into, and read terminal programs.
- **Share what happened:** Screenshots, MP4 or GIF video, and bug-report bundles from one recording.

## Installation

```sh
curl -LsSf https://github.com/zigai/termtrace/raw/main/install.sh | bash
```

Linux only (x86_64 or aarch64, `glibc` 2.39+). Video needs `ghostty` and `ffmpeg`;
`termtrace doctor` tells you what's missing.

## Quick start

Record a program, use it, quit, and look at what happened:

```sh
termtrace record --out app.termtrace -- my-app
termtrace screen app.termtrace
termtrace analyze timeline app.termtrace
```

Or run it in the background and drive it. `send` returns once `Saved` is on screen:

```sh
termtrace record -d app --until-screen Ready -- my-app
termtrace send app text:hello enter --until-screen Saved
termtrace stop app --wait
```

Make something you can attach to an issue:

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

Claude Code (`.mcp.json` in your project):

```json
{
  "mcpServers": {
    "termtrace": { "command": "termtrace", "args": ["mcp"] }
  }
}
```

Codex (`~/.codex/config.toml`):

```toml
[mcp_servers.termtrace]
command = "termtrace"
args = ["mcp"]
```

Other clients: run `termtrace mcp` as a stdio server. Agent sessions show up in the CLI, so `termtrace screen <name>` shows what the agent sees.

## SDKs

Rust, Python, and TypeScript. The Python and npm packages include the `termtrace` CLI.

```sh
cargo add termtrace-sdk
pip install termtrace
npm install @zigai/termtrace
```

```python
import termtrace

with termtrace.Termtrace().record(["my-app"]) as rec:
    rec.screen().wait_for_text("Ready", timeout_secs=5)
    rec.actions().send_text("hello").send_key("enter").until_screen("Saved").run()
```

## License

[MIT](LICENSE)
