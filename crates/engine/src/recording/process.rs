//! Child-process lifecycle, runtime workers, and session cleanup.

use super::{
    control::{spawn, ControlWorker},
    environment::EffectiveEnvironment,
    model::{RecordOptions, RuntimeMessage, SpawnedCommand, TerminalGeometry},
    support::{finish_with_cleanup, poll_ready, terminal_error, thread_panicked},
};
use crate::{Error, Result};
use nix::{
    errno::Errno,
    poll::{poll, PollFd, PollFlags, PollTimeout},
    sys::signal::Signal,
    unistd::{isatty, pipe, read as fd_read, write as fd_write},
};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use rustix::termios::{tcgetwinsize, Winsize};
use signal_hook::{
    consts::signal::{SIGHUP, SIGINT, SIGTERM, SIGWINCH},
    iterator::{Handle, Signals},
};
use std::{
    env,
    fs::{DirBuilder, File, Permissions},
    io::{self, ErrorKind, Read, Write},
    os::fd::{AsFd as _, OwnedFd},
    os::unix::fs::{DirBuilderExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Instant,
};
use termtrace_model::protocol::control::ProcessSignal;
use termtrace_model::terminal::state::supported_geometry_cell_count;

const SESSION_DIRECTORY_MODE: u32 = 0o700;
const READ_BUFFER_BYTES: usize = 16 * 1024;

fn spawn_reader(
    mut reader: Box<dyn Read + Send>,
    master_raw_fd: Option<i32>,
    cancel_read: OwnedFd,
    sender: mpsc::SyncSender<RuntimeMessage>,
) -> Result<thread::JoinHandle<()>> {
    let thread = thread::Builder::new()
        .name("termtrace-pty-reader".to_owned())
        .spawn(move || {
            let mut buffer = [0_u8; READ_BUFFER_BYTES];

            loop {
                if let Some(raw_fd) = master_raw_fd {
                    #[allow(
                        unsafe_code,
                        clippy::undocumented_unsafe_blocks,
                        reason = "master PTY raw file descriptor remains valid for the recording session"
                    )]
                    let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(raw_fd) };

                    let mut descriptors = [
                        PollFd::new(borrowed, PollFlags::POLLIN),
                        PollFd::new(cancel_read.as_fd(), PollFlags::POLLIN),
                    ];

                    match poll(&mut descriptors, PollTimeout::NONE) {
                        Ok(_) => {}
                        Err(Errno::EINTR) => continue,
                        Err(error) => {
                            notify_runtime(&sender, RuntimeMessage::ReadFailed(error.to_string()));

                            return;
                        }
                    }

                    if descriptors
                        .get(1)
                        .is_some_and(|cancel_fd| return poll_ready(cancel_fd))
                    {
                        return;
                    }

                    if descriptors
                        .first()
                        .is_some_and(|master_fd| return !poll_ready(master_fd))
                    {
                        continue;
                    }
                } else {
                    let mut descriptors = [PollFd::new(cancel_read.as_fd(), PollFlags::POLLIN)];
                    if poll(&mut descriptors, PollTimeout::from(0_u16)).is_ok()
                        && descriptors
                            .first()
                            .is_some_and(|cancel_fd| return poll_ready(cancel_fd))
                    {
                        return;
                    }
                }

                match reader.read(&mut buffer) {
                    Ok(0) => {
                        notify_runtime(&sender, RuntimeMessage::OutputClosed);

                        return;
                    }
                    Ok(length) => {
                        let observed = Instant::now();
                        let Some(slice) = buffer.get(..length) else {
                            continue;
                        };

                        if sender
                            .send(RuntimeMessage::Output {
                                data: slice.to_vec(),
                                observed,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        let message = if error.raw_os_error() == Some(Errno::EIO as i32) {
                            RuntimeMessage::OutputClosed
                        } else {
                            RuntimeMessage::ReadFailed(error.to_string())
                        };

                        notify_runtime(&sender, message);

                        return;
                    }
                }
            }
        })
        .map_err(|error| return terminal_error("spawn PTY reader", error.to_string()))?;

    return Ok(thread);
}

fn spawn_input(sender: mpsc::SyncSender<RuntimeMessage>) -> Result<InputWorker> {
    let (cancel_read, cancel) = pipe().map_err(|error| {
        return terminal_error("create input cancellation pipe", error.to_string());
    })?;

    let thread = thread::Builder::new()
        .name("termtrace-stdin-reader".to_owned())
        .spawn(move || input_loop(&cancel_read, &sender))
        .map_err(|error| return terminal_error("spawn standard-input reader", error.to_string()))?;

    return Ok(InputWorker { cancel, thread });
}

fn spawn_resize(
    sender: mpsc::SyncSender<RuntimeMessage>,
    initial: TerminalGeometry,
) -> Result<Option<ResizeWorker>> {
    let stdin = io::stdin();
    if !isatty(&stdin)
        .map_err(|error| return terminal_error("inspect standard input", error.to_string()))?
    {
        return Ok(None);
    }

    let mut signals = Signals::new([SIGWINCH])
        .map_err(|error| return terminal_error("listen for terminal resize", error.to_string()))?;
    let handle = signals.handle();
    let thread = thread::Builder::new()
        .name("termtrace-resize-listener".to_owned())
        .spawn(move || {
            let stdin = io::stdin();
            let mut previous = initial;

            for _ in signals.forever() {
                match tcgetwinsize(&stdin) {
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
                        notify_runtime(&sender, RuntimeMessage::ResizeFailed(error.to_string()));

                        return;
                    }
                }
            }
        })
        .map_err(|error| {
            return terminal_error("spawn terminal-resize listener", error.to_string());
        })?;

    return Ok(Some(ResizeWorker { handle, thread }));
}

pub(super) fn register_host_signals() -> Result<Signals> {
    return Signals::new([SIGHUP, SIGINT, SIGTERM])
        .map_err(|error| return terminal_error("listen for recorder signals", error.to_string()));
}

fn spawn_host_signals(
    mut signals: Signals,
    sender: mpsc::SyncSender<RuntimeMessage>,
) -> Result<SignalWorker> {
    let handle = signals.handle();
    let thread = thread::Builder::new()
        .name("termtrace-signal-listener".to_owned())
        .spawn(move || {
            for signal in signals.forever() {
                let signal = match signal {
                    SIGHUP => ProcessSignal::Hangup,
                    SIGINT => ProcessSignal::Interrupt,
                    SIGTERM => ProcessSignal::Terminate,
                    _ => continue,
                };
                if sender.send(RuntimeMessage::HostSignal(signal)).is_err() {
                    return;
                }
            }
        })
        .map_err(|error| {
            return terminal_error("spawn recorder-signal listener", error.to_string());
        })?;

    return Ok(SignalWorker { handle, thread });
}

fn geometry_from_winsize(size: Winsize, fallback: TerminalGeometry) -> Option<TerminalGeometry> {
    supported_geometry_cell_count(size.ws_col, size.ws_row)?;

    let cell_width = fallback.pixel_width.checked_div(fallback.cols)?.max(1);
    let cell_height = fallback.pixel_height.checked_div(fallback.rows)?.max(1);

    return Some(TerminalGeometry {
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
    });
}

/// Deliver a worker's final notification to the recording runtime.
///
/// A send fails only once the runtime has dropped its receiver and stopped consuming worker
/// notifications, so the undelivered notification is traced instead.
fn notify_runtime(sender: &mpsc::SyncSender<RuntimeMessage>, message: RuntimeMessage) {
    if sender.send(message).is_err() {
        tracing::debug!("recording runtime stopped before a worker notification was delivered");
    }
}

fn input_loop(cancel: &OwnedFd, sender: &mpsc::SyncSender<RuntimeMessage>) {
    let stdin = io::stdin();
    let terminal = match isatty(&stdin) {
        Ok(terminal) => terminal,
        Err(Errno::EIO) => {
            notify_runtime(sender, RuntimeMessage::TerminalInputClosed);

            return;
        }
        Err(error) => {
            notify_runtime(sender, RuntimeMessage::InputFailed(error.to_string()));

            return;
        }
    };

    let mut buffer = [0_u8; READ_BUFFER_BYTES];

    loop {
        let mut descriptors = [
            PollFd::new(stdin.as_fd(), PollFlags::POLLIN),
            PollFd::new(cancel.as_fd(), PollFlags::POLLIN),
        ];
        match poll(&mut descriptors, PollTimeout::NONE) {
            Ok(_) => {}
            Err(Errno::EINTR) => continue,
            Err(error) => {
                notify_runtime(sender, RuntimeMessage::InputFailed(error.to_string()));

                return;
            }
        }

        if descriptors
            .get(1)
            .is_some_and(|cancel_fd| return poll_ready(cancel_fd))
        {
            return;
        }

        if descriptors
            .first()
            .is_some_and(|stdin_fd| return !poll_ready(stdin_fd))
        {
            continue;
        }

        match fd_read(&stdin, &mut buffer) {
            Ok(0) => {
                if terminal {
                    notify_runtime(sender, RuntimeMessage::TerminalInputClosed);
                }

                return;
            }
            Ok(length) => {
                let Some(slice) = buffer.get(..length) else {
                    continue;
                };
                if sender.send(RuntimeMessage::Input(slice.to_vec())).is_err() {
                    return;
                }
            }
            Err(Errno::EINTR) => {}
            Err(Errno::EIO) if terminal => {
                notify_runtime(sender, RuntimeMessage::TerminalInputClosed);

                return;
            }
            Err(error) => {
                notify_runtime(sender, RuntimeMessage::InputFailed(error.to_string()));

                return;
            }
        }
    }
}

struct InputWorker {
    cancel: OwnedFd,
    thread: thread::JoinHandle<()>,
}

struct ResizeWorker {
    handle: Handle,
    thread: thread::JoinHandle<()>,
}

struct SignalWorker {
    handle: Handle,
    thread: thread::JoinHandle<()>,
}

pub(super) struct RuntimeWorkers {
    reader: thread::JoinHandle<()>,
    reader_cancel: OwnedFd,
    input: Option<InputWorker>,
    resize: Option<ResizeWorker>,
    signals: SignalWorker,
    control: ControlWorker,
}

pub(in crate::recording) struct PtyWriter {
    inner: Box<dyn Write + Send>,
    master_raw_fd: Option<i32>,
}

impl PtyWriter {
    pub(in crate::recording) fn new(
        inner: Box<dyn Write + Send>,
        master_raw_fd: Option<i32>,
    ) -> Self {
        return Self {
            inner,
            master_raw_fd,
        };
    }
}

impl Write for PtyWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(raw_fd) = self.master_raw_fd {
            #[allow(
                unsafe_code,
                clippy::undocumented_unsafe_blocks,
                reason = "master PTY raw file descriptor remains valid for the recording session"
            )]
            let borrowed = unsafe { std::os::fd::BorrowedFd::borrow_raw(raw_fd) };
            let mut descriptors = [PollFd::new(borrowed, PollFlags::POLLOUT)];
            let timeout = PollTimeout::try_from(250).unwrap_or(PollTimeout::NONE);

            match poll(&mut descriptors, timeout) {
                Ok(0) => {
                    return Err(io::Error::new(
                        ErrorKind::TimedOut,
                        "PTY input write poll timed out waiting for writability",
                    ));
                }
                Ok(_) => {
                    if let Some(poll_fd) = descriptors.first() {
                        let revents = poll_fd.revents().unwrap_or(PollFlags::empty());
                        if !revents.contains(PollFlags::POLLOUT)
                            && revents.intersects(PollFlags::POLLERR | PollFlags::POLLNVAL)
                        {
                            return Err(io::Error::new(
                                ErrorKind::BrokenPipe,
                                "PTY input write poll reported error",
                            ));
                        }
                    }
                }
                Err(Errno::EINTR) => {
                    return Err(io::Error::from(ErrorKind::Interrupted));
                }
                Err(err) => {
                    return Err(io::Error::new(
                        ErrorKind::TimedOut,
                        format!("PTY input write poll failed: {err}"),
                    ));
                }
            }
        }

        let to_write = buf.get(..buf.len().min(READ_BUFFER_BYTES)).unwrap_or(buf);

        return self.inner.write(to_write);
    }

    fn flush(&mut self) -> io::Result<()> {
        return self.inner.flush();
    }
}

pub(super) fn spawn_command(
    options: &RecordOptions,
    working_directory: &Path,
    pty_size: PtySize,
    effective_environment: &EffectiveEnvironment,
) -> Result<SpawnedCommand> {
    let pair = native_pty_system()
        .openpty(pty_size)
        .map_err(|error| return terminal_error("open PTY", error.to_string()))?;

    let (program, arguments) = options
        .command
        .split_first()
        .ok_or_else(|| return terminal_error("spawn command", "command must not be empty"))?;
    let mut command = CommandBuilder::new(program);
    command.args(arguments);
    command.env_clear();

    for (name, value) in effective_environment.variables() {
        command.env(name, value);
    }

    command.cwd(working_directory);

    let raw_fd = pair.master.as_raw_fd();
    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| return terminal_error("clone PTY reader", error.to_string()))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| return terminal_error("take PTY writer", error.to_string()))?;

    let pty_writer = PtyWriter::new(writer, raw_fd);

    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| return terminal_error("spawn terminal command", error.to_string()))?;

    drop(pair.slave);

    let pid = child.process_id();

    return Ok(SpawnedCommand {
        master: pair.master,
        reader,
        writer: Box::new(pty_writer),
        child,
        pid,
    });
}

pub(super) fn resolve_working_directory(requested: Option<&Path>) -> Result<PathBuf> {
    let candidate = match requested {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        requested => {
            let current = env::current_dir().map_err(|source| {
                return Error::Filesystem {
                    path: PathBuf::from("."),
                    source,
                };
            })?;

            match requested {
                Some(path) => current.join(path),
                None => current,
            }
        }
    };

    let resolved = fs_err::canonicalize(&candidate).map_err(|source| {
        return Error::Filesystem {
            path: candidate,
            source,
        };
    })?;

    if !resolved.is_dir() {
        return Err(terminal_error(
            "resolve child working directory",
            format!("`{}` is not a directory", resolved.display()),
        ));
    }
    return Ok(resolved);
}

impl InputWorker {
    fn stop(self) -> Result<()> {
        let cancel_result = match fd_write(&self.cancel, b"x") {
            Ok(_) | Err(Errno::EPIPE) => Ok(()),
            Err(error) => Err(terminal_error(
                "cancel standard-input reader",
                error.to_string(),
            )),
        };

        let join_result = self
            .thread
            .join()
            .map_err(|payload| return thread_panicked("join standard-input reader", payload));

        return finish_with_cleanup(cancel_result, join_result);
    }
}

impl ResizeWorker {
    fn stop(self) -> Result<()> {
        self.handle.close();

        return self
            .thread
            .join()
            .map_err(|payload| return thread_panicked("join terminal-resize listener", payload));
    }
}

impl SignalWorker {
    fn stop(self) -> Result<()> {
        self.handle.close();

        return self
            .thread
            .join()
            .map_err(|payload| return thread_panicked("join recorder-signal listener", payload));
    }
}

impl RuntimeWorkers {
    pub(super) fn start(
        reader: Box<dyn Read + Send>,
        master_fd: Option<i32>,
        sender: &mpsc::SyncSender<RuntimeMessage>,
        receiver: mpsc::Receiver<RuntimeMessage>,
        forward_stdin: bool,
        initial_geometry: TerminalGeometry,
        control_socket: &str,
        host_signals: Signals,
    ) -> Result<(Self, mpsc::Receiver<RuntimeMessage>)> {
        let signals = spawn_host_signals(host_signals, sender.clone())?;
        let control = match spawn(control_socket, sender.clone()) {
            Ok(worker) => worker,
            Err(error) => {
                drop(receiver);

                return finish_with_cleanup(Err(error), signals.stop());
            }
        };

        let resize = match spawn_resize(sender.clone(), initial_geometry) {
            Ok(worker) => worker,
            Err(error) => {
                drop(receiver);

                let cleanup = finish_with_cleanup(signals.stop(), control.stop());

                return finish_with_cleanup(Err(error), cleanup);
            }
        };

        let input = match forward_stdin
            .then(|| return spawn_input(sender.clone()))
            .transpose()
        {
            Ok(worker) => worker,
            Err(error) => {
                drop(receiver);

                let cleanup =
                    finish_with_cleanup(resize.map_or(Ok(()), ResizeWorker::stop), control.stop());
                let cleanup = finish_with_cleanup(cleanup, signals.stop());

                return finish_with_cleanup(Err(error), cleanup);
            }
        };

        let (cancel_read, reader_cancel) = pipe().map_err(|error| {
            return terminal_error("create PTY reader cancellation pipe", error.to_string());
        })?;

        let reader = match spawn_reader(reader, master_fd, cancel_read, sender.clone()) {
            Ok(worker) => worker,
            Err(error) => {
                drop(receiver);

                let cleanup = finish_with_cleanup(
                    input.map_or(Ok(()), InputWorker::stop),
                    resize.map_or(Ok(()), ResizeWorker::stop),
                );

                let cleanup = finish_with_cleanup(cleanup, control.stop());
                let cleanup = finish_with_cleanup(cleanup, signals.stop());

                return finish_with_cleanup(Err(error), cleanup);
            }
        };

        return Ok((
            Self {
                reader,
                reader_cancel,
                input,
                resize,
                signals,
                control,
            },
            receiver,
        ));
    }

    pub(super) fn begin_control_shutdown(&self) -> Result<()> {
        return self.control.begin_shutdown();
    }

    pub(super) fn control_shutdown_finished(&self) -> bool {
        return self.control.shutdown_finished();
    }

    pub(super) fn finish<T>(
        self,
        receiver: mpsc::Receiver<RuntimeMessage>,
        primary: Result<T>,
    ) -> Result<T> {
        drop(receiver);

        let control_stop = self.control.stop();
        let input_stop = self.input.map_or(Ok(()), InputWorker::stop);
        let resize_stop = self.resize.map_or(Ok(()), ResizeWorker::stop);
        let signals_stop = self.signals.stop();
        let reader_cancel = match fd_write(&self.reader_cancel, b"x") {
            Ok(_) | Err(Errno::EPIPE) => Ok(()),
            Err(error) => Err(terminal_error("cancel PTY reader", error.to_string())),
        };

        let reader_join = self
            .reader
            .join()
            .map_err(|payload| return thread_panicked("join PTY reader", payload));

        let result = finish_with_cleanup(primary, control_stop);
        let result = finish_with_cleanup(result, input_stop);
        let result = finish_with_cleanup(result, resize_stop);
        let result = finish_with_cleanup(result, signals_stop);
        let result = finish_with_cleanup(result, reader_cancel);

        return finish_with_cleanup(result, reader_join);
    }
}

fn remove_session_directory(path: &Path) -> Result<()> {
    return fs_err::remove_dir(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    });
}

pub(super) fn create_session_directory(path: &Path) -> Result<()> {
    let mut builder = DirBuilder::new();
    builder.mode(SESSION_DIRECTORY_MODE);

    match builder.create(path) {
        Ok(()) => {
            if let Err(source) =
                fs_err::set_permissions(path, Permissions::from_mode(SESSION_DIRECTORY_MODE))
            {
                return finish_with_cleanup(
                    Err(Error::Filesystem {
                        path: path.to_path_buf(),
                        source,
                    }),
                    remove_session_directory(path),
                );
            }

            if let Err(error) = sync_parent_directory(path) {
                return finish_with_cleanup(Err(error), remove_session_directory(path));
            }
            return Ok(());
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            return Err(Error::SessionExists {
                path: path.to_path_buf(),
            });
        }
        Err(source) => {
            return Err(Error::Filesystem {
                path: path.to_path_buf(),
                source,
            });
        }
    }
}

fn sync_parent_directory(path: &Path) -> Result<()> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };

    let directory = File::open(parent).map_err(|source| {
        return Error::Filesystem {
            path: parent.to_path_buf(),
            source,
        };
    })?;

    return directory.sync_all().map_err(|source| {
        return Error::Filesystem {
            path: parent.to_path_buf(),
            source,
        };
    });
}

/// The host signal and its conventional name for a requested process signal.
pub(in crate::recording) const fn host_signal(signal: ProcessSignal) -> (Signal, &'static str) {
    match signal {
        ProcessSignal::Interrupt => return (Signal::SIGINT, "SIGINT"),
        ProcessSignal::Terminate => return (Signal::SIGTERM, "SIGTERM"),
        ProcessSignal::Hangup => return (Signal::SIGHUP, "SIGHUP"),
        ProcessSignal::User1 => return (Signal::SIGUSR1, "SIGUSR1"),
        ProcessSignal::User2 => return (Signal::SIGUSR2, "SIGUSR2"),
    }
}

pub(super) fn invalid_geometry(options: &RecordOptions) -> Error {
    return Error::InvalidGeometry {
        cols: options.cols,
        rows: options.rows,
        cell_width_px: options.cell_width_px,
        cell_height_px: options.cell_height_px,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct NotifyingReader(mpsc::SyncSender<()>);

    impl Read for NotifyingReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.send(()).map_err(io::Error::other)?;

            buf[0] = b'x';

            return Ok(1);
        }
    }

    #[test]
    fn runtime_workers_error_closes_full_inbox_before_joining_reader() {
        let (completed_sender, completed) = mpsc::sync_channel(1);
        let thread = thread::spawn(move || {
            let (sender, receiver) = mpsc::sync_channel(1);
            sender
                .send(RuntimeMessage::Output {
                    data: vec![b'a'],
                    observed: Instant::now(),
                })
                .expect("fill inbox");

            let (reading_sender, reading) = mpsc::sync_channel(1);
            let (workers, receiver) = RuntimeWorkers::start(
                Box::new(NotifyingReader(reading_sender)),
                None,
                &sender,
                receiver,
                false,
                TerminalGeometry {
                    cols: 80,
                    rows: 24,
                    pixel_width: 720,
                    pixel_height: 432,
                },
                &format!("termtrace-worker-test-{}", uuid::Uuid::now_v7()),
                register_host_signals().expect("register host signals"),
            )
            .expect("start real runtime workers");

            reading
                .recv_timeout(Duration::from_secs(5))
                .expect("reader reached full inbox");

            let result: Result<()> = workers.finish(
                receiver,
                Err(terminal_error("test actor failure", "stop consuming")),
            );
            assert!(matches!(
                result,
                Err(Error::TerminalOperation {
                    operation: "test actor failure",
                    ..
                })
            ));
            assert!(sender.send(RuntimeMessage::OutputClosed).is_err());
            completed_sender.send(()).expect("report completed cleanup");
        });

        completed
            .recv_timeout(Duration::from_secs(10))
            .expect("runtime cleanup must release the blocked reader");

        thread.join().expect("runtime cleanup thread");
    }

    #[test]
    fn runtime_workers_partial_startup_failure_disconnects_full_inbox() {
        let (sender, receiver) = mpsc::sync_channel(1);
        sender
            .send(RuntimeMessage::Output {
                data: vec![b'a'],
                observed: Instant::now(),
            })
            .expect("fill inbox");

        let (completed_sender, completed) = mpsc::sync_channel(1);
        let blocked_sender = sender.clone();
        let producer = thread::spawn(move || {
            assert!(blocked_sender.send(RuntimeMessage::OutputClosed).is_err());

            completed_sender
                .send(())
                .expect("report disconnected producer");
        });

        let started = RuntimeWorkers::start(
            Box::new(io::empty()),
            None,
            &sender,
            receiver,
            false,
            TerminalGeometry {
                cols: 80,
                rows: 24,
                pixel_width: 720,
                pixel_height: 432,
            },
            &"x".repeat(256),
            register_host_signals().expect("register host signals"),
        );
        assert!(started.is_err());

        completed
            .recv_timeout(Duration::from_secs(5))
            .expect("partial startup must release the blocked producer");

        producer.join().expect("producer thread");
    }

    #[test]
    fn pty_writer_times_out_when_blocked() {
        use std::os::fd::AsRawFd as _;

        let (read_pipe, write_pipe) = pipe().expect("create pipe");
        let write_fd = write_pipe.as_raw_fd();
        let fill_data = [b'x'; 4096];

        loop {
            let mut descriptors = [PollFd::new(write_pipe.as_fd(), PollFlags::POLLOUT)];

            if let Ok(1) = poll(&mut descriptors, PollTimeout::from(0_u16)) {
                fd_write(&write_pipe, &fill_data).expect("fill pipe");
            } else {
                break;
            }
        }

        let write_file = File::from(write_pipe);
        let mut pty_writer = PtyWriter::new(Box::new(write_file), Some(write_fd));
        let write_result = pty_writer.write(b"blocked data");
        assert!(write_result.is_err());
        assert_eq!(write_result.unwrap_err().kind(), ErrorKind::TimedOut,);

        drop(read_pipe);
    }

    #[test]
    fn pty_reader_cancellation_exits_thread_promptly() {
        let (read_pipe, _write_pipe) = pipe().expect("create pipe");
        let (cancel_read, cancel_write) = pipe().expect("create cancel pipe");
        let (sender, receiver) = mpsc::sync_channel(16);

        let thread = spawn_reader(Box::new(File::from(read_pipe)), None, cancel_read, sender)
            .expect("spawn reader");

        fd_write(&cancel_write, b"x").expect("signal reader cancellation");
        let join_result = thread.join();
        assert!(join_result.is_ok());

        drop(cancel_write);
        drop(receiver);
    }
}
