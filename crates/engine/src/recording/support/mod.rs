//! Host-facing support for configuring deterministic recordings.

mod actor;
mod environment;
mod geometry;
mod profile;

pub(super) use actor::{
    actor_boundaries, control_error_response, control_rejected, deliver_response,
    finish_with_cleanup, poll_ready, terminal_error, thread_panicked, validate_control_input,
};
pub use environment::{parse_environment_spec, read_environment_spec};
pub use geometry::inherited_geometry;
pub use profile::{resolve_ghostty_profile, resolved_terminal_colors, restore_ghostty_profile};
