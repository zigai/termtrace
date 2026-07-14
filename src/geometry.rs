//! Terminal geometry measurement and playback verification.

use std::{
    io::{self, Write as _},
    os::fd::AsFd as _,
    path::Path,
    result::Result as StdResult,
    thread,
    time::{Duration, Instant},
};

use nix::{
    errno::Errno,
    poll::{PollFd, PollFlags, PollTimeout, poll},
    unistd::read,
};
use rustix::termios::tcgetwinsize;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Result, artifact::write_secure, session::SessionGeometry,
    terminal_mode::RawTerminalGuard,
};

const QUERY_TIMEOUT: Duration = Duration::from_secs(3);

/// Geometry reported by a live terminal through XTWINOPS queries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalGeometryReport {
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
    /// Width of one rendered cell in pixels.
    pub cell_width: u16,
    /// Height of one rendered cell in pixels.
    pub cell_height: u16,
    /// Width of the complete text area in pixels.
    pub text_width: u16,
    /// Height of the complete text area in pixels.
    pub text_height: u16,
}

impl TerminalGeometryReport {
    /// Convert the report into the geometry stored by a recording.
    ///
    /// # Errors
    ///
    /// Returns an error when Ghostty reports internally inconsistent values.
    pub fn session_geometry(self) -> Result<SessionGeometry> {
        let expected_width = self.cols.checked_mul(self.cell_width);
        let expected_height = self.rows.checked_mul(self.cell_height);
        if expected_width != Some(self.text_width) || expected_height != Some(self.text_height) {
            return Err(geometry_error(
                "Ghostty reported inconsistent grid, cell, and text-area dimensions",
            ));
        }
        return Ok(SessionGeometry {
            cols: self.cols,
            rows: self.rows,
            pixel_width: self.text_width,
            pixel_height: self.text_height,
        });
    }
}

/// Measure the controlling terminal through the kernel window-size record.
///
/// Pixel dimensions are used when the terminal provides them. Otherwise the
/// supplied cell dimensions are used as a deterministic fallback.
///
/// # Errors
///
/// Returns an error when standard input has no valid terminal geometry.
pub fn inherited_geometry(
    fallback_cell_width: u16,
    fallback_cell_height: u16,
) -> Result<SessionGeometry> {
    let size = tcgetwinsize(io::stdin())
        .map_err(|error| return geometry_error(format!("read terminal size: {error}")))?;
    if size.ws_col == 0 || size.ws_row == 0 {
        return Err(geometry_error(
            "controlling terminal has no usable grid size",
        ));
    }
    let cell_width = size
        .ws_xpixel
        .checked_div(size.ws_col)
        .filter(|value| return *value > 0)
        .unwrap_or(fallback_cell_width);
    let cell_height = size
        .ws_ypixel
        .checked_div(size.ws_row)
        .filter(|value| return *value > 0)
        .unwrap_or(fallback_cell_height);
    if cell_width == 0 || cell_height == 0 {
        return Err(geometry_error(
            "controlling terminal has invalid cell dimensions",
        ));
    }
    let pixel_width = size
        .ws_col
        .checked_mul(cell_width)
        .ok_or_else(|| return geometry_error("inherited terminal width overflow"))?;
    let pixel_height = size
        .ws_row
        .checked_mul(cell_height)
        .ok_or_else(|| return geometry_error("inherited terminal height overflow"))?;
    return Ok(SessionGeometry {
        cols: size.ws_col,
        rows: size.ws_row,
        pixel_width,
        pixel_height,
    });
}

/// Run the hidden geometry probe inside a real terminal and write its report.
///
/// # Errors
///
/// Returns an error when raw mode, terminal queries, serialization, or output
/// persistence fails.
pub fn run_probe(output: &Path) -> Result<()> {
    let mut terminal_mode = RawTerminalGuard::enter(true)?;
    let result = query_terminal();
    let result = finish_with_cleanup(result, terminal_mode.restore());
    let report = result?;
    let mut encoded = serde_json::to_vec_pretty(&report).map_err(|source| {
        return Error::InvalidJson {
            path: output.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    encoded.push(b'\n');
    return write_secure(output, &encoded);
}

pub(crate) fn resize_and_query(cols: u16, rows: u16) -> Result<TerminalGeometryReport> {
    if cols == 0 || rows == 0 {
        return Err(geometry_error("playback geometry must be non-zero"));
    }
    let stdout = io::stdout();
    let mut output = stdout.lock();
    write!(output, "\x1b[8;{rows};{cols}t")
        .and_then(|()| return output.flush())
        .map_err(|error| return geometry_error(format!("request terminal resize: {error}")))?;
    thread::sleep(Duration::from_millis(150));
    return query_terminal();
}

pub(crate) fn query_terminal() -> Result<TerminalGeometryReport> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    output
        .write_all(b"\x1b[14t\x1b[16t\x1b[18t")
        .and_then(|()| return output.flush())
        .map_err(|error| return geometry_error(format!("query terminal geometry: {error}")))?;

    let stdin = io::stdin();
    let deadline = Instant::now() + QUERY_TIMEOUT;
    let mut bytes = Vec::with_capacity(128);
    let mut buffer = [0_u8; 128];
    loop {
        if let Some(report) = parse_report(&bytes)? {
            return Ok(report);
        }
        if Instant::now() >= deadline {
            return Err(geometry_error(
                "timed out waiting for terminal geometry reports",
            ));
        }
        let mut descriptors = [PollFd::new(stdin.as_fd(), PollFlags::POLLIN)];
        match poll(&mut descriptors, PollTimeout::from(100_u16)) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(Errno::EINTR) => continue,
            Err(error) => return Err(geometry_error(format!("poll terminal geometry: {error}"))),
        }
        match read(&stdin, &mut buffer) {
            Ok(0) | Err(Errno::EIO) => {
                return Err(geometry_error(
                    "terminal closed before reporting its geometry",
                ));
            }
            Ok(length) => {
                let Some(chunk) = buffer.get(..length) else {
                    return Err(geometry_error("terminal returned an invalid read length"));
                };
                bytes.extend_from_slice(chunk);
                if bytes.len() > 4096 {
                    return Err(geometry_error(
                        "terminal geometry response exceeded 4096 bytes",
                    ));
                }
            }
            Err(Errno::EINTR) => {}
            Err(error) => return Err(geometry_error(format!("read terminal geometry: {error}"))),
        }
    }
}

fn parse_report(bytes: &[u8]) -> Result<Option<TerminalGeometryReport>> {
    let text = String::from_utf8_lossy(bytes);
    let mut text_size = None;
    let mut cell_size = None;
    let mut grid_size = None;
    for sequence in text.split('\x1b').skip(1) {
        let Some(body) = sequence.strip_prefix('[') else {
            continue;
        };
        let Some((parameters, _)) = body.split_once('t') else {
            continue;
        };
        let values = parameters
            .split(';')
            .map(|value| return value.parse::<u16>())
            .collect::<StdResult<Vec<_>, _>>()
            .map_err(|error| return geometry_error(format!("parse terminal geometry: {error}")))?;
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
        return Err(geometry_error("terminal reported zero-sized geometry"));
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

fn finish_with_cleanup<T>(primary: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => return Ok(value),
        (Err(primary), Ok(())) => return Err(primary),
        (Ok(_), Err(cleanup)) => return Err(cleanup),
        (Err(primary), Err(cleanup)) => {
            return Err(Error::CleanupFailed {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            });
        }
    }
}

fn geometry_error(reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation: "resolve terminal geometry",
        reason: reason.into(),
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
