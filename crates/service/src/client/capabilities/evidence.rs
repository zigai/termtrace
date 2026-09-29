use super::super::client_error;
use super::super::{
    create_bundle, save, verify_bundle, verify_saved_artifact, verify_session, BundleManifest,
    BundleProfile, BundleVerificationPolicy, Path, Result, SaveFormat, SaveOutcome, Session,
    StatePoint, VerificationLevel, VerificationReport,
};

/// Completed evidence, artifact, and retention operations.
pub struct Evidence<'session> {
    pub(in crate::client) session: &'session Session,
}

impl Evidence<'_> {
    /// Save and verify a deterministic semantic PNG for one exact state.
    ///
    /// # Errors
    /// Returns an error when `point` cannot be resolved, the state cannot be rendered, the
    /// destination cannot be published, or the saved artifact fails verification.
    pub fn semantic_screenshot(
        &self,
        destination: impl AsRef<Path>,
        point: StatePoint,
    ) -> Result<SaveOutcome> {
        let outcome = save(
            self.session.path(),
            destination.as_ref(),
            point,
            &[SaveFormat::Png],
            false,
            None,
        )?;

        verify_saved_artifact(&outcome.directory, &outcome.manifest)?;

        return Ok(outcome);
    }

    /// Create and independently verify a bounded evidence bundle.
    ///
    /// # Errors
    /// Returns an error when the bundle cannot be created or fails independent verification, or its
    /// verified manifest differs from the published one.
    pub fn bundle(
        &self,
        destination: impl AsRef<Path>,
        profile: BundleProfile,
    ) -> Result<BundleManifest> {
        let destination = destination.as_ref();
        let manifest = create_bundle(self.session.path(), destination, profile, None)?;
        let policy = BundleVerificationPolicy::new(self.session.path(), profile);
        let verified = verify_bundle(destination, &policy)?;

        if verified != manifest {
            return Err(client_error(
                "verified bundle manifest changed after publication",
            ));
        }
        return Ok(verified);
    }

    /// Verify the completed canonical session without mutating it.
    #[must_use]
    pub fn verify(&self, level: VerificationLevel) -> VerificationReport {
        return verify_session(self.session.path(), level);
    }
}
