//! Semantic analysis, capability composition, profiling, and read-only review.

/// Runtime capability discovery and requirement checks.
pub mod capability;
/// Evidence-backed diagnostic findings and explanations.
pub mod diagnostics;
/// Analysis scoped to the most recent recorded input action.
pub mod last_action;
/// Ghostty replay-profile resolution and validation.
pub mod profile;
/// Read-only session summaries, searches, and state exports.
pub mod reviewer;
