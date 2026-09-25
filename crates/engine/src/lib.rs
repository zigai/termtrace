//! Stateful Termtrace recording, replay, observation, and analysis.
//!
//! This crate coordinates canonical [`termtrace_evidence`] with the semantic
//! terminal implementation in [`termtrace_terminal`]. Stable data contracts
//! remain owned by [`termtrace_model`]; engine modules re-export only model
//! types that form part of their operation surfaces.
//!
//! Recording keeps PTY, event-log, and live semantic state under one actor.
//! Replay and observation derive state without mutating canonical evidence.
//! Timeline, diff, inspection, analysis, and registry APIs build on those
//! boundaries while internal checkpoint and replay coordination stays private.

/// Evidence-backed semantic analysis and read-only review.
pub mod analysis;
mod completion;
/// Semantic differences between replayed terminal states.
pub mod diff;
mod error;
/// Direct inspection and bounded waiting over canonical sessions.
pub mod inspection;
/// Truth-labeled terminal observations and replay-cache management.
pub mod observation;
/// PTY recording, child-environment resolution, and live control.
pub mod recording;
/// Secure allocation and lookup of named recording sessions.
pub mod registry;
/// Deterministic replay and derived semantic indexes.
pub mod replay;
/// Chronological views and source-range selection.
pub mod timeline;

pub use error::{Error, Result};
