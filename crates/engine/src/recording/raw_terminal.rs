//! Native terminal mode lifecycle for interactive recording.

use super::support::terminal_error;
use crate::Result;
use nix::{
    sys::termios::{cfmakeraw, tcgetattr, tcsetattr, SetArg, Termios},
    unistd::isatty,
};
use std::io::{self, Write as _};

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
        let res = tcsetattr(io::stdin(), SetArg::TCSANOW, &original)
            .map_err(|error| return terminal_error("restore terminal mode", error.to_string()));

        // Standard output may not be the terminal, so resetting its screen modes stays best-effort.
        let mut stdout = io::stdout().lock();
        if let Err(error) = stdout
            .write_all(b"\x1b[?1049l\x1b[?25h")
            .and_then(|()| return stdout.flush())
        {
            tracing::debug!(%error, "could not reset terminal screen modes");
        }

        return res;
    }
}

impl Drop for RawTerminalGuard {
    fn drop(&mut self) {
        if let Err(error) = self.restore() {
            tracing::warn!(%error, "could not restore terminal mode while dropping the raw-mode guard");
        }
    }
}
