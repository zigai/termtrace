//! Terminal emulation, VT framing, and semantic screen projection.

#![deny(unsafe_code)]

mod error;
/// Safe adapter around the pinned libghostty-vt engine.
pub mod ghostty_vt;
/// Streaming projection of child output into printable text.
pub mod output_projection;
/// Pure semantic-screen projection and hashing.
pub mod screen;
/// Stateful trackers for presentation and terminal-query sequences.
pub mod tracking;
/// Lossless framing of canonical output into VT tokens.
pub mod vt_framer;

pub use error::{Error, Result};
pub use ghostty_vt::{encode_focus, supported_geometry_cell_count, Format, GhosttyVt};
pub use output_projection::ControlStripper;
pub use screen::{
    hash_semantic_row, project_screen_frame, screen_frame_hash, semantic_state_hash,
    semantic_state_hash_from_row_hashes, semantic_viewport_text, validate_screen_region,
};
pub use tracking::{PresentationAction, PresentationTracker, PresentationTransition, QueryTracker};
pub use vt_framer::{frame_event_records, VtFramer};
