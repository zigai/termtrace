//! Shared native-terminal mode and input-drain lifecycle helpers.

use std::{
    io,
    os::fd::{AsFd as _, OwnedFd},
    sync::mpsc,
    thread,
};

use nix::{
    errno::Errno,
    poll::{PollFd, PollFlags, PollTimeout, poll},
    sys::termios::{SetArg, Termios, cfmakeraw, tcgetattr, tcsetattr},
    unistd::{isatty, pipe, read, write},
};

use crate::{Error, Result};

pub(crate) struct RawTerminalGuard {
    original: Option<Termios>,
}

pub(crate) struct TerminalInputDrain {
    cancel: Option<OwnedFd>,
    thread: Option<thread::JoinHandle<Result<()>>>,
}

impl RawTerminalGuard {
    pub(crate) fn enter(enabled: bool) -> Result<Self> {
        let stdin = io::stdin();
        if !enabled
            || !isatty(&stdin)
                .map_err(|e| return terminal_error("inspect standard input", e.to_string()))?
        {
            return Ok(Self { original: None });
        }
        let original = tcgetattr(&stdin)
            .map_err(|e| return terminal_error("read terminal mode", e.to_string()))?;
        let mut raw = original.clone();
        cfmakeraw(&mut raw);
        tcsetattr(&stdin, SetArg::TCSANOW, &raw)
            .map_err(|e| return terminal_error("enter raw terminal mode", e.to_string()))?;
        return Ok(Self {
            original: Some(original),
        });
    }

    pub(crate) fn restore(&mut self) -> Result<()> {
        let Some(original) = self.original.take() else {
            return Ok(());
        };
        return tcsetattr(io::stdin(), SetArg::TCSANOW, &original)
            .map_err(|e| return terminal_error("restore terminal mode", e.to_string()));
    }

    pub(crate) fn is_active(&self) -> bool {
        return self.original.is_some();
    }
}

impl Drop for RawTerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

impl TerminalInputDrain {
    pub(crate) fn start(enabled: bool) -> Result<Self> {
        if !enabled {
            return Ok(Self {
                cancel: None,
                thread: None,
            });
        }
        let (cancel_read, cancel) = pipe()
            .map_err(|e| return terminal_error("create playback input drain", e.to_string()))?;
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("termtrace-playback-input".to_owned())
            .spawn(move || return drain_terminal_input(cancel_read, ready_sender))
            .map_err(|e| return terminal_error("spawn playback input drain", e.to_string()))?;
        ready_receiver
            .recv()
            .map_err(|e| return terminal_error("start playback input drain", e.to_string()))?;
        return Ok(Self {
            cancel: Some(cancel),
            thread: Some(thread),
        });
    }

    pub(crate) fn stop(&mut self) -> Result<()> {
        let cancel_result = self.cancel.take().map_or(Ok(()), |cancel| {
            return write(&cancel, b"x")
                .map(|_| ())
                .or_else(|e| {
                    if e == Errno::EPIPE {
                        return Ok(());
                    } else {
                        return Err(e);
                    }
                })
                .map_err(|e| return terminal_error("cancel playback input drain", e.to_string()));
        });
        let join_result = self.thread.take().map_or(Ok(()), |thread| {
            return thread.join().map_err(|_| {
                return terminal_error("join playback input drain", "thread panicked");
            })?;
        });
        return combine_cleanup(cancel_result, join_result);
    }
}

impl Drop for TerminalInputDrain {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn drain_terminal_input(cancel: OwnedFd, ready_sender: mpsc::SyncSender<()>) -> Result<()> {
    let stdin = io::stdin();
    ready_sender
        .send(())
        .map_err(|e| return terminal_error("start playback input drain", e.to_string()))?;
    drop(ready_sender);
    let mut buffer = [0_u8; 4096];
    let result = loop {
        let mut descriptors = [
            PollFd::new(stdin.as_fd(), PollFlags::POLLIN),
            PollFd::new(cancel.as_fd(), PollFlags::POLLIN),
        ];
        match poll(&mut descriptors, PollTimeout::NONE) {
            Ok(_) => {}
            Err(Errno::EINTR) => continue,
            Err(e) => return Err(terminal_error("poll playback input", e.to_string())),
        }
        if poll_ready(&descriptors[1]) {
            break Ok(());
        }
        if poll_ready(&descriptors[0]) {
            match read(&stdin, &mut buffer) {
                Ok(0) | Err(Errno::EIO) => break Ok(()),
                Ok(_) | Err(Errno::EINTR) => {}
                Err(e) => break Err(terminal_error("drain playback input", e.to_string())),
            }
        }
    };
    drop(cancel);
    return result;
}

fn poll_ready(descriptor: &PollFd<'_>) -> bool {
    return descriptor.revents().is_some_and(|events| {
        return events.intersects(
            PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR | PollFlags::POLLNVAL,
        );
    });
}

fn combine_cleanup(primary: Result<()>, cleanup: Result<()>) -> Result<()> {
    match (primary, cleanup) {
        (Ok(()), Ok(())) => return Ok(()),
        (Err(primary), Ok(())) => return Err(primary),
        (Ok(()), Err(cleanup)) => return Err(cleanup),
        (Err(primary), Err(cleanup)) => {
            return Err(Error::CleanupFailed {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            });
        }
    }
}

fn terminal_error(operation: &'static str, reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation,
        reason: reason.into(),
    };
}
