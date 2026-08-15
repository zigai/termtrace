use std::{
    io::{self, Write as _},
    os::fd::{AsFd as _, OwnedFd},
    result::Result as StdResult,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use nix::{
    errno::Errno,
    poll::{PollFd, PollFlags, PollTimeout, poll},
    sys::termios::{SetArg, Termios, cfmakeraw, tcgetattr, tcsetattr},
    unistd::{isatty, pipe, read, write},
};
use termtrace_model::value::geometry::TerminalGeometryReport;

use crate::{Error, Result};

const QUERY_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) struct RawTerminalGuard {
    original: Option<Termios>,
}

pub(super) struct TerminalInputDrain {
    cancel: Option<OwnedFd>,
    thread: Option<thread::JoinHandle<Result<()>>>,
}

impl RawTerminalGuard {
    pub(super) fn enter(enabled: bool) -> Result<Self> {
        let stdin = io::stdin();
        if !enabled
            || !isatty(&stdin)
                .map_err(|error| return terminal_error("inspect standard input", error))?
        {
            return Ok(Self { original: None });
        }
        let original = tcgetattr(&stdin)
            .map_err(|error| return terminal_error("read terminal mode", error))?;
        let mut raw = original.clone();
        cfmakeraw(&mut raw);
        tcsetattr(&stdin, SetArg::TCSANOW, &raw)
            .map_err(|error| return terminal_error("enter raw terminal mode", error))?;
        return Ok(Self {
            original: Some(original),
        });
    }

    pub(super) fn restore(&mut self) -> Result<()> {
        let Some(original) = self.original.take() else {
            return Ok(());
        };
        return tcsetattr(io::stdin(), SetArg::TCSANOW, &original)
            .map_err(|error| return terminal_error("restore terminal mode", error));
    }

    pub(super) fn is_active(&self) -> bool {
        return self.original.is_some();
    }
}

impl Drop for RawTerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

impl TerminalInputDrain {
    pub(super) fn start(enabled: bool) -> Result<Self> {
        if !enabled {
            return Ok(Self {
                cancel: None,
                thread: None,
            });
        }
        let (cancel_read, cancel) =
            pipe().map_err(|error| return terminal_error("create playback input drain", error))?;
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("termtrace-playback-input".to_owned())
            .spawn(move || return drain_input(cancel_read, ready_sender))
            .map_err(|error| return terminal_error("spawn playback input drain", error))?;
        ready_receiver
            .recv()
            .map_err(|error| return terminal_error("start playback input drain", error))?;
        return Ok(Self {
            cancel: Some(cancel),
            thread: Some(thread),
        });
    }

    pub(super) fn stop(&mut self) -> Result<()> {
        let cancel = self.cancel.take().map_or(Ok(()), |fd| {
            return write(&fd, b"x")
                .map(|_| ())
                .or_else(|error| {
                    if error == Errno::EPIPE {
                        return Ok(());
                    } else {
                        return Err(error);
                    }
                })
                .map_err(|error| return terminal_error("cancel playback input drain", error));
        });
        let joined = self.thread.take().map_or(Ok(()), |handle| {
            return handle.join().map_err(|_| {
                return terminal_error("join playback input drain", "thread panicked");
            })?;
        });
        return super::runtime::finish_with_cleanup(cancel, joined);
    }
}

impl Drop for TerminalInputDrain {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub(super) fn resize_and_query(cols: u16, rows: u16) -> Result<TerminalGeometryReport> {
    if cols == 0 || rows == 0 {
        return Err(terminal_error(
            "resize terminal",
            "geometry must be non-zero",
        ));
    }
    let mut output = io::stdout().lock();
    write!(output, "\x1b[8;{rows};{cols}t")
        .and_then(|()| return output.flush())
        .map_err(|error| return terminal_error("request terminal resize", error))?;
    thread::sleep(Duration::from_millis(150));
    return query_terminal();
}

pub(super) fn query_terminal() -> Result<TerminalGeometryReport> {
    let mut output = io::stdout().lock();
    output
        .write_all(b"\x1b[14t\x1b[16t\x1b[18t")
        .and_then(|()| return output.flush())
        .map_err(|error| return terminal_error("query terminal geometry", error))?;
    drop(output);
    let stdin = io::stdin();
    let deadline = Instant::now() + QUERY_TIMEOUT;
    let mut bytes: Vec<u8> = Vec::with_capacity(128);
    let mut buffer = [0_u8; 128];
    loop {
        if let Some(report) = parse_report(&bytes)? {
            return Ok(report);
        }
        if Instant::now() >= deadline {
            return Err(terminal_error(
                "query terminal geometry",
                "timed out waiting for XTWINOPS reports",
            ));
        }
        let mut descriptors = [PollFd::new(stdin.as_fd(), PollFlags::POLLIN)];
        match poll(&mut descriptors, PollTimeout::from(100_u16)) {
            Ok(0) | Err(Errno::EINTR) => continue,
            Ok(_) => {}
            Err(error) => return Err(terminal_error("poll terminal geometry", error)),
        }
        match read(&stdin, &mut buffer) {
            Ok(0) | Err(Errno::EIO) => {
                return Err(terminal_error(
                    "read terminal geometry",
                    "terminal closed before reporting geometry",
                ));
            }
            Ok(length) => bytes.extend_from_slice(buffer.get(..length).ok_or_else(|| {
                return terminal_error("read terminal geometry", "read exceeded input buffer");
            })?),
            Err(Errno::EINTR) => {}
            Err(error) => return Err(terminal_error("read terminal geometry", error)),
        }
        if bytes.len() > 4096 {
            return Err(terminal_error(
                "read terminal geometry",
                "response exceeded 4096 bytes",
            ));
        }
    }
}

fn parse_report(bytes: &[u8]) -> Result<Option<TerminalGeometryReport>> {
    let mut text_size = None;
    let mut cell_size = None;
    let mut grid_size = None;
    for sequence in String::from_utf8_lossy(bytes).split('\x1b').skip(1) {
        let Some(body) = sequence.strip_prefix('[') else {
            continue;
        };
        let Some((parameters, _)) = body.split_once('t') else {
            continue;
        };
        let values = parameters
            .split(';')
            .map(str::parse::<u16>)
            .collect::<StdResult<Vec<_>, _>>()
            .map_err(|error| return terminal_error("parse terminal geometry", error))?;
        let Some((kind, dimensions)) = values.split_first() else {
            continue;
        };
        let (Some(first), Some(second)) = (dimensions.first(), dimensions.get(1)) else {
            continue;
        };
        match *kind {
            4 => text_size = Some((*second, *first)),
            6 => cell_size = Some((*second, *first)),
            8 => grid_size = Some((*second, *first)),
            _ => {}
        }
    }
    let (Some((text_width, text_height)), Some((cell_width, cell_height)), Some((cols, rows))) =
        (text_size, cell_size, grid_size)
    else {
        return Ok(None);
    };
    if cols == 0 || rows == 0 || cell_width == 0 || cell_height == 0 {
        return Err(terminal_error(
            "parse terminal geometry",
            "terminal reported a zero dimension",
        ));
    }
    return Ok(Some(TerminalGeometryReport {
        cols,
        rows,
        cell_width,
        cell_height,
        text_width,
        text_height,
    }));
}

fn drain_input(cancel: OwnedFd, ready: mpsc::SyncSender<()>) -> Result<()> {
    let stdin = io::stdin();
    ready
        .send(())
        .map_err(|error| return terminal_error("start playback input drain", error))?;
    let mut buffer = [0_u8; 4096];
    loop {
        let mut descriptors = [
            PollFd::new(stdin.as_fd(), PollFlags::POLLIN),
            PollFd::new(cancel.as_fd(), PollFlags::POLLIN),
        ];
        match poll(&mut descriptors, PollTimeout::NONE) {
            Ok(_) => {}
            Err(Errno::EINTR) => continue,
            Err(error) => return Err(terminal_error("poll playback input", error)),
        }
        if ready_fd(&descriptors[1]) {
            return Ok(());
        }
        if ready_fd(&descriptors[0]) {
            match read(&stdin, &mut buffer) {
                Ok(0) | Err(Errno::EIO) => return Ok(()),
                Ok(_) | Err(Errno::EINTR) => {}
                Err(error) => return Err(terminal_error("drain playback input", error)),
            }
        }
    }
}

fn ready_fd(fd: &PollFd<'_>) -> bool {
    return fd.revents().is_some_and(|flags| {
        return flags.intersects(
            PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR | PollFlags::POLLNVAL,
        );
    });
}

fn terminal_error(operation: &'static str, reason: impl ToString) -> Error {
    return Error::TerminalOperation {
        operation,
        reason: reason.to_string(),
    };
}

#[cfg(test)]
mod tests {
    use super::{TerminalGeometryReport, parse_report};

    #[test]
    fn parses_xtwinops_geometry_in_any_order() {
        let report = parse_report(b"\x1b[8;40;160t\x1b[4;720;1440t\x1b[6;18;9t")
            .expect("valid report")
            .expect("complete report");
        assert_eq!(
            report,
            TerminalGeometryReport {
                cols: 160,
                rows: 40,
                cell_width: 9,
                cell_height: 18,
                text_width: 1440,
                text_height: 720,
            }
        );
    }

    #[test]
    fn waits_for_all_three_reports() {
        assert_eq!(
            parse_report(b"\x1b[4;720;1440t\x1b[6;18;9t").expect("valid partial report"),
            None
        );
    }
}
