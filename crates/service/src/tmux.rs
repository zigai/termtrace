//! Evidence export for completed aggregate tmux recordings.
//!
//! Recording, observation, and stop live in `termtrace-tmux`; this service only exposes
//! the streaming bundle export.

use crate::error::{Error, Result};
use std::path::Path;
use termtrace_export::tmux::streaming::{
    create as create_tmux_streaming_bundle, verify as verify_tmux_streaming_bundle,
};
use termtrace_tmux::artifact::TmuxStreamingBundleManifest;

/// Streaming evidence bundles of completed aggregate tmux recordings.
pub struct TmuxService;

impl TmuxService {
    /// Bundle a completed aggregate tmux recording into a streaming evidence bundle.
    ///
    /// # Errors
    /// Returns an error if bundling fails or evidence is incomplete.
    pub fn bundle(
        aggregate_manifest: &Path,
        destination: &Path,
    ) -> Result<TmuxStreamingBundleManifest> {
        return create_tmux_streaming_bundle(aggregate_manifest, destination).map_err(Error::from);
    }

    /// Verify an aggregate tmux streaming bundle against its manifest.
    ///
    /// # Errors
    /// Returns an error if verification fails or members do not match.
    pub fn verify_bundle(
        directory: &Path,
        aggregate_manifest: &Path,
    ) -> Result<TmuxStreamingBundleManifest> {
        return verify_tmux_streaming_bundle(directory, aggregate_manifest).map_err(Error::from);
    }
}
