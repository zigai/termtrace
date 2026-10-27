//! One-shot PTY recording with byte-exact event persistence.

mod actor;
/// Live-session control protocol helpers and screen predicates.
pub mod control;
mod degraded;
mod environment;
mod input;
mod lifecycle;
mod model;
mod outer_resize;
mod presentation;
mod process;
mod raw_terminal;
mod runtime;
mod support;
mod termination;

pub use crate::analysis::profile::ResolvedGhosttyProfile;
pub use control::control_socket_name;
pub use environment::EffectiveEnvironment;
pub use lifecycle::{record, record_with_output, record_with_resize_authority};
pub use model::{RecordOptions, RecordOutcome};
pub use outer_resize::ResizeAuthority;
pub use support::{
    inherited_geometry, parse_environment_spec, read_environment_spec, resolve_ghostty_profile,
    resolved_terminal_colors, restore_ghostty_profile,
};
