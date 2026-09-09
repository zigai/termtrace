use crate::{Error, Result};
use rustix::termios::tcgetwinsize;
use std::io;
use termtrace_model::execution::session::SessionGeometry;

/// Measure the controlling terminal through the kernel window-size record.
///
/// Pixel dimensions are used when present. Otherwise the supplied cell dimensions
/// are used as the deterministic fallback recorded by the caller's provenance.
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

fn geometry_error(reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation: "resolve terminal geometry",
        reason: reason.into(),
    };
}
