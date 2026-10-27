//! Recorder-to-supervisor protocol for resizing the outer terminal window.
//!
//! When the recorder runs inside a supervised terminal, resize authority belongs to that
//! terminal's window. The recorder sends one request per connection and reads one reply.
//! Every message is a single JSON line.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Maximum bytes in one protocol line.
pub const MAX_OUTER_RESIZE_LINE_BYTES: u64 = 4096;

/// Resize the outer terminal window so its grid becomes `cols` by `rows`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OuterResizeRequest {
    /// Caller request identity.
    pub request_id: Uuid,
    /// Requested columns.
    pub cols: u16,
    /// Requested rows.
    pub rows: u16,
}

/// Supervisor decision for one request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum OuterResizeReply {
    /// The window was configured and flushed; the PTY grid change is observed separately.
    Configured {
        /// Request identity.
        request_id: Uuid,
        /// Configured window width in pixels.
        window_width: u32,
        /// Configured window height in pixels.
        window_height: u32,
    },
    /// The window was not touched.
    Rejected {
        /// Request identity.
        request_id: Uuid,
        /// Stable code such as `exceeds_native_canvas`.
        code: String,
        /// Human-readable reason.
        message: String,
    },
}
