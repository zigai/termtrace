//! Native terminal mode lifecycle for interactive recording.

use std::io;

use nix::{
    sys::termios::{cfmakeraw, tcgetattr, tcsetattr, SetArg, Termios},
    unistd::isatty,
};

use super::support::terminal_error;
use crate::Result;

pub(super) struct RawTerminalGuard {
    original: Option<Termios>,
}

impl RawTerminalGuard {
    pub(super) fn enter(enabled: bool) -> Result<Self> {
        let stdin = io::stdin();
        if !enabled
            || !isatty(&stdin).map_err(|error| {
                return terminal_error("inspect standard input", error.to_string());
            })?
        {
            return Ok(Self { original: None });
        }

        let original = tcgetattr(&stdin)
            .map_err(|error| return terminal_error("read terminal mode", error.to_string()))?;
        let mut raw = original.clone();
        cfmakeraw(&mut raw);
        tcsetattr(&stdin, SetArg::TCSANOW, &raw)
            .map_err(|error| return terminal_error("enter raw terminal mode", error.to_string()))?;

        return Ok(Self {
            original: Some(original),
        });
    }

    pub(super) fn restore(&mut self) -> Result<()> {
        let Some(original) = self.original.take() else {
            return Ok(());
        };

        return tcsetattr(io::stdin(), SetArg::TCSANOW, &original)
            .map_err(|error| return terminal_error("restore terminal mode", error.to_string()));
    }
}

impl Drop for RawTerminalGuard {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
