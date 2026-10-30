//! Safe, narrow adapter around the pinned libghostty-vt C API.

#![allow(
    unsafe_code,
    clippy::undocumented_unsafe_blocks,
    reason = "the module is the audited boundary around Ghostty's C API"
)]
#![allow(
    clippy::struct_excessive_bools,
    reason = "terminal mode snapshots intentionally preserve independent protocol flags"
)]

mod input;
mod snapshot;
mod state;
mod sys;
mod terminal;

pub use input::encode_focus;
pub use state::Format;
pub use terminal::{GhosttyVt, OutputStep, RenderHold};
