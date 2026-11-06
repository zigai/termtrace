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
mod scanner;
mod snapshot;
mod state;
mod sys;
mod terminal;

pub use input::encode_focus;
pub use scanner::{ControlString, ScanKind, ScanStep, SequenceScanner, StringEnd, StringStart};
pub use state::Format;
pub use terminal::{GhosttyVt, OutputStep, RenderHold};
use termtrace_model::terminal::replay::GHOSTTY_REVISION;

// The model crate is published alone and cannot read crates/terminal/ghostty-revision, so it
// keeps its own copy of the pin; compilation stops here when the two copies diverge.
const _: () = assert!(
    same_bytes(
        GHOSTTY_REVISION.as_bytes(),
        env!("TERMTRACE_GHOSTTY_REVISION").as_bytes()
    ),
    "termtrace_model::terminal::replay::GHOSTTY_REVISION differs from crates/terminal/ghostty-revision"
);

const fn same_bytes(left: &[u8], right: &[u8]) -> bool {
    return match (left.split_first(), right.split_first()) {
        (Some((left_first, left_rest)), Some((right_first, right_rest))) => {
            *left_first == *right_first && same_bytes(left_rest, right_rest)
        }
        (None, None) => true,
        _ => false,
    };
}
