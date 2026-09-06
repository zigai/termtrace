//! One-shot PTY recording with byte-exact event persistence.

mod actor;
/// Live-session control protocol helpers and screen predicates.
pub mod control;
mod degraded;
mod environment;
mod input;
mod lifecycle;
mod model;
mod presentation;
mod process;
mod raw_terminal;
mod runtime;
mod support;

pub use crate::analysis::profile::ResolvedGhosttyProfile;
pub use control::control_socket_name;
pub use environment::EffectiveEnvironment;
pub use lifecycle::{record, record_with_output};
pub use model::{RecordOptions, RecordOutcome};
pub use support::{
    inherited_geometry, parse_environment_spec, read_environment_spec, resolve_ghostty_profile,
    resolved_terminal_colors, restore_ghostty_profile,
};
