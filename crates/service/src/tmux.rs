//! Tmux aggregate recording, catalog inspection, bundle export, and setup services.

use crate::error::{Error, Result};
use std::path::Path;
use termtrace_evidence::tmux_catalog::TmuxCatalog;
use termtrace_export::tmux::streaming::{
    create as create_tmux_streaming_bundle, verify as verify_tmux_streaming_bundle,
};
use termtrace_model::artifact::tmux::{TmuxRecordingManifest, TmuxStreamingBundleManifest};

/// Service managing tmux aggregate recording lifecycles, catalogs, and streaming bundles.
pub struct TmuxService;

impl TmuxService {
    /// Bundle an aggregate tmux recording into an evidentiary streaming bundle archive.
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

    /// Open an index catalog reader for a tmux aggregate recording.
    ///
    /// # Errors
    /// Returns an error if the catalog cannot be read or validated.
    pub fn catalog_reader(
        directory: &Path,
        manifest: &TmuxRecordingManifest,
    ) -> Result<TmuxCatalog> {
        return TmuxCatalog::read(directory, manifest).map_err(Error::from);
    }

    /// Open a fully verified index catalog reader.
    ///
    /// # Errors
    /// Returns an error if verification fails.
    pub fn catalog_reader_verified(
        directory: &Path,
        manifest: &TmuxRecordingManifest,
    ) -> Result<TmuxCatalog> {
        return TmuxCatalog::read_verified(directory, manifest).map_err(Error::from);
    }
}
