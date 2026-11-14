<h1 align="center">Termtrace for Rust</h1>

<p align="center"><strong>Record and drive terminal programs from Rust.</strong></p>

<p align="center">
  <a href="https://github.com/zigai/termtrace/actions/workflows/ci.yml"><img alt="tests" src="https://img.shields.io/github/actions/workflow/status/zigai/termtrace/ci.yml?branch=main&label=tests&labelColor=30363d"></a>
  <a href="https://docs.rs/termtrace-sdk"><img alt="docs.rs" src="https://img.shields.io/docsrs/termtrace-sdk?label=docs.rs&logo=docsdotrs&labelColor=30363d"></a>
  <a href="https://crates.io/crates/termtrace-sdk"><img alt="crates.io" src="https://img.shields.io/crates/v/termtrace-sdk?label=crates.io&color=blue&logo=rust&labelColor=30363d"></a>
  <a href="https://github.com/zigai/termtrace/blob/main/crates/sdk/Cargo.toml"><img alt="Rust" src="https://img.shields.io/badge/Rust-1.97.1+-blue?logo=rust&logoColor=white&labelColor=30363d"></a>
  <a href="https://github.com/zigai/termtrace/blob/main/LICENSE"><img alt="license" src="https://img.shields.io/github/license/zigai/termtrace?label=license&labelColor=30363d"></a>
</p>

`termtrace-sdk` is the Rust SDK for [Termtrace](https://github.com/zigai/termtrace), a flight recorder for terminal programs. Add events to a recording from inside the recorded program, or start a program in a recorded terminal and drive it.

- **Application events:** A recorded program adds its own events and spans to the recording.
- **Session control:** Start a program, wait for text on its screen, and send it keys.
- **Light default build:** The default features talk to a running recording without linking the recorder.

## Installation

```sh
cargo add termtrace-sdk
```

Requires Rust 1.97.1 or newer and the [`termtrace` CLI](https://github.com/zigai/termtrace#installation).

## Examples

### Add events from a recorded program

The event is recorded when the program runs under `termtrace record`.

```rust
use std::collections::BTreeMap;

use termtrace_sdk::{instrumentation::Instrumentation, Client};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(client) = Client::from_environment() {
        Instrumentation::new(client).event("cache warmed", BTreeMap::new())?;
    }

    Ok(())
}
```

### Start and drive a program

Session control needs the `local` feature. That feature links libghostty-vt, which `scripts/bootstrap.sh` in the Termtrace repository builds. Set `TERMTRACE_GHOSTTY_PREFIX` to the built library when you compile.

```sh
cargo add termtrace-sdk --features local
```

```rust
use std::time::Duration;

use termtrace_sdk::{
    input::KeyChord,
    screen::{frame_text, ScreenPredicate},
    ActRequest, FrameDetail, LaunchSpec, ScreenProjectionRequest, StopRequest, Terminal,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let terminal = Terminal::new("termtrace", "recordings");
    let mut session = terminal.start(LaunchSpec::new(["python3"]))?;
    let timeout = Duration::from_secs(5);

    let prompt = ScreenPredicate::Contains {
        text: ">>>".to_owned(),
    };
    session.wait_screen(prompt, Duration::ZERO, timeout)?;

    let enter = KeyChord::parse("enter")?.ok_or("enter is a key")?;
    let answer = ScreenPredicate::Contains {
        text: "42".to_owned(),
    };
    session.act(
        ActRequest::new()
            .text("print(6 * 7)")
            .key(enter)
            .until_screen(answer, timeout),
    )?;

    let frame = session.frame(ScreenProjectionRequest {
        detail: FrameDetail::Text,
        region: None,
    })?;
    println!("{}", frame_text(&frame));

    session.stop(StopRequest::default(), Some(timeout))?;
    Ok(())
}
```

## License

[MIT](https://github.com/zigai/termtrace/blob/main/LICENSE)
