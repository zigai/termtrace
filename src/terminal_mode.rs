//! Shared native-terminal mode and input-drain lifecycle helpers.

use std::{os::fd::AsFd as _, sync::mpsc, thread};

use nix::{
    poll::{PollFd, PollFlags, PollTimeout, poll},
    sys::termios::{SetArg, Termios, cfmakeraw, tcgetattr, tcsetattr},
    unistd::{isatty, pipe, read, write},
};

use crate::{Error, Result};

pub(crate) struct RawTerminalGuard {
    original: Option<Termios>,
}

pub(crate) struct TerminalInputDrain {
    cancel: Option<std::os::fd::OwnedFd>,
    thread: Option<thread::JoinHandle<Result<()>>>,
}

impl RawTerminalGuard {
    pub(crate) fn enter(enabled: bool) -> Result<Self> {
        let stdin = std::io::stdin();
        if !enabled
            || !isatty(&stdin)
                .map_err(|e| terminal_error("inspect standard input", e.to_string()))?
        {
            return Ok(Self { original: None });
        }
        let original =
            tcgetattr(&stdin).map_err(|e| terminal_error("read terminal mode", e.to_string()))?;
        let mut raw = original.clone();
        cfmakeraw(&mut raw);
        tcsetattr(&stdin, SetArg::TCSANOW, &raw)
            .map_err(|e| terminal_error("enter raw terminal mode", e.to_string()))?;
        Ok(Self {
            original: Some(original),
        })
    }

    pub(crate) fn restore(&mut self) -> Result<()> {
        let Some(original) = self.original.take() else {
            return Ok(());
        };
        tcsetattr(std::io::stdin(), SetArg::TCSANOW, &original)
            .map_err(|e| terminal_error("restore terminal mode", e.to_string()))
    }

    pub(crate) fn is_active(&self) -> bool {
        self.original.is_some()
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
        let (cancel_read, cancel) =
            pipe().map_err(|e| terminal_error("create playback input drain", e.to_string()))?;
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("termtrace-playback-input".to_owned())
            .spawn(move || drain_terminal_input(cancel_read, ready_sender))
            .map_err(|e| terminal_error("spawn playback input drain", e.to_string()))?;
        ready_receiver
            .recv()
            .map_err(|e| terminal_error("start playback input drain", e.to_string()))?;
        Ok(Self {
            cancel: Some(cancel),
            thread: Some(thread),
        })
    }

    pub(crate) fn stop(&mut self) -> Result<()> {
        let cancel_result = self.cancel.take().map_or(Ok(()), |cancel| {
            write(&cancel, b"x")
                .map(|_| ())
                .or_else(|e| {
                    if e == nix::errno::Errno::EPIPE {
                        Ok(())
                    } else {
                        Err(e)
                    }
                })
                .map_err(|e| terminal_error("cancel playback input drain", e.to_string()))
        });
        let join_result = self.thread.take().map_or(Ok(()), |thread| {
            thread
                .join()
                .map_err(|_| terminal_error("join playback input drain", "thread panicked"))?
        });
        combine_cleanup(cancel_result, join_result)
    }
}

impl Drop for TerminalInputDrain {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn drain_terminal_input(
    cancel: std::os::fd::OwnedFd,
    ready_sender: mpsc::SyncSender<()>,
) -> Result<()> {
    let stdin = std::io::stdin();
    ready_sender
        .send(())
        .map_err(|e| terminal_error("start playback input drain", e.to_string()))?;
    drop(ready_sender);
    let mut buffer = [0_u8; 4096];
    let result = loop {
        let mut descriptors = [
            PollFd::new(stdin.as_fd(), PollFlags::POLLIN),
            PollFd::new(cancel.as_fd(), PollFlags::POLLIN),
        ];
        match poll(&mut descriptors, PollTimeout::NONE) {
            Ok(_) => {}
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => return Err(terminal_error("poll playback input", e.to_string())),
        }
        if poll_ready(&descriptors[1]) {
            break Ok(());
        }
        if poll_ready(&descriptors[0]) {
            match read(&stdin, &mut buffer) {
                Ok(0) | Err(nix::errno::Errno::EIO) => break Ok(()),
                Ok(_) | Err(nix::errno::Errno::EINTR) => {}
                Err(e) => break Err(terminal_error("drain playback input", e.to_string())),
            }
        }
    };
    drop(cancel);
    result
}

fn poll_ready(descriptor: &PollFd<'_>) -> bool {
    descriptor.revents().is_some_and(|events| {
        events.intersects(
            PollFlags::POLLIN | PollFlags::POLLHUP | PollFlags::POLLERR | PollFlags::POLLNVAL,
        )
    })
}

fn combine_cleanup(primary: Result<()>, cleanup: Result<()>) -> Result<()> {
    match (primary, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Err(primary), Err(cleanup)) => Err(Error::CleanupFailed {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
    }
}

fn terminal_error(operation: &'static str, reason: impl Into<String>) -> Error {
    Error::TerminalOperation {
        operation,
        reason: reason.into(),
    }
}
