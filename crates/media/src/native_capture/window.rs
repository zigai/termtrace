//! Ghostty X11 window geometry shared by replay playback and original-capture supervision.
//!
//! Ghostty ignores window-resize control sequences, so both paths resize its X11 window.

use crate::{Error, Result};
use std::{
    thread,
    time::{Duration, Instant},
};
use x11rb::{
    connection::Connection as _,
    protocol::xproto::{
        AtomEnum, ConfigureWindowAux, ConnectionExt as _, MapState, PropMode, Window,
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

/// The single mapped top-level window of a private display.
pub(crate) struct TerminalWindow {
    connection: RustConnection,
    window: Window,
}

impl TerminalWindow {
    /// Connect to `display`, or `$DISPLAY` when `None`, and wait for exactly one mapped window.
    pub(crate) fn connect(display: Option<&str>, timeout: Duration) -> Result<Self> {
        let (connection, screen) = x11rb::connect(display)
            .map_err(|error| return window_error("connect to the terminal display", error))?;

        let root = connection
            .setup()
            .roots
            .get(screen)
            .ok_or_else(|| {
                return window_error("connect to the terminal display", "screen is missing");
            })?
            .root;

        let deadline = Instant::now() + timeout;
        let window = loop {
            let mapped = mapped_children(&connection, root)?;
            if let [window] = mapped[..] {
                break window;
            }

            if Instant::now() >= deadline {
                return Err(window_error(
                    "find the terminal window",
                    format!("expected one mapped window, found {}", mapped.len()),
                ));
            }

            thread::sleep(Duration::from_millis(20));
        };

        return Ok(Self { connection, window });
    }

    /// Current window width and height in pixels.
    pub(crate) fn size(&self) -> Result<(u16, u16)> {
        let geometry = self
            .connection
            .get_geometry(self.window)
            .map_err(|error| return window_error("measure the terminal window", error))?
            .reply()
            .map_err(|error| return window_error("measure the terminal window", error))?;

        return Ok((geometry.width, geometry.height));
    }

    /// Request a new window size and flush it to the server.
    pub(crate) fn configure(&self, width: u32, height: u32) -> Result<()> {
        self.connection
            .configure_window(
                self.window,
                &ConfigureWindowAux::new().width(width).height(height),
            )
            .map_err(|error| return window_error("resize the terminal window", error))?;

        return self
            .connection
            .flush()
            .map_err(|error| return window_error("resize the terminal window", error));
    }
}

/// Publish `Xft.dpi` on a private display before Ghostty starts.
///
/// Ghostty multiplies its content scale by GTK's font DPI over 96. GTK 4 ignores
/// `GDK_DPI_SCALE` and, without a settings daemon, reads that DPI only from the display's
/// `Xft.dpi` resource, so fractional profile DPI must be set here.
pub(crate) fn publish_font_dpi(display: &str, dpi: u32) -> Result<()> {
    let operation = "publish the display font DPI";
    let (connection, screen) =
        x11rb::connect(Some(display)).map_err(|error| return window_error(operation, error))?;

    let root = connection
        .setup()
        .roots
        .get(screen)
        .ok_or_else(|| return window_error(operation, "screen is missing"))?
        .root;

    connection
        .change_property8(
            PropMode::REPLACE,
            root,
            AtomEnum::RESOURCE_MANAGER,
            AtomEnum::STRING,
            format!("Xft.dpi:\t{dpi}\n").as_bytes(),
        )
        .map_err(|error| return window_error(operation, error))?;
    connection
        .get_input_focus()
        .map_err(|error| return window_error(operation, error))?
        .reply()
        .map_err(|error| return window_error(operation, error))?;

    return Ok(());
}

/// Grid metrics reported by the terminal for its current window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GridMetrics {
    pub(crate) cell_width: u16,
    pub(crate) cell_height: u16,
    pub(crate) text_width: u16,
    pub(crate) text_height: u16,
}

/// Window size giving a `cols` by `rows` grid, keeping the current padding and decoration
/// overhead between the window and its text area.
pub(crate) fn window_size_for_grid(
    cols: u16,
    rows: u16,
    metrics: GridMetrics,
    window: (u16, u16),
) -> (u32, u32) {
    let overhead_width = window.0.saturating_sub(metrics.text_width);
    let overhead_height = window.1.saturating_sub(metrics.text_height);

    return (
        u32::from(cols) * u32::from(metrics.cell_width) + u32::from(overhead_width),
        u32::from(rows) * u32::from(metrics.cell_height) + u32::from(overhead_height),
    );
}

fn mapped_children(connection: &RustConnection, root: Window) -> Result<Vec<Window>> {
    let children = connection
        .query_tree(root)
        .map_err(|error| return window_error("list terminal windows", error))?
        .reply()
        .map_err(|error| return window_error("list terminal windows", error))?
        .children;

    let mut mapped = Vec::new();
    for child in children {
        let attributes = connection
            .get_window_attributes(child)
            .map_err(|error| return window_error("inspect a terminal window", error))?
            .reply()
            .map_err(|error| return window_error("inspect a terminal window", error))?;

        if attributes.map_state == MapState::VIEWABLE {
            mapped.push(child);
        }
    }

    return Ok(mapped);
}

fn window_error(operation: &'static str, reason: impl ToString) -> Error {
    return Error::TerminalOperation {
        operation,
        reason: reason.to_string(),
    };
}
