//! Safe, narrow adapter around the pinned libghostty-vt C API.

#![allow(
    unsafe_code,
    reason = "the module is the audited boundary around Ghostty's C API"
)]
#![allow(
    clippy::struct_excessive_bools,
    reason = "terminal mode snapshots intentionally preserve independent protocol flags"
)]

mod input;
mod snapshot;
mod sys;
mod terminal;
mod types;

pub use input::encode_focus;
pub use terminal::GhosttyVt;
pub use types::{Format, supported_geometry_cell_count};
