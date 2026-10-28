//! Replays a completed session onto this process's terminal through `Native::playback`.

use std::{env, error::Error, path::PathBuf, time::Duration};
use termtrace_sdk::{artifacts::PlaybackOptions, Terminal};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1).map(PathBuf::from);
    let (Some(binary), Some(session), Some(handshake)) =
        (arguments.next(), arguments.next(), arguments.next())
    else {
        return Err("usage: native_playback TERMTRACE_BIN SESSION HANDSHAKE_DIRECTORY".into());
    };
    let attached = Terminal::new(binary, &handshake).attach(&session)?;

    attached.native().playback(&PlaybackOptions {
        session_directory: session,
        timing_file: handshake.join("timing.json"),
        error_file: handshake.join("error.json"),
        geometry_file: handshake.join("geometry.json"),
        ready_file: handshake.join("ready"),
        start_delay: Duration::ZERO,
        final_hold: Duration::ZERO,
        start_monotonic_ns: None,
        end_monotonic_ns: None,
        endpoint_event_seq: None,
        endpoint_event_byte_offset: None,
        instant: false,
    })?;

    return Ok(());
}
