//! Supervisor side of outer-terminal resizes during an original capture.
//!
//! The recorder inside Ghostty forwards each control resize here. The window is configured only
//! when the resulting window fits the constant capture canvas; the grid change itself is observed
//! on the captured terminal's PTY and recorded beside the decision.

use super::{
    capture_io::unix_now_ns,
    window::{window_size_for_grid, GridMetrics, TerminalWindow},
    Error, Result,
};
use nix::{
    libc,
    sys::socket::{getsockopt, sockopt::PeerCredentials},
    unistd::Uid,
};
use rustix::termios::tcgetwinsize;
use std::{
    fs::OpenOptions,
    io::{self, BufRead as _, BufReader, ErrorKind, Read as _, Write as _},
    os::{
        linux::net::SocketAddrExt as _,
        unix::{
            fs::OpenOptionsExt as _,
            net::{SocketAddr, UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    result::Result as StdResult,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use termtrace_model::{
    artifact::{
        original::{CellGrid, WindowResizeDecision, WindowResizeRecord},
        sdk::DecimalU64,
    },
    protocol::outer_resize::{OuterResizeReply, OuterResizeRequest, MAX_OUTER_RESIZE_LINE_BYTES},
};

const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(20);
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(2);
const OBSERVATION_TIMEOUT: Duration = Duration::from_secs(2);

/// PTY winsize of the captured terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Winsize {
    pub(super) cols: u16,
    pub(super) rows: u16,
    pub(super) pixel_width: u16,
    pub(super) pixel_height: u16,
}

impl Winsize {
    pub(super) const fn grid(self) -> CellGrid {
        return CellGrid {
            cols: self.cols,
            rows: self.rows,
        };
    }
}

/// Read the winsize of the terminal device at `tty` without becoming its controlling process.
pub(super) fn read_winsize(tty: &Path) -> Result<Winsize> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
        .open(tty)
        .map_err(|source| {
            return Error::Filesystem {
                path: tty.to_path_buf(),
                source,
            };
        })?;

    let size = tcgetwinsize(&file).map_err(|error| {
        return Error::TerminalOperation {
            operation: "read captured terminal size",
            reason: error.to_string(),
        };
    })?;

    return Ok(Winsize {
        cols: size.ws_col,
        rows: size.ws_row,
        pixel_width: size.ws_xpixel,
        pixel_height: size.ws_ypixel,
    });
}

/// Window size for a requested grid, or the rejection code when it cannot be applied.
pub(super) fn resize_decision(
    requested: CellGrid,
    current: Winsize,
    window: (u16, u16),
    canvas: (u16, u16),
) -> StdResult<(u32, u32), (&'static str, String)> {
    if current.cols == 0
        || current.rows == 0
        || current.pixel_width == 0
        || current.pixel_height == 0
    {
        return Err((
            "cell_size_unavailable",
            "the captured terminal does not report its pixel size".to_owned(),
        ));
    }

    if requested.cols == 0 || requested.rows == 0 {
        return Err(("invalid_geometry", "grid must be non-zero".to_owned()));
    }

    let size = window_size_for_grid(
        requested.cols,
        requested.rows,
        GridMetrics {
            cell_width: current.pixel_width / current.cols,
            cell_height: current.pixel_height / current.rows,
            text_width: current.pixel_width,
            text_height: current.pixel_height,
        },
        window,
    );

    if size.0 > u32::from(canvas.0) || size.1 > u32::from(canvas.1) {
        return Err((
            "exceeds_native_canvas",
            format!(
                "{}x{} cells need a {}x{} pixel window, larger than the {}x{} capture canvas",
                requested.cols, requested.rows, size.0, size.1, canvas.0, canvas.1
            ),
        ));
    }
    return Ok(size);
}

/// Wait until the captured terminal reports `expected`, or return the last observed size.
pub(super) fn wait_for_grid(
    tty: &Path,
    expected: CellGrid,
    timeout: Duration,
) -> Result<StdResult<Winsize, Winsize>> {
    let deadline = Instant::now() + timeout;

    loop {
        let size = read_winsize(tty)?;
        if size.grid() == expected {
            return Ok(Ok(size));
        }

        if Instant::now() >= deadline {
            return Ok(Err(size));
        }

        thread::sleep(ACCEPT_POLL_INTERVAL);
    }
}

struct Shared {
    window: TerminalWindow,
    tty: PathBuf,
    canvas: (u16, u16),
    finalizing: AtomicBool,
    stop: AtomicBool,
    records: Mutex<Vec<WindowResizeRecord>>,
    errors: Mutex<Vec<String>>,
}

/// Owner-only socket answering recorder resize requests until the capture finalizes.
pub(super) struct ResizeServer {
    shared: Arc<Shared>,
    thread: Option<thread::JoinHandle<()>>,
}

impl ResizeServer {
    pub(super) fn start(
        socket: &str,
        window: TerminalWindow,
        tty: PathBuf,
        canvas: (u16, u16),
    ) -> Result<Self> {
        let socket_error = |error: io::Error| {
            return Error::TerminalOperation {
                operation: "serve outer-terminal resizes",
                reason: error.to_string(),
            };
        };
        let listener = SocketAddr::from_abstract_name(socket.as_bytes())
            .and_then(|address| return UnixListener::bind_addr(&address))
            .map_err(socket_error)?;
        listener.set_nonblocking(true).map_err(socket_error)?;

        let shared = Arc::new(Shared {
            window,
            tty,
            canvas,
            finalizing: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            records: Mutex::new(Vec::new()),
            errors: Mutex::new(Vec::new()),
        });

        let served = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("termtrace-resize-server".to_owned())
            .spawn(move || serve(&listener, &served))
            .map_err(|error| {
                return Error::TerminalOperation {
                    operation: "spawn the resize server",
                    reason: error.to_string(),
                };
            })?;

        return Ok(Self {
            shared,
            thread: Some(thread),
        });
    }

    /// Reject further requests; the program has completed and acquisition is stopping.
    pub(super) fn finalize(&self) {
        self.shared.finalizing.store(true, Ordering::Release);
    }

    /// Stop serving and return every decision in request order, plus failed exchanges.
    pub(super) fn finish(mut self) -> (Vec<WindowResizeRecord>, Vec<String>) {
        self.stop();
        let records = self
            .shared
            .records
            .lock()
            .unwrap_or_else(|error| return error.into_inner())
            .clone();

        let errors = self
            .shared
            .errors
            .lock()
            .unwrap_or_else(|error| return error.into_inner())
            .clone();

        return (records, errors);
    }

    fn stop(&mut self) {
        self.shared.finalizing.store(true, Ordering::Release);

        self.shared.stop.store(true, Ordering::Release);

        if let Some(thread) = self.thread.take() {
            // A panicked server loses only its own records; the capture reports what remains.
            drop(thread.join());
        }
    }
}

impl Drop for ResizeServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn serve(listener: &UnixListener, shared: &Shared) {
    while !shared.stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _address)) => {
                if let Err(error) = answer(stream, shared) {
                    failed(shared, format!("resize request failed: {error}"));
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(error) => {
                failed(
                    shared,
                    format!("resize server stopped accepting requests: {error}"),
                );

                return;
            }
        }
    }
}

fn answer(stream: UnixStream, shared: &Shared) -> io::Result<()> {
    let credentials = getsockopt(&stream, PeerCredentials).map_err(io::Error::from)?;
    if credentials.uid() != Uid::effective().as_raw() {
        return Err(io::Error::new(
            ErrorKind::PermissionDenied,
            "resize peer belongs to another user",
        ));
    }

    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(CONNECTION_TIMEOUT))?;
    stream.set_write_timeout(Some(CONNECTION_TIMEOUT))?;

    let mut line = Vec::new();
    BufReader::new((&stream).take(MAX_OUTER_RESIZE_LINE_BYTES + 1)).read_until(b'\n', &mut line)?;
    let request: OuterResizeRequest = serde_json::from_slice(&line)?;
    let requested = CellGrid {
        cols: request.cols,
        rows: request.rows,
    };
    let decided = if shared.finalizing.load(Ordering::Acquire) {
        Err((
            "capture_finalizing",
            "the captured program has completed".to_owned(),
        ))
    } else {
        match (read_winsize(&shared.tty), shared.window.size()) {
            (Ok(current), Ok(window)) => resize_decision(requested, current, window, shared.canvas)
                .map(|size| return (size, current)),
            (Err(error), _) | (_, Err(error)) => {
                Err(("terminal_size_unavailable", error.to_string()))
            }
        }
    };

    let ((width, height), before) = match decided {
        Ok(decided) => decided,
        Err((code, message)) => {
            record(shared, rejected(request.request_id, requested, code));

            return reply(
                &stream,
                &OuterResizeReply::Rejected {
                    request_id: request.request_id,
                    code: code.to_owned(),
                    message,
                },
            );
        }
    };

    let configured_unix_ns = unix_now_ns()
        .ok()
        .and_then(|ns| return u64::try_from(ns).ok());

    // A failed configure leaves the window state unknown, so the recorder gets no reply.
    shared
        .window
        .configure(width, height)
        .map_err(io::Error::other)?;

    reply(
        &stream,
        &OuterResizeReply::Configured {
            request_id: request.request_id,
            window_width: width,
            window_height: height,
        },
    )?;

    let deadline = Instant::now() + OBSERVATION_TIMEOUT;
    let mut observed = None;

    while Instant::now() < deadline {
        match read_winsize(&shared.tty) {
            Ok(size) if size.grid() == requested => {
                observed = Some(size.grid());

                break;
            }
            Ok(size) if size != before => observed = Some(size.grid()),
            Ok(_) | Err(_) => {}
        }

        thread::sleep(ACCEPT_POLL_INTERVAL);
    }

    record(
        shared,
        WindowResizeRecord {
            request_id: request.request_id,
            requested,
            decision: WindowResizeDecision::Configured,
            code: None,
            window_width: Some(width),
            window_height: Some(height),
            configured_unix_ns: configured_unix_ns.map(DecimalU64),
            observed,
            event_seq: None,
        },
    );

    return Ok(());
}

fn rejected(request_id: uuid::Uuid, requested: CellGrid, code: &str) -> WindowResizeRecord {
    return WindowResizeRecord {
        request_id,
        requested,
        decision: WindowResizeDecision::Rejected,
        code: Some(code.to_owned()),
        window_width: None,
        window_height: None,
        configured_unix_ns: None,
        observed: None,
        event_seq: None,
    };
}

fn failed(shared: &Shared, error: String) {
    shared
        .errors
        .lock()
        .unwrap_or_else(|error| return error.into_inner())
        .push(error);
}

fn record(shared: &Shared, record: WindowResizeRecord) {
    shared
        .records
        .lock()
        .unwrap_or_else(|error| return error.into_inner())
        .push(record);
}

fn reply(mut stream: &UnixStream, reply: &OuterResizeReply) -> io::Result<()> {
    let mut line = serde_json::to_vec(reply)?;
    line.push(b'\n');

    return stream.write_all(&line);
}
