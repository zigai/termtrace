//! Stable, serializable value contracts shared across Termtrace crates.
//!
//! This crate owns data only. Filesystem access, process control, replay traversal,
//! rendering, media production, and other side effects belong to higher layers.

/// Derived artifact metadata and publication contracts.
pub mod artifact;
/// Model validation failures.
pub mod error;
/// Execution history, environment, session, and provenance contracts.
pub mod execution;
/// Control, capability, and CLI response protocol contracts.
pub mod protocol;
/// Named-session registry values.
pub mod registry;
/// Cross-domain scenario document and report contracts.
pub mod scenario;
/// Terminal state, observation, replay, and analysis contracts.
pub mod terminal;
/// Shared scalar and compact value contracts.
pub mod value;
