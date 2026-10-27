use super::super::client_error;
use super::super::{
    create_bundle, save, verify_bundle, verify_saved_artifact, verify_session, BundleManifest,
    BundleProfile, BundleVerificationPolicy, Path, Result, SaveFormat, SaveOutcome, Session,
    StatePoint, VerificationLevel, VerificationReport,
};
use crate::artifacts::ArtifactService as LocalArtifactService;
use crate::integrity::IntegrityService as LocalIntegrityService;
use std::path::PathBuf as StdPathBuf;
use termtrace_model::execution::event_log::RecoveryReport as ModelRecoveryReport;

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

impl Evidence<'_> {
    /// All existing save formats and state selectors, without implicit overwrites.
    /// # Errors
    /// Propagates source/state/render/publication failures.
    pub fn save(
        &self,
        destination: &Path,
        point: StatePoint,
        formats: &[SaveFormat],
        hidden: bool,
    ) -> Result<SaveOutcome> {
        return LocalArtifactService::save(
            self.session.path(),
            destination,
            point,
            formats,
            hidden,
        );
    }

    /// Explicit source/profile verification; failed verification is an error, not a boolean.
    /// # Errors
    /// Propagates invalid source/profile/member/bundle failures.
    pub fn verify_bundle(
        &self,
        directory: &Path,
        profile: BundleProfile,
    ) -> Result<BundleManifest> {
        return LocalArtifactService::verify_bundle(
            directory,
            BundleVerificationPolicy::new(self.session.path(), profile),
        );
    }

    /// Distinguish canonical validity, authenticity and configured signer trust.
    #[must_use]
    pub fn verify_with_trust(
        &self,
        level: VerificationLevel,
        allowed_signers: Option<&Path>,
    ) -> VerificationReport {
        return LocalIntegrityService::verify(self.session.path(), level, allowed_signers);
    }

    /// Sign a detached receipt. Abandoning an await does not undo publication or imply trust.
    /// # Errors
    /// Propagates verification/key/signature/publication failures.
    pub fn sign(&self, key: &Path, signer: &str) -> Result<StdPathBuf> {
        return LocalIntegrityService::sign(self.session.path(), key, signer);
    }

    /// Recover into a distinct destination, leaving the source unchanged.
    /// # Errors
    /// Propagates prefix validation and publication failures.
    pub fn recover(&self, destination: &Path) -> Result<ModelRecoveryReport> {
        return LocalIntegrityService::recover_session(self.session.path(), destination);
    }

    /// Export canonical raw PTY playback as asciicast.
    /// # Errors
    /// Propagates source and destination failures.
    pub fn asciicast(&self, destination: &Path) -> Result<()> {
        return LocalArtifactService::export_asciicast(self.session.path(), destination);
    }
}
