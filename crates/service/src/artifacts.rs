//! Artifact planning, generation, save representations, bundles, and media export.

use crate::error::{Error, Result};
use std::{io::Write, path::Path};
use termtrace_export::{
    asciicast::{export as export_asciicast_file, export_to_writer as export_asciicast_writer},
    bundle::{create as create_bundle_export, verify_profile as verify_bundle_profile},
};
use termtrace_media::save::save as save_representations;
use termtrace_model::{
    artifact::bundle::{BundleManifest, BundleProfile, BundleVerificationPolicy},
    artifact::save::{SaveFormat, SaveOutcome},
    terminal::observation::StatePoint,
};

mod video;

pub use video::{
    source_name, CursorStyle, VideoMode, VideoOutcome, VideoPreset, VideoRenderer, VideoRequest,
};

/// High-level service orchestrating artifact export, bundles, and save formats.
pub struct ArtifactService;

impl ArtifactService {
    /// Export a canonical session as a UTF-8 asciinema v2 asciicast file.
    ///
    /// # Errors
    /// Returns an error if the session cannot be read or export fails.
    pub fn export_asciicast(session: &Path, destination: &Path) -> Result<()> {
        return export_asciicast_file(session, destination).map_err(Error::from);
    }

    /// Stream an asciinema v2 cast directly to a caller-owned writer.
    ///
    /// # Errors
    /// Returns an error if streaming fails.
    pub fn export_asciicast_to_writer<W: Write>(session: &Path, writer: &mut W) -> Result<()> {
        return export_asciicast_writer(session, writer, "<stream>").map_err(Error::from);
    }

    /// Atomically publish multiple save representations of one state point.
    ///
    /// # Errors
    /// Returns an error if saving fails.
    pub fn save(
        session: &Path,
        destination: &Path,
        point: StatePoint,
        formats: &[SaveFormat],
        include_hidden: bool,
    ) -> Result<SaveOutcome> {
        return save_representations(session, destination, point, formats, include_hidden, None)
            .map_err(Error::from);
    }

    /// Create an evidence bundle with the specified sensitivity profile.
    ///
    /// # Errors
    /// Returns an error if bundle creation fails.
    pub fn create_bundle(
        session: &Path,
        destination: &Path,
        profile: BundleProfile,
    ) -> Result<BundleManifest> {
        return create_bundle_export(session, destination, profile, None).map_err(Error::from);
    }

    /// # Errors
    /// Returns an error if bundle verification fails.
    pub fn verify_bundle(
        bundle_directory: &Path,
        policy: BundleVerificationPolicy,
    ) -> Result<BundleManifest> {
        return verify_bundle_profile(bundle_directory, &policy).map_err(Error::from);
    }
}
