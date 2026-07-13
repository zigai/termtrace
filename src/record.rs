//! One-shot PTY recording with byte-exact event persistence.

use std::{
    ffi::OsString,
    fs::{DirBuilder, Permissions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

use nix::{
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use portable_pty::{Child, CommandBuilder, ExitStatus, PtySize, native_pty_system};
use uuid::Uuid;

use crate::{
    Error, Result,
    event::{ByteString, Event, EventLogHeader, LifecycleKind},
    event_log::{EventLogWriter, SessionClock},
};

const SESSION_DIRECTORY_MODE: u32 = 0o700;
const READ_BUFFER_BYTES: usize = 16 * 1024;
const DESCENDANT_GRACE_PERIOD: Duration = Duration::from_millis(250);
const DESCENDANT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Configuration for one recorded command.
#[derive(Clone, Debug)]
pub struct RecordOptions {
    /// New session directory.
    pub output_directory: PathBuf,
    /// Executable and arguments.
    pub command: Vec<OsString>,
    /// Child working directory.
    pub working_directory: Option<PathBuf>,
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
    /// Cell width reported to the PTY, in pixels.
    pub cell_width_px: u16,
    /// Cell height reported to the PTY, in pixels.
    pub cell_height_px: u16,
    /// Terminal identity exposed to the child.
    pub term: String,
}

/// Completed command information.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordOutcome {
    /// Normal process exit code when available.
    pub exit_code: Option<i32>,
    /// Signal description when signal termination occurred.
    pub signal_name: Option<String>,
    /// Number of output bytes recorded.
    pub output_bytes: u64,
    /// Canonical event-log path.
    pub event_log: PathBuf,
}

enum RuntimeMessage {
    Output(Vec<u8>),
    OutputClosed,
    ReadFailed(String),
}

struct SpawnedCommand {
    reader: Box<dyn Read + Send>,
    child: Box<dyn Child + Send + Sync>,
    pid: Option<u32>,
}

struct RuntimeOutcome {
    status: ExitStatus,
    output_bytes: u64,
}

/// Record a command running in a real pseudo-terminal and mirror its output.
///
/// # Errors
///
/// Returns an error when configuration is invalid, secure artifacts cannot be
/// created, PTY setup or child execution fails, output cannot be mirrored, or
/// the canonical event log cannot be durably committed.
pub fn record(options: &RecordOptions) -> Result<RecordOutcome> {
    let stdout = std::io::stdout();
    record_to(options, &mut stdout.lock())
}

fn record_to(options: &RecordOptions, output: &mut impl Write) -> Result<RecordOutcome> {
    let pty_size = validate_options(options)?;
    create_session_directory(&options.output_directory)?;
    let clock = SessionClock::now()?;
    let header = EventLogHeader::new(
        Uuid::now_v7(),
        clock.started_unix_ns(),
        options.cols,
        options.rows,
        &options.term,
    );
    let event_log_path = options.output_directory.join("events.jsonl");
    let mut event_log = EventLogWriter::create(&event_log_path, &header, clock)?;

    let mut spawned = spawn_command(options, pty_size)?;
    if let Err(error) = event_log.append(Event::Lifecycle {
        lifecycle_kind: LifecycleKind::Spawned,
        pid: spawned.pid,
    }) {
        terminate_process_group(spawned.pid);
        let _ = spawned.child.kill();
        return Err(error);
    }
    let runtime = match consume_runtime(&mut spawned, &mut event_log, output) {
        Ok(runtime) => runtime,
        Err(error) => {
            terminate_process_group(spawned.pid);
            let _ = spawned.child.kill();
            return Err(error);
        }
    };
    let exit_code = i32::try_from(runtime.status.exit_code()).ok();
    let signal_name = runtime.status.signal().map(str::to_owned);
    event_log.append(Event::Exit {
        code: exit_code,
        signal: None,
        signal_name: signal_name.clone(),
    })?;

    Ok(RecordOutcome {
        exit_code,
        signal_name,
        output_bytes: runtime.output_bytes,
        event_log: event_log_path,
    })
}

fn validate_options(options: &RecordOptions) -> Result<PtySize> {
    if options.command.is_empty() {
        return Err(terminal_error("validate command", "command is empty"));
    }
    let pixel_width = options
        .cols
        .checked_mul(options.cell_width_px)
        .ok_or_else(|| invalid_geometry(options))?;
    let pixel_height = options
        .rows
        .checked_mul(options.cell_height_px)
        .ok_or_else(|| invalid_geometry(options))?;
    if options.cols == 0
        || options.rows == 0
        || options.cell_width_px == 0
        || options.cell_height_px == 0
    {
        return Err(invalid_geometry(options));
    }
    Ok(PtySize {
        rows: options.rows,
        cols: options.cols,
        pixel_width,
        pixel_height,
    })
}

fn spawn_command(options: &RecordOptions, pty_size: PtySize) -> Result<SpawnedCommand> {
    let pair = native_pty_system()
        .openpty(pty_size)
        .map_err(|error| terminal_error("open PTY", error.to_string()))?;
    let mut command = CommandBuilder::new(&options.command[0]);
    command.args(&options.command[1..]);
    command.env("TERM", &options.term);
    command.env("COLORTERM", "truecolor");
    if let Some(directory) = options.working_directory.as_deref() {
        command.cwd(directory);
    }

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| terminal_error("clone PTY reader", error.to_string()))?;
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| terminal_error("spawn terminal command", error.to_string()))?;
    drop(pair.slave);
    let pid = child.process_id();
    Ok(SpawnedCommand { reader, child, pid })
}

fn consume_runtime(
    spawned: &mut SpawnedCommand,
    event_log: &mut EventLogWriter,
    output: &mut impl Write,
) -> Result<RuntimeOutcome> {
    let reader = std::mem::replace(&mut spawned.reader, Box::new(std::io::empty()));
    let (receiver, reader_thread) = spawn_reader(reader)?;

    let mut output_closed = false;
    let mut exit_status = None;
    let mut output_bytes = 0_u64;
    let mut drain_deadline: Option<Instant> = None;
    let mut process_group_killed = false;

    let result = (|| {
        while !output_closed || exit_status.is_none() {
            match receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(RuntimeMessage::Output(data)) => {
                    output_bytes =
                        output_bytes.saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
                    event_log.append(Event::Output {
                        data: ByteString::from(data.as_slice()),
                    })?;
                    output
                        .write_all(&data)
                        .and_then(|()| output.flush())
                        .map_err(|source| Error::Filesystem {
                            path: PathBuf::from("<stdout>"),
                            source,
                        })?;
                }
                Ok(RuntimeMessage::OutputClosed) => {
                    output_closed = true;
                    event_log.append(Event::Lifecycle {
                        lifecycle_kind: LifecycleKind::OutputClosed,
                        pid: spawned.pid,
                    })?;
                }
                Ok(RuntimeMessage::ReadFailed(reason)) => {
                    return Err(terminal_error("read PTY output", reason));
                }
                Err(RecvTimeoutError::Disconnected) if !output_closed => {
                    return Err(terminal_error(
                        "receive runtime event",
                        "PTY reader disconnected before reporting closure",
                    ));
                }
                Err(RecvTimeoutError::Disconnected | RecvTimeoutError::Timeout) => {}
            }

            if exit_status.is_none() {
                exit_status = spawned
                    .child
                    .try_wait()
                    .map_err(|error| terminal_error("poll child", error.to_string()))?;
                if exit_status.is_some() && !output_closed {
                    drain_deadline = Some(Instant::now() + DESCENDANT_GRACE_PERIOD);
                }
            }
            if drain_deadline.is_some_and(|deadline| Instant::now() >= deadline) && !output_closed {
                if process_group_killed {
                    return Err(terminal_error(
                        "drain PTY output",
                        "PTY remained open after terminating retained process-group descendants",
                    ));
                }
                terminate_process_group(spawned.pid);
                process_group_killed = true;
                drain_deadline = Some(Instant::now() + DESCENDANT_DRAIN_TIMEOUT);
            }
        }
        exit_status.ok_or_else(|| terminal_error("wait for child", "missing exit status"))
    })();

    if result.is_err() {
        terminate_process_group(spawned.pid);
        let _ = spawned.child.kill();
    }
    let join_result = reader_thread
        .join()
        .map_err(|_| terminal_error("join PTY reader", "reader thread panicked"));
    let status = result?;
    join_result?;

    Ok(RuntimeOutcome {
        status,
        output_bytes,
    })
}

fn spawn_reader(
    mut reader: Box<dyn Read + Send>,
) -> Result<(mpsc::Receiver<RuntimeMessage>, thread::JoinHandle<()>)> {
    let (sender, receiver) = mpsc::sync_channel::<RuntimeMessage>(64);
    let thread = thread::Builder::new()
        .name("termtrace-pty-reader".to_owned())
        .spawn(move || {
            let mut buffer = [0_u8; READ_BUFFER_BYTES];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        let _ = sender.send(RuntimeMessage::OutputClosed);
                        return;
                    }
                    Ok(length) => {
                        if sender
                            .send(RuntimeMessage::Output(buffer[..length].to_vec()))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        let message = if error.raw_os_error() == Some(nix::errno::Errno::EIO as i32)
                        {
                            RuntimeMessage::OutputClosed
                        } else {
                            RuntimeMessage::ReadFailed(error.to_string())
                        };
                        let _ = sender.send(message);
                        return;
                    }
                }
            }
        })
        .map_err(|error| terminal_error("spawn PTY reader", error.to_string()))?;
    Ok((receiver, thread))
}

fn create_session_directory(path: &Path) -> Result<()> {
    let mut builder = DirBuilder::new();
    builder.mode(SESSION_DIRECTORY_MODE);
    match builder.create(path) {
        Ok(()) => {
            fs_err::set_permissions(path, Permissions::from_mode(SESSION_DIRECTORY_MODE)).map_err(
                |source| Error::Filesystem {
                    path: path.to_path_buf(),
                    source,
                },
            )?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(Error::SessionExists {
                path: path.to_path_buf(),
            })
        }
        Err(source) => Err(Error::Filesystem {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn terminate_process_group(pid: Option<u32>) {
    let Some(pid) = pid.and_then(|value| i32::try_from(value).ok()) else {
        return;
    };
    let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
}

fn invalid_geometry(options: &RecordOptions) -> Error {
    Error::InvalidGeometry {
        cols: options.cols,
        rows: options.rows,
        cell_width_px: options.cell_width_px,
        cell_height_px: options.cell_height_px,
    }
}

fn terminal_error(operation: &'static str, reason: impl Into<String>) -> Error {
    Error::TerminalOperation {
        operation,
        reason: reason.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::{os::unix::fs::PermissionsExt as _, time::Instant};

    use tempfile::TempDir;

    use super::*;
    use crate::{event::Event, event_log::EventLog};

    #[test]
    fn records_exact_output_and_exit_after_output_close() {
        let temporary = TempDir::new().expect("temporary directory");
        let session = temporary.path().join("recording");
        let options = RecordOptions {
            output_directory: session.clone(),
            command: vec![
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from(r"printf '\033[31mhello\033[0m\377'"),
            ],
            working_directory: None,
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
            term: "xterm-ghostty".to_owned(),
        };

        let mut mirrored_output = Vec::new();
        let outcome = record_to(&options, &mut mirrored_output).expect("record command");
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(
            fs_err::metadata(&session)
                .expect("session metadata")
                .permissions()
                .mode()
                & 0o777,
            SESSION_DIRECTORY_MODE
        );

        let recording = EventLog::read(&outcome.event_log).expect("valid event log");
        let output = recording
            .events
            .iter()
            .filter_map(|record| match &record.event {
                Event::Output { data } => Some(data.as_bytes()),
                _ => None,
            })
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        assert_eq!(output, b"\x1b[31mhello\x1b[0m\xff");
        assert_eq!(mirrored_output, output);
        assert!(matches!(
            recording.events.last().map(|record| &record.event),
            Some(Event::Exit { code: Some(0), .. })
        ));
        let output_closed_index = recording
            .events
            .iter()
            .position(|record| {
                matches!(
                    record.event,
                    Event::Lifecycle {
                        lifecycle_kind: LifecycleKind::OutputClosed,
                        ..
                    }
                )
            })
            .expect("output closed event");
        assert!(output_closed_index < recording.events.len() - 1);
    }

    #[test]
    fn refuses_to_overwrite_existing_session_directory() {
        let temporary = TempDir::new().expect("temporary directory");
        let options = RecordOptions {
            output_directory: temporary.path().to_path_buf(),
            command: vec![OsString::from("true")],
            working_directory: None,
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
            term: "xterm-ghostty".to_owned(),
        };

        let error = record(&options).expect_err("existing directory must fail");
        assert!(matches!(error, Error::SessionExists { .. }));
    }

    #[test]
    fn terminates_descendants_that_retain_the_slave_pty() {
        let temporary = TempDir::new().expect("temporary directory");
        let options = RecordOptions {
            output_directory: temporary.path().join("recording"),
            command: vec![
                OsString::from("/bin/sh"),
                OsString::from("-c"),
                OsString::from("(trap '' HUP; exec sleep 30) & printf done"),
            ],
            working_directory: None,
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
            term: "xterm-ghostty".to_owned(),
        };
        let started = Instant::now();
        let mut output = Vec::new();

        let outcome = record_to(&options, &mut output).expect("record command");

        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(output, b"done");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "retained PTY was not terminated promptly"
        );
    }
}
