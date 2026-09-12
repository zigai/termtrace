//! Child-process lifecycle, runtime workers, and session cleanup.

use super::{
    control::{spawn_control_server, ControlWorker},
    environment::EffectiveEnvironment,
    model::{RecordOptions, RuntimeMessage, SpawnedCommand, TerminalGeometry},
    support::{finish_with_cleanup, poll_ready, terminal_error},
};
use crate::{Error, Result};
use nix::{
    errno::Errno,
    poll::{poll, PollFd, PollFlags, PollTimeout},
    sys::signal::{killpg, Signal},
    unistd::{isatty, pipe, read as fd_read, write as fd_write, Pid},
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
    io::{self, ErrorKind, Read},
    os::fd::{AsFd as _, OwnedFd},
    os::unix::fs::{DirBuilderExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Instant,
};
use termtrace_model::protocol::control::ProcessSignal;
use termtrace_terminal::supported_geometry_cell_count;

const SESSION_DIRECTORY_MODE: u32 = 0o700;
const READ_BUFFER_BYTES: usize = 16 * 1024;

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

                        let _ = sender.send(message);

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
                        let _ = sender.send(RuntimeMessage::ResizeFailed(error.to_string()));

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

fn input_loop(cancel: &OwnedFd, sender: &mpsc::SyncSender<RuntimeMessage>) {
    let stdin = io::stdin();
    let terminal = match isatty(&stdin) {
        Ok(terminal) => terminal,
        Err(Errno::EIO) => {
            let _ = sender.send(RuntimeMessage::TerminalInputClosed);

            return;
        }
        Err(error) => {
            let _ = sender.send(RuntimeMessage::InputFailed(error.to_string()));

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
                if terminal {
                    let _ = sender.send(RuntimeMessage::TerminalInputClosed);
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
                let _ = sender.send(RuntimeMessage::TerminalInputClosed);

                return;
            }
            Err(error) => {
                let _ = sender.send(RuntimeMessage::InputFailed(error.to_string()));

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
    input: Option<InputWorker>,
    resize: Option<ResizeWorker>,
    signals: SignalWorker,
    control: ControlWorker,
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

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| return terminal_error("clone PTY reader", error.to_string()))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| return terminal_error("take PTY writer", error.to_string()))?;

    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| return terminal_error("spawn terminal command", error.to_string()))?;

    drop(pair.slave);

    let pid = child.process_id();

    return Ok(SpawnedCommand {
        master: pair.master,
        reader,
        writer,
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

        let join_result = self.thread.join().map_err(|_| {
            return terminal_error("join standard-input reader", "input thread panicked");
        });

        return finish_with_cleanup(cancel_result, join_result);
    }
}

impl ResizeWorker {
    fn stop(self) -> Result<()> {
        self.handle.close();

        return self.thread.join().map_err(|_| {
            return terminal_error("join terminal-resize listener", "resize thread panicked");
        });
    }
}

impl SignalWorker {
    fn stop(self) -> Result<()> {
        self.handle.close();

        return self.thread.join().map_err(|_| {
            return terminal_error("join recorder-signal listener", "signal thread panicked");
        });
    }
}

impl RuntimeWorkers {
    pub(super) fn start(
        reader: Box<dyn Read + Send>,
        sender: &mpsc::SyncSender<RuntimeMessage>,
        receiver: mpsc::Receiver<RuntimeMessage>,
        forward_stdin: bool,
        initial_geometry: TerminalGeometry,
        control_socket: &str,
        host_signals: Signals,
    ) -> Result<(Self, mpsc::Receiver<RuntimeMessage>)> {
        let signals = spawn_host_signals(host_signals, sender.clone())?;
        let control = match spawn_control_server(control_socket, sender.clone()) {
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

        let reader = match spawn_reader(reader, sender.clone()) {
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
        let reader_join = self
            .reader
            .join()
            .map_err(|_| return terminal_error("join PTY reader", "reader thread panicked"));

        let result = finish_with_cleanup(primary, control_stop);
        let result = finish_with_cleanup(result, input_stop);
        let result = finish_with_cleanup(result, resize_stop);
        let result = finish_with_cleanup(result, signals_stop);

        return finish_with_cleanup(result, reader_join);
    }
}

pub(super) fn create_session_directory(path: &Path) -> Result<()> {
    let mut builder = DirBuilder::new();
    builder.mode(SESSION_DIRECTORY_MODE);

    match builder.create(path) {
        Ok(()) => {
            if let Err(source) =
                fs_err::set_permissions(path, Permissions::from_mode(SESSION_DIRECTORY_MODE))
            {
                let _ = fs_err::remove_dir(path);

                return Err(Error::Filesystem {
                    path: path.to_path_buf(),
                    source,
                });
            }

            if let Err(error) = sync_parent_directory(path) {
                let _ = fs_err::remove_dir(path);

                return Err(error);
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

pub(crate) fn terminate_process_group(pid: Option<u32>) {
    let Some(pid) = pid.and_then(|value| return i32::try_from(value).ok()) else {
        return;
    };
    let _ = killpg(Pid::from_raw(pid), Signal::SIGKILL);
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
}
