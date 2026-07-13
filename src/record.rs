//! One-shot PTY recording with byte-exact event persistence.

use std::{
    collections::BTreeMap,
    collections::HashMap,
    ffi::OsString,
    fs::{DirBuilder, Permissions},
    io::{BufRead as _, BufReader, Read, Write},
    os::fd::{AsFd as _, OwnedFd},
    os::unix::ffi::OsStrExt as _,
    os::unix::fs::{DirBuilderExt as _, PermissionsExt as _},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

use nix::{
    poll::{PollFd, PollFlags, PollTimeout, poll},
    sys::{
        signal::{Signal, killpg},
        socket::{getsockopt, sockopt::PeerCredentials},
        termios::{SetArg, Termios, cfmakeraw, tcgetattr, tcsetattr},
    },
    unistd::{Pid, isatty, pipe, read as fd_read, write as fd_write},
};
use portable_pty::{Child, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system};
use signal_hook::{consts::signal::SIGWINCH, iterator::Signals};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::write_secure,
    control::{
        CONTROL_PROTOCOL_VERSION, CONTROL_SOCKET_NAME, ControlCommand, ControlData, ControlRequest,
        ControlResponse, ControlResult, MAX_CONTROL_MESSAGE_BYTES, ProcessSignal, validate_frame,
    },
    event::{
        ByteString, Event, EventLogHeader, InputKind, InputSource, LifecycleKind, OperationStatus,
        TerminalColors,
    },
    event_log::{EventLogWriter, SessionClock},
    ghostty_vt::GhosttyVt,
    session::{
        SESSION_MANIFEST_FORMAT, SESSION_MANIFEST_VERSION, SessionCompletion, SessionGeometry,
        SessionManifest,
    },
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
    /// Default colors exposed by the terminal host and pinned into replay.
    pub colors: TerminalColors,
    /// Forward standard input to the child PTY.
    pub forward_stdin: bool,
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
    Input(Vec<u8>),
    InputClosed,
    InputFailed(String),
    Resize(TerminalGeometry),
    ResizeFailed(String),
    Control {
        request: ControlRequest,
        response: mpsc::SyncSender<ControlResponse>,
    },
    ControlServerFailed(String),
}

struct SpawnedCommand {
    master: Box<dyn MasterPty + Send>,
    reader: Box<dyn Read + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    pid: Option<u32>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct TerminalGeometry {
    cols: u16,
    rows: u16,
    pixel_width: u16,
    pixel_height: u16,
}

impl From<PtySize> for TerminalGeometry {
    fn from(size: PtySize) -> Self {
        Self {
            cols: size.cols,
            rows: size.rows,
            pixel_width: size.pixel_width,
            pixel_height: size.pixel_height,
        }
    }
}

impl From<TerminalGeometry> for PtySize {
    fn from(geometry: TerminalGeometry) -> Self {
        Self {
            rows: geometry.rows,
            cols: geometry.cols,
            pixel_width: geometry.pixel_width,
            pixel_height: geometry.pixel_height,
        }
    }
}

struct InputWorker {
    cancel: OwnedFd,
    thread: thread::JoinHandle<()>,
}

struct TerminalModeGuard {
    original: Option<Termios>,
}

struct ResizeWorker {
    handle: signal_hook::iterator::Handle,
    thread: thread::JoinHandle<()>,
}

struct ControlWorker {
    cancel: OwnedFd,
    socket_path: PathBuf,
    thread: thread::JoinHandle<()>,
}

struct RuntimeWorkers {
    reader: thread::JoinHandle<()>,
    input: Option<InputWorker>,
    resize: Option<ResizeWorker>,
    control: ControlWorker,
}

struct RuntimeOutcome {
    status: ExitStatus,
    output_bytes: u64,
}

struct RuntimeProgress {
    output_closed: bool,
    output_bytes: u64,
}

struct LiveTerminal {
    semantic: GhosttyVt,
    query_tracker: QueryTracker,
}

#[derive(Default)]
struct QueryTracker {
    sequence: Vec<u8>,
    kind: QueryKind,
}

#[derive(Clone, Copy, Default)]
enum QueryKind {
    #[default]
    None,
    Escape,
    Csi,
    Osc,
    Single,
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
    let mut terminal_mode = TerminalModeGuard::enter(options.forward_stdin)?;
    let result = record_to(options, &mut stdout.lock());
    finish_with_cleanup(result, terminal_mode.restore())
}

fn record_to(options: &RecordOptions, output: &mut impl Write) -> Result<RecordOutcome> {
    let pty_size = validate_options(options)?;
    create_session_directory(&options.output_directory)?;
    let clock = SessionClock::now()?;
    let session_id = Uuid::now_v7();
    let header = EventLogHeader::new(
        session_id,
        clock.started_unix_ns(),
        options.cols,
        options.rows,
        pty_size.pixel_width,
        pty_size.pixel_height,
        &options.term,
    )
    .with_colors(options.colors.clone());
    let event_log_path = options.output_directory.join("events.jsonl");
    let mut event_log = EventLogWriter::create(&event_log_path, &header, clock)?;
    write_session_manifest(options, pty_size, session_id, header.started_unix_ns)?;

    let mut spawned = spawn_command(options, pty_size)?;
    if let Err(error) = event_log.append(Event::Lifecycle {
        lifecycle_kind: LifecycleKind::Spawned,
        pid: spawned.pid,
    }) {
        terminate_process_group(spawned.pid);
        let _ = spawned.child.kill();
        return Err(error);
    }
    let runtime = match consume_runtime(
        &mut spawned,
        &mut event_log,
        output,
        options.forward_stdin,
        TerminalGeometry::from(pty_size),
        &options.output_directory,
        &options.colors,
    ) {
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
    write_session_completion(
        options,
        &SessionCompletion {
            session_id,
            exit_code,
            signal_name: signal_name.clone(),
            output_bytes: runtime.output_bytes,
            final_event_hash: event_log.current_hash(),
            event_count: event_log.next_seq(),
        },
    )?;

    Ok(RecordOutcome {
        exit_code,
        signal_name,
        output_bytes: runtime.output_bytes,
        event_log: event_log_path,
    })
}

fn write_session_manifest(
    options: &RecordOptions,
    size: PtySize,
    session_id: Uuid,
    started_unix_ns: u128,
) -> Result<()> {
    let mut child_environment = BTreeMap::new();
    child_environment.insert("COLORTERM".to_owned(), "truecolor".to_owned());
    child_environment.insert("TERM".to_owned(), options.term.clone());
    let manifest = SessionManifest {
        format: SESSION_MANIFEST_FORMAT.to_owned(),
        version: SESSION_MANIFEST_VERSION,
        session_id,
        started_unix_ns,
        command: options
            .command
            .iter()
            .map(|value| ByteString::from(value.as_os_str().as_bytes()))
            .collect(),
        working_directory: options
            .working_directory
            .as_deref()
            .map(|path| ByteString::from(path.as_os_str().as_bytes())),
        geometry: SessionGeometry {
            cols: size.cols,
            rows: size.rows,
            pixel_width: size.pixel_width,
            pixel_height: size.pixel_height,
        },
        child_environment,
        termtrace_version: env!("CARGO_PKG_VERSION").to_owned(),
        operating_system: std::env::consts::OS.to_owned(),
        architecture: std::env::consts::ARCH.to_owned(),
        event_log: "events.jsonl".to_owned(),
        control_socket: CONTROL_SOCKET_NAME.to_owned(),
        contains_sensitive_terminal_data: true,
    };
    write_json_secure(&options.output_directory.join("session.json"), &manifest)
}

fn write_session_completion(options: &RecordOptions, completion: &SessionCompletion) -> Result<()> {
    write_json_secure(
        &options.output_directory.join("completion.json"),
        &completion,
    )
}

fn write_json_secure(path: &Path, value: &impl serde::Serialize) -> Result<()> {
    let encoded = serde_json::to_vec_pretty(value).map_err(|source| Error::InvalidJson {
        path: path.to_path_buf(),
        json_path: "$".to_owned(),
        source,
    })?;
    write_secure(path, &encoded)
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
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| terminal_error("take PTY writer", error.to_string()))?;
    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| terminal_error("spawn terminal command", error.to_string()))?;
    drop(pair.slave);
    let pid = child.process_id();
    Ok(SpawnedCommand {
        master: pair.master,
        reader,
        writer,
        child,
        pid,
    })
}

fn consume_runtime(
    spawned: &mut SpawnedCommand,
    event_log: &mut EventLogWriter,
    output: &mut impl Write,
    forward_stdin: bool,
    initial_geometry: TerminalGeometry,
    session_directory: &Path,
    colors: &TerminalColors,
) -> Result<RuntimeOutcome> {
    let mut semantic = GhosttyVt::new(initial_geometry.cols, initial_geometry.rows, 100_000)?;
    semantic.apply_terminal_colors(colors)?;
    semantic.resize(
        initial_geometry.cols,
        initial_geometry.rows,
        geometry_cell_size(initial_geometry.pixel_width, initial_geometry.cols),
        geometry_cell_size(initial_geometry.pixel_height, initial_geometry.rows),
    )?;
    let mut live_terminal = LiveTerminal {
        semantic,
        query_tracker: QueryTracker::default(),
    };
    let (sender, receiver) = mpsc::sync_channel::<RuntimeMessage>(64);
    let reader = std::mem::replace(&mut spawned.reader, Box::new(std::io::empty()));
    let workers = RuntimeWorkers::start(
        reader,
        &sender,
        forward_stdin,
        initial_geometry,
        session_directory,
    )?;
    drop(sender);

    let mut progress = RuntimeProgress {
        output_closed: false,
        output_bytes: 0,
    };
    let mut exit_status = None;
    let mut drain_deadline: Option<Instant> = None;
    let mut process_group_killed = false;
    let mut control_history: HashMap<Uuid, (ControlCommand, ControlResponse)> = HashMap::new();

    let result = (|| {
        while !progress.output_closed || exit_status.is_none() {
            match receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(message) => process_runtime_message(
                    message,
                    spawned,
                    event_log,
                    output,
                    &mut progress,
                    &mut control_history,
                    &mut live_terminal,
                )?,
                Err(RecvTimeoutError::Disconnected) if !progress.output_closed => {
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
                if exit_status.is_some() && !progress.output_closed {
                    drain_deadline = Some(Instant::now() + DESCENDANT_GRACE_PERIOD);
                }
            }
            if drain_deadline.is_some_and(|deadline| Instant::now() >= deadline)
                && !progress.output_closed
            {
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
    let status = workers.finish(result)?;

    Ok(RuntimeOutcome {
        status,
        output_bytes: progress.output_bytes,
    })
}

fn process_runtime_message(
    message: RuntimeMessage,
    spawned: &mut SpawnedCommand,
    event_log: &mut EventLogWriter,
    output: &mut impl Write,
    progress: &mut RuntimeProgress,
    control_history: &mut HashMap<Uuid, (ControlCommand, ControlResponse)>,
    live: &mut LiveTerminal,
) -> Result<()> {
    match message {
        RuntimeMessage::Output(data) => {
            progress.output_bytes = progress
                .output_bytes
                .saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
            event_log.append_buffered(Event::Output {
                data: ByteString::from(data.as_slice()),
            })?;
            output
                .write_all(&data)
                .and_then(|()| output.flush())
                .map_err(|source| Error::Filesystem {
                    path: PathBuf::from("<stdout>"),
                    source,
                })?;
            process_semantic_output(
                &mut live.semantic,
                &mut live.query_tracker,
                spawned.writer.as_mut(),
                event_log,
                &data,
            )?;
        }
        RuntimeMessage::OutputClosed => {
            progress.output_closed = true;
            event_log.append(Event::Lifecycle {
                lifecycle_kind: LifecycleKind::OutputClosed,
                pid: spawned.pid,
            })?;
        }
        RuntimeMessage::ReadFailed(reason) => {
            return Err(terminal_error("read PTY output", reason));
        }
        RuntimeMessage::Input(data) => apply_input(
            spawned.writer.as_mut(),
            event_log,
            Uuid::now_v7(),
            InputSource::Human,
            InputKind::Raw,
            None,
            &data,
        )?,
        RuntimeMessage::InputClosed => {}
        RuntimeMessage::InputFailed(reason) => {
            return Err(terminal_error("read standard input", reason));
        }
        RuntimeMessage::Resize(geometry) => {
            apply_resize(spawned.master.as_mut(), event_log, Uuid::now_v7(), geometry)?;
            resize_semantic(&mut live.semantic, geometry)?;
        }
        RuntimeMessage::ResizeFailed(reason) => {
            return Err(terminal_error("read terminal geometry", reason));
        }
        RuntimeMessage::Control { request, response } => handle_control_request(
            spawned,
            event_log,
            progress.output_bytes,
            request,
            &response,
            control_history,
            &mut live.semantic,
        )?,
        RuntimeMessage::ControlServerFailed(reason) => {
            return Err(terminal_error("serve control socket", reason));
        }
    }
    Ok(())
}

fn spawn_reader(
    mut reader: Box<dyn Read + Send>,
    sender: mpsc::SyncSender<RuntimeMessage>,
) -> Result<thread::JoinHandle<()>> {
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
    Ok(thread)
}

fn spawn_input(sender: mpsc::SyncSender<RuntimeMessage>) -> Result<InputWorker> {
    let (cancel_read, cancel) = pipe()
        .map_err(|error| terminal_error("create input cancellation pipe", error.to_string()))?;
    let thread = thread::Builder::new()
        .name("termtrace-stdin-reader".to_owned())
        .spawn(move || input_loop(&cancel_read, &sender))
        .map_err(|error| terminal_error("spawn standard-input reader", error.to_string()))?;
    Ok(InputWorker { cancel, thread })
}

fn spawn_resize(
    sender: mpsc::SyncSender<RuntimeMessage>,
    initial: TerminalGeometry,
) -> Result<Option<ResizeWorker>> {
    let stdin = std::io::stdin();
    if !isatty(&stdin)
        .map_err(|error| terminal_error("inspect standard input", error.to_string()))?
    {
        return Ok(None);
    }
    let mut signals = Signals::new([SIGWINCH])
        .map_err(|error| terminal_error("listen for terminal resize", error.to_string()))?;
    let handle = signals.handle();
    let thread = thread::Builder::new()
        .name("termtrace-resize-listener".to_owned())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut previous = initial;
            for _ in signals.forever() {
                match rustix::termios::tcgetwinsize(&stdin) {
                    Ok(size) => {
                        let Some(geometry) = geometry_from_winsize(size, initial) else {
                            continue;
                        };
                        if geometry != previous {
                            previous = geometry;
                            if sender.send(RuntimeMessage::Resize(geometry)).is_err() {
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(RuntimeMessage::ResizeFailed(error.to_string()));
                        return;
                    }
                }
            }
        })
        .map_err(|error| terminal_error("spawn terminal-resize listener", error.to_string()))?;
    Ok(Some(ResizeWorker { handle, thread }))
}

fn spawn_control_server(
    session_directory: &Path,
    sender: mpsc::SyncSender<RuntimeMessage>,
) -> Result<ControlWorker> {
    let socket_path = session_directory.join(CONTROL_SOCKET_NAME);
    let listener = UnixListener::bind(&socket_path).map_err(|source| Error::Filesystem {
        path: socket_path.clone(),
        source,
    })?;
    fs_err::set_permissions(&socket_path, Permissions::from_mode(0o600)).map_err(|source| {
        cleanup_failed_control_socket(
            &socket_path,
            Error::Filesystem {
                path: socket_path.clone(),
                source,
            },
        )
    })?;
    listener.set_nonblocking(true).map_err(|source| {
        cleanup_failed_control_socket(
            &socket_path,
            Error::Filesystem {
                path: socket_path.clone(),
                source,
            },
        )
    })?;
    let (cancel_read, cancel) = pipe().map_err(|error| {
        cleanup_failed_control_socket(
            &socket_path,
            terminal_error("create control cancellation pipe", error.to_string()),
        )
    })?;
    let thread_socket_path = socket_path.clone();
    let thread = thread::Builder::new()
        .name("termtrace-control-server".to_owned())
        .spawn(move || {
            if let Err(error) = control_server_loop(&listener, &cancel_read, &sender) {
                let _ = sender.send(RuntimeMessage::ControlServerFailed(error.to_string()));
            }
        })
        .map_err(|error| {
            cleanup_failed_control_socket(
                &socket_path,
                terminal_error("spawn control server", error.to_string()),
            )
        })?;
    tracing::debug!(socket = %thread_socket_path.display(), "control socket ready");
    Ok(ControlWorker {
        cancel,
        socket_path,
        thread,
    })
}

fn cleanup_failed_control_socket(socket_path: &Path, primary: Error) -> Error {
    match fs_err::remove_file(socket_path) {
        Ok(()) => primary,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => primary,
        Err(source) => Error::CleanupFailed {
            primary: Box::new(primary),
            cleanup: Box::new(Error::Filesystem {
                path: socket_path.to_path_buf(),
                source,
            }),
        },
    }
}

fn control_server_loop(
    listener: &UnixListener,
    cancel: &OwnedFd,
    sender: &mpsc::SyncSender<RuntimeMessage>,
) -> Result<()> {
    loop {
        let (listener_ready, cancel_ready) = {
            let mut descriptors = [
                PollFd::new(listener.as_fd(), PollFlags::POLLIN),
                PollFd::new(cancel.as_fd(), PollFlags::POLLIN),
            ];
            match poll(&mut descriptors, PollTimeout::NONE) {
                Ok(_) => (poll_ready(&descriptors[0]), poll_ready(&descriptors[1])),
                Err(nix::errno::Errno::EINTR) => continue,
                Err(error) => return Err(terminal_error("poll control socket", error.to_string())),
            }
        };
        if cancel_ready {
            return Ok(());
        }
        if !listener_ready {
            continue;
        }
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Err(error) = handle_control_connection(stream, sender) {
                        tracing::warn!(error = %error, "control connection rejected");
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => {
                    return Err(terminal_error(
                        "accept control connection",
                        error.to_string(),
                    ));
                }
            }
        }
    }
}

fn handle_control_connection(
    mut stream: UnixStream,
    sender: &mpsc::SyncSender<RuntimeMessage>,
) -> Result<()> {
    let credentials = getsockopt(&stream, PeerCredentials)
        .map_err(|error| terminal_error("authenticate control peer", error.to_string()))?;
    if credentials.uid() != nix::unistd::Uid::effective().as_raw() {
        return Err(Error::ControlRejected {
            code: "unauthorized_peer".to_owned(),
            message: "control peer uid does not own this session".to_owned(),
        });
    }
    let timeout = Some(Duration::from_secs(10));
    stream
        .set_read_timeout(timeout)
        .and_then(|()| stream.set_write_timeout(timeout))
        .map_err(|error| terminal_error("configure control connection", error.to_string()))?;
    let mut encoded = Vec::new();
    BufReader::new(
        stream
            .try_clone()
            .map_err(|error| terminal_error("clone control connection", error.to_string()))?,
    )
    .take(MAX_CONTROL_MESSAGE_BYTES + 1)
    .read_until(b'\n', &mut encoded)
    .map_err(|error| terminal_error("read control request", error.to_string()))?;
    validate_frame(&encoded)?;
    let request: ControlRequest =
        serde_json::from_slice(&encoded).map_err(|source| Error::InvalidJson {
            path: PathBuf::from("<control-socket>"),
            json_path: "$".to_owned(),
            source,
        })?;
    if request.version != CONTROL_PROTOCOL_VERSION {
        return write_control_response(
            &mut stream,
            &ControlResponse {
                version: CONTROL_PROTOCOL_VERSION,
                request_id: request.request_id,
                result: ControlResult::Error {
                    code: "unsupported_version".to_owned(),
                    message: format!("protocol version {} is unsupported", request.version),
                },
            },
        );
    }
    let (response_sender, response_receiver) = mpsc::sync_channel(1);
    sender
        .send(RuntimeMessage::Control {
            request,
            response: response_sender,
        })
        .map_err(|error| terminal_error("dispatch control request", error.to_string()))?;
    let response = response_receiver
        .recv_timeout(Duration::from_secs(10))
        .map_err(|error| terminal_error("wait for control response", error.to_string()))?;
    write_control_response(&mut stream, &response)
}

fn write_control_response(stream: &mut UnixStream, response: &ControlResponse) -> Result<()> {
    serde_json::to_writer(&mut *stream, &response).map_err(|source| Error::InvalidJson {
        path: PathBuf::from("<control-socket>"),
        json_path: "$".to_owned(),
        source,
    })?;
    stream
        .write_all(b"\n")
        .and_then(|()| stream.flush())
        .map_err(|error| terminal_error("write control response", error.to_string()))
}

fn geometry_from_winsize(
    size: rustix::termios::Winsize,
    fallback: TerminalGeometry,
) -> Option<TerminalGeometry> {
    if size.ws_col == 0 || size.ws_row == 0 {
        return None;
    }
    let cell_width = fallback.pixel_width.checked_div(fallback.cols)?.max(1);
    let cell_height = fallback.pixel_height.checked_div(fallback.rows)?.max(1);
    Some(TerminalGeometry {
        cols: size.ws_col,
        rows: size.ws_row,
        pixel_width: if size.ws_xpixel == 0 {
            size.ws_col.saturating_mul(cell_width)
        } else {
            size.ws_xpixel
        },
        pixel_height: if size.ws_ypixel == 0 {
            size.ws_row.saturating_mul(cell_height)
        } else {
            size.ws_ypixel
        },
    })
}

fn input_loop(cancel: &OwnedFd, sender: &mpsc::SyncSender<RuntimeMessage>) {
    let stdin = std::io::stdin();
    let mut buffer = [0_u8; READ_BUFFER_BYTES];
    loop {
        let mut descriptors = [
            PollFd::new(stdin.as_fd(), PollFlags::POLLIN),
            PollFd::new(cancel.as_fd(), PollFlags::POLLIN),
        ];
        match poll(&mut descriptors, PollTimeout::NONE) {
            Ok(_) => {}
            Err(nix::errno::Errno::EINTR) => continue,
            Err(error) => {
                let _ = sender.send(RuntimeMessage::InputFailed(error.to_string()));
                return;
            }
        }
        if poll_ready(&descriptors[1]) {
            return;
        }
        if !poll_ready(&descriptors[0]) {
            continue;
        }
        match fd_read(&stdin, &mut buffer) {
            Ok(0) => {
                let _ = sender.send(RuntimeMessage::InputClosed);
                return;
            }
            Ok(length) => {
                if sender
                    .send(RuntimeMessage::Input(buffer[..length].to_vec()))
                    .is_err()
                {
                    return;
                }
            }
            Err(nix::errno::Errno::EINTR) => {}
            Err(error) => {
                let _ = sender.send(RuntimeMessage::InputFailed(error.to_string()));
                return;
            }
        }
    }
}

fn poll_ready(descriptor: &PollFd<'_>) -> bool {
    descriptor
        .revents()
        .unwrap_or(PollFlags::empty())
        .intersects(
            PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR | PollFlags::POLLNVAL,
        )
}

fn handle_control_request(
    spawned: &mut SpawnedCommand,
    event_log: &mut EventLogWriter,
    output_bytes: u64,
    request: ControlRequest,
    response_sender: &mpsc::SyncSender<ControlResponse>,
    history: &mut HashMap<Uuid, (ControlCommand, ControlResponse)>,
    semantic: &mut GhosttyVt,
) -> Result<()> {
    if let Some((original, response)) = history.get(&request.request_id) {
        let response = if original == &request.command {
            response.clone()
        } else {
            control_error_response(
                request.request_id,
                "request_id_collision",
                "request identifier was already used for a different command",
            )
        };
        let _ = response_sender.send(response);
        return Ok(());
    }

    let outcome = execute_control_command(
        spawned,
        event_log,
        output_bytes,
        request.request_id,
        &request.command,
        semantic,
    );
    let response = match &outcome {
        Ok(data) => ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: request.request_id,
            result: ControlResult::Ok { data: data.clone() },
        },
        Err(error) => control_error_response(
            request.request_id,
            control_error_code(error),
            error.to_string(),
        ),
    };
    history.insert(request.request_id, (request.command, response.clone()));
    if response_sender.send(response).is_err() {
        tracing::warn!(request_id = %request.request_id, "control response receiver disconnected");
    }
    match outcome {
        Ok(_) | Err(Error::ControlRejected { .. }) => Ok(()),
        Err(error) => Err(error),
    }
}

fn execute_control_command(
    spawned: &mut SpawnedCommand,
    event_log: &mut EventLogWriter,
    output_bytes: u64,
    request_id: Uuid,
    command: &ControlCommand,
    semantic: &mut GhosttyVt,
) -> Result<ControlData> {
    match command {
        ControlCommand::Ping => {
            event_log.append(Event::ControlResponse {
                request_id,
                command: "ping".to_owned(),
                response: serde_json::json!({"responsive": true}),
            })?;
            Ok(ControlData::Acknowledged)
        }
        ControlCommand::Status => {
            let observed_seq = event_log.next_seq();
            event_log.append(Event::ControlResponse {
                request_id,
                command: "status".to_owned(),
                response: serde_json::json!({
                    "pid": spawned.pid,
                    "output_bytes": output_bytes,
                    "observed_next_event_seq": observed_seq,
                }),
            })?;
            Ok(ControlData::Status {
                pid: spawned.pid,
                output_bytes,
                next_event_seq: event_log.next_seq(),
            })
        }
        ControlCommand::Input { data, intent } => {
            validate_control_input(data.as_bytes(), intent.as_deref())?;
            apply_input(
                spawned.writer.as_mut(),
                event_log,
                request_id,
                InputSource::Agent,
                InputKind::Raw,
                intent.clone(),
                data.as_bytes(),
            )?;
            Ok(ControlData::Acknowledged)
        }
        ControlCommand::Resize {
            cols,
            rows,
            pixel_width,
            pixel_height,
        } => {
            let geometry = TerminalGeometry {
                cols: *cols,
                rows: *rows,
                pixel_width: *pixel_width,
                pixel_height: *pixel_height,
            };
            validate_control_geometry(geometry)?;
            apply_resize(spawned.master.as_mut(), event_log, request_id, geometry)?;
            resize_semantic(semantic, geometry)?;
            Ok(ControlData::Acknowledged)
        }
        ControlCommand::Signal { signal } => {
            apply_signal(spawned.pid, event_log, request_id, *signal)?;
            Ok(ControlData::Acknowledged)
        }
        ControlCommand::Mark { label } => {
            let label = label.trim();
            if label.is_empty() || label.len() > 256 {
                return Err(control_rejected(
                    "invalid_marker",
                    "marker must contain 1 to 256 UTF-8 bytes",
                ));
            }
            event_log.append(Event::Marker {
                label: label.to_owned(),
            })?;
            Ok(ControlData::Acknowledged)
        }
        ControlCommand::Stop => {
            apply_signal(spawned.pid, event_log, request_id, ProcessSignal::Terminate)?;
            Ok(ControlData::Acknowledged)
        }
    }
}

fn validate_control_input(data: &[u8], intent: Option<&str>) -> Result<()> {
    if data.is_empty() || data.len() > 64 * 1024 {
        return Err(control_rejected(
            "invalid_input",
            "input must contain 1 to 65536 bytes",
        ));
    }
    if intent.is_some_and(|value| value.len() > 1024) {
        return Err(control_rejected(
            "invalid_intent",
            "input intent must not exceed 1024 UTF-8 bytes",
        ));
    }
    Ok(())
}

fn validate_control_geometry(geometry: TerminalGeometry) -> Result<()> {
    if geometry.cols == 0
        || geometry.rows == 0
        || geometry.pixel_width == 0
        || geometry.pixel_height == 0
    {
        return Err(control_rejected(
            "invalid_geometry",
            "columns, rows, pixel width, and pixel height must be non-zero",
        ));
    }
    Ok(())
}

fn apply_signal(
    pid: Option<u32>,
    event_log: &mut EventLogWriter,
    operation_id: Uuid,
    signal: ProcessSignal,
) -> Result<()> {
    let (signal, name) = nix_signal(signal);
    event_log.append(Event::SignalIntent {
        operation_id,
        number: signal as i32,
        name: name.to_owned(),
    })?;
    let signal_result = pid
        .and_then(|value| i32::try_from(value).ok())
        .ok_or(nix::errno::Errno::ESRCH)
        .and_then(|value| killpg(Pid::from_raw(value), signal));
    let status = if signal_result.is_ok() {
        OperationStatus::Applied
    } else {
        OperationStatus::Failed
    };
    let result_write = event_log
        .append(Event::SignalResult {
            operation_id,
            status,
            error_code: signal_result
                .as_ref()
                .err()
                .map(|error| format!("os_errno_{}", *error as i32)),
        })
        .map(|_| ());
    let Some(error) = signal_result.err() else {
        return result_write;
    };
    finish_with_cleanup(
        Err(terminal_error(
            "signal child process group",
            error.to_string(),
        )),
        result_write,
    )
}

fn nix_signal(signal: ProcessSignal) -> (Signal, &'static str) {
    match signal {
        ProcessSignal::Interrupt => (Signal::SIGINT, "SIGINT"),
        ProcessSignal::Terminate => (Signal::SIGTERM, "SIGTERM"),
        ProcessSignal::Hangup => (Signal::SIGHUP, "SIGHUP"),
        ProcessSignal::User1 => (Signal::SIGUSR1, "SIGUSR1"),
        ProcessSignal::User2 => (Signal::SIGUSR2, "SIGUSR2"),
    }
}

fn control_error_response(
    request_id: Uuid,
    code: impl Into<String>,
    message: impl Into<String>,
) -> ControlResponse {
    ControlResponse {
        version: CONTROL_PROTOCOL_VERSION,
        request_id,
        result: ControlResult::Error {
            code: code.into(),
            message: message.into(),
        },
    }
}

fn control_error_code(error: &Error) -> &str {
    match error {
        Error::ControlRejected { code, .. } => code,
        Error::EventLogPoisoned { .. } | Error::InvalidEventLog { .. } => "event_log_failed",
        Error::Filesystem { .. } => "io_failed",
        Error::TerminalOperation { .. } => "terminal_operation_failed",
        _ => "internal_error",
    }
}

fn control_rejected(code: &str, message: &str) -> Error {
    Error::ControlRejected {
        code: code.to_owned(),
        message: message.to_owned(),
    }
}

fn process_semantic_output(
    semantic: &mut GhosttyVt,
    tracker: &mut QueryTracker,
    writer: &mut dyn Write,
    event_log: &mut EventLogWriter,
    data: &[u8],
) -> Result<()> {
    for byte in data {
        let complete = tracker.push(*byte);
        semantic.write(std::slice::from_ref(byte));
        for response in semantic.take_responses() {
            event_log.append(Event::QueryResponse {
                query: ByteString::from(tracker.bytes()),
                response: ByteString::from(response.as_slice()),
                responder: "pinned-libghostty-vt".to_owned(),
            })?;
            apply_input(
                writer,
                event_log,
                Uuid::now_v7(),
                InputSource::Host,
                InputKind::Raw,
                Some("terminal query response".to_owned()),
                &response,
            )?;
        }
        if complete {
            tracker.clear();
        }
    }
    Ok(())
}

impl QueryTracker {
    fn push(&mut self, byte: u8) -> bool {
        const MAX_QUERY_BYTES: usize = 64 * 1024;
        match self.kind {
            QueryKind::None => match byte {
                0x1b => {
                    self.sequence.push(byte);
                    self.kind = QueryKind::Escape;
                    false
                }
                0x9b => {
                    self.sequence.push(byte);
                    self.kind = QueryKind::Csi;
                    false
                }
                0x9d => {
                    self.sequence.push(byte);
                    self.kind = QueryKind::Osc;
                    false
                }
                0x05 => {
                    self.sequence.push(byte);
                    self.kind = QueryKind::Single;
                    true
                }
                _ => false,
            },
            QueryKind::Escape => {
                self.sequence.push(byte);
                match byte {
                    b'[' => {
                        self.kind = QueryKind::Csi;
                        false
                    }
                    b']' => {
                        self.kind = QueryKind::Osc;
                        false
                    }
                    _ => true,
                }
            }
            QueryKind::Csi => {
                self.sequence.push(byte);
                (0x40..=0x7e).contains(&byte) || self.sequence.len() > MAX_QUERY_BYTES
            }
            QueryKind::Osc => {
                self.sequence.push(byte);
                byte == 0x07
                    || self.sequence.ends_with(b"\x1b\\")
                    || self.sequence.len() > MAX_QUERY_BYTES
            }
            QueryKind::Single => true,
        }
    }

    fn bytes(&self) -> &[u8] {
        &self.sequence
    }

    fn clear(&mut self) {
        self.sequence.clear();
        self.kind = QueryKind::None;
    }
}

fn resize_semantic(semantic: &mut GhosttyVt, geometry: TerminalGeometry) -> Result<()> {
    semantic.resize(
        geometry.cols,
        geometry.rows,
        geometry_cell_size(geometry.pixel_width, geometry.cols),
        geometry_cell_size(geometry.pixel_height, geometry.rows),
    )
}

fn geometry_cell_size(pixels: u16, cells: u16) -> u32 {
    u32::from(pixels.checked_div(cells).unwrap_or(0).max(1))
}

fn apply_input(
    writer: &mut dyn Write,
    event_log: &mut EventLogWriter,
    operation_id: Uuid,
    source: InputSource,
    input_kind: InputKind,
    intent: Option<String>,
    data: &[u8],
) -> Result<()> {
    event_log.append(Event::InputIntent {
        operation_id,
        source,
        input_kind,
        data: ByteString::from(data),
        intent,
    })?;
    let (bytes_written, write_error) = write_input(writer, data);
    let status = if write_error.is_some() {
        OperationStatus::Failed
    } else {
        OperationStatus::Applied
    };
    let error_code = write_error.as_ref().map(io_error_code);
    let result_write = event_log
        .append(Event::InputResult {
            operation_id,
            status,
            bytes_written,
            error_code,
        })
        .map(|_| ());
    let Some(error) = write_error else {
        return result_write;
    };
    finish_with_cleanup(
        Err(terminal_error("write PTY input", error.to_string())),
        result_write,
    )
}

fn apply_resize(
    master: &mut dyn MasterPty,
    event_log: &mut EventLogWriter,
    operation_id: Uuid,
    geometry: TerminalGeometry,
) -> Result<()> {
    event_log.append(Event::ResizeIntent {
        operation_id,
        cols: geometry.cols,
        rows: geometry.rows,
        pixel_width: geometry.pixel_width,
        pixel_height: geometry.pixel_height,
    })?;
    let resize_result = master.resize(geometry.into());
    let status = if resize_result.is_ok() {
        OperationStatus::Applied
    } else {
        OperationStatus::Failed
    };
    let result_write = event_log
        .append(Event::ResizeResult {
            operation_id,
            status,
            error_code: resize_result
                .as_ref()
                .err()
                .map(|_| "pty_resize_failed".to_owned()),
        })
        .map(|_| ());
    let Some(error) = resize_result.err() else {
        return result_write;
    };
    finish_with_cleanup(
        Err(terminal_error("resize child PTY", error.to_string())),
        result_write,
    )
}

fn write_input(writer: &mut dyn Write, data: &[u8]) -> (u64, Option<std::io::Error>) {
    let mut offset = 0_usize;
    while offset < data.len() {
        match writer.write(&data[offset..]) {
            Ok(0) => {
                return (
                    u64::try_from(offset).unwrap_or(u64::MAX),
                    Some(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "PTY accepted zero input bytes",
                    )),
                );
            }
            Ok(written) => offset = offset.saturating_add(written),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return (u64::try_from(offset).unwrap_or(u64::MAX), Some(error)),
        }
    }
    (u64::try_from(offset).unwrap_or(u64::MAX), None)
}

fn io_error_code(error: &std::io::Error) -> String {
    error.raw_os_error().map_or_else(
        || format!("io_{:?}", error.kind()).to_ascii_lowercase(),
        |errno| format!("os_errno_{errno}"),
    )
}

impl InputWorker {
    fn stop(self) -> Result<()> {
        let cancel_result = match fd_write(&self.cancel, b"x") {
            Ok(_) | Err(nix::errno::Errno::EPIPE) => Ok(()),
            Err(error) => Err(terminal_error(
                "cancel standard-input reader",
                error.to_string(),
            )),
        };
        let join_result = self
            .thread
            .join()
            .map_err(|_| terminal_error("join standard-input reader", "input thread panicked"));
        finish_with_cleanup(cancel_result, join_result)
    }
}

impl ResizeWorker {
    fn stop(self) -> Result<()> {
        self.handle.close();
        self.thread
            .join()
            .map_err(|_| terminal_error("join terminal-resize listener", "resize thread panicked"))
    }
}

impl ControlWorker {
    fn stop(self) -> Result<()> {
        let cancel_result = match fd_write(&self.cancel, b"x") {
            Ok(_) | Err(nix::errno::Errno::EPIPE) => Ok(()),
            Err(error) => Err(terminal_error("cancel control server", error.to_string())),
        };
        let join_result = self
            .thread
            .join()
            .map_err(|_| terminal_error("join control server", "control thread panicked"));
        let remove_result = match fs_err::remove_file(&self.socket_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(Error::Filesystem {
                path: self.socket_path,
                source,
            }),
        };
        let result = finish_with_cleanup(cancel_result, join_result);
        finish_with_cleanup(result, remove_result)
    }
}

impl RuntimeWorkers {
    fn start(
        reader: Box<dyn Read + Send>,
        sender: &mpsc::SyncSender<RuntimeMessage>,
        forward_stdin: bool,
        initial_geometry: TerminalGeometry,
        session_directory: &Path,
    ) -> Result<Self> {
        let control = spawn_control_server(session_directory, sender.clone())?;
        let resize = match spawn_resize(sender.clone(), initial_geometry) {
            Ok(worker) => worker,
            Err(error) => return finish_with_cleanup(Err(error), control.stop()),
        };
        let input = match forward_stdin
            .then(|| spawn_input(sender.clone()))
            .transpose()
        {
            Ok(worker) => worker,
            Err(error) => {
                let cleanup =
                    finish_with_cleanup(resize.map_or(Ok(()), ResizeWorker::stop), control.stop());
                return finish_with_cleanup(Err(error), cleanup);
            }
        };
        let reader = match spawn_reader(reader, sender.clone()) {
            Ok(worker) => worker,
            Err(error) => {
                let cleanup = finish_with_cleanup(
                    input.map_or(Ok(()), InputWorker::stop),
                    resize.map_or(Ok(()), ResizeWorker::stop),
                );
                let cleanup = finish_with_cleanup(cleanup, control.stop());
                return finish_with_cleanup(Err(error), cleanup);
            }
        };
        Ok(Self {
            reader,
            input,
            resize,
            control,
        })
    }

    fn finish<T>(self, primary: Result<T>) -> Result<T> {
        let control_stop = self.control.stop();
        let input_stop = self.input.map_or(Ok(()), InputWorker::stop);
        let resize_stop = self.resize.map_or(Ok(()), ResizeWorker::stop);
        let reader_join = self
            .reader
            .join()
            .map_err(|_| terminal_error("join PTY reader", "reader thread panicked"));
        let result = finish_with_cleanup(primary, control_stop);
        let result = finish_with_cleanup(result, input_stop);
        let result = finish_with_cleanup(result, resize_stop);
        finish_with_cleanup(result, reader_join)
    }
}

impl TerminalModeGuard {
    fn enter(enabled: bool) -> Result<Self> {
        let stdin = std::io::stdin();
        if !enabled
            || !isatty(&stdin)
                .map_err(|error| terminal_error("inspect standard input", error.to_string()))?
        {
            return Ok(Self { original: None });
        }
        let original = tcgetattr(&stdin)
            .map_err(|error| terminal_error("read terminal mode", error.to_string()))?;
        let mut raw = original.clone();
        cfmakeraw(&mut raw);
        tcsetattr(&stdin, SetArg::TCSANOW, &raw)
            .map_err(|error| terminal_error("enter raw terminal mode", error.to_string()))?;
        Ok(Self {
            original: Some(original),
        })
    }

    fn restore(&mut self) -> Result<()> {
        let Some(original) = self.original.take() else {
            return Ok(());
        };
        tcsetattr(std::io::stdin(), SetArg::TCSANOW, &original)
            .map_err(|error| terminal_error("restore terminal mode", error.to_string()))
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn finish_with_cleanup<T>(primary: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(primary), Ok(())) => Err(primary),
        (Err(primary), Err(cleanup)) => Err(Error::CleanupFailed {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
    }
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
    use crate::{
        event::Event,
        event_log::EventLog,
        session::{SessionCompletion, SessionManifest},
    };

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
            colors: TerminalColors::default(),
            forward_stdin: false,
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
        let manifest: SessionManifest = serde_json::from_slice(
            &fs_err::read(session.join("session.json")).expect("session manifest"),
        )
        .expect("valid session manifest");
        let completion: SessionCompletion = serde_json::from_slice(
            &fs_err::read(session.join("completion.json")).expect("completion manifest"),
        )
        .expect("valid completion manifest");
        assert_eq!(manifest.session_id, recording.header.session_id);
        assert_eq!(completion.session_id, recording.header.session_id);
        assert_eq!(
            completion.final_event_hash,
            recording.events.last().unwrap().hash
        );
        assert_eq!(completion.event_count, recording.events.len() as u64);
        assert_eq!(manifest.command[0].as_bytes(), b"/bin/sh");
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
            colors: TerminalColors::default(),
            forward_stdin: false,
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
            colors: TerminalColors::default(),
            forward_stdin: false,
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

    #[test]
    fn answers_and_audits_terminal_queries() {
        let temporary = TempDir::new().expect("temporary directory");
        let options = RecordOptions {
            output_directory: temporary.path().join("query"),
            command: vec![
                OsString::from("/usr/bin/python3"),
                OsString::from("-c"),
                OsString::from(
                    "import os,tty; tty.setraw(0); os.write(1,b'\\x1b[6n'); r=os.read(0,32); os.write(1,b'GOT:'+r.hex().encode())",
                ),
            ],
            working_directory: None,
            cols: 80,
            rows: 24,
            cell_width_px: 9,
            cell_height_px: 18,
            term: "xterm-ghostty".to_owned(),
            colors: TerminalColors::default(),
            forward_stdin: false,
        };
        let mut output = Vec::new();

        let outcome = record_to(&options, &mut output).expect("record query command");
        let log = EventLog::read(outcome.event_log).expect("valid query log");

        assert!(output.windows(4).any(|window| window == b"GOT:"));
        assert!(log.events.iter().any(|record| matches!(
            &record.event,
            Event::QueryResponse { query, response, .. }
                if query.as_bytes() == b"\x1b[6n" && response.as_bytes() == b"\x1b[1;1R"
        )));
        assert!(log.events.iter().any(|record| matches!(
            record.event,
            Event::InputIntent {
                source: InputSource::Host,
                ..
            }
        )));
    }
}
