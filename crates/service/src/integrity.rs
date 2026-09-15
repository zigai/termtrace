//! Cryptographic integrity verification, OpenSSH signing, and evidence recovery.

use crate::error::{Error, Result};
use std::path::{Path, PathBuf};
use termtrace_evidence::{
    event_log::{EventLog, RecoveryReport},
    receipt::create_ssh_receipt,
    session::{recover_session as recover_canonical_session, verify_session_with_trust},
};
use termtrace_model::execution::session::{VerificationLevel, VerificationReport};

/// Service for verifying, signing, and recovering canonical evidence.
pub struct IntegrityService;

impl IntegrityService {
    /// Verify a completed session's structure and evidence bindings offline.
    #[must_use]
    pub fn verify(
        session: &Path,
        level: VerificationLevel,
        allowed_signers: Option<&Path>,
    ) -> VerificationReport {
        return verify_session_with_trust(session, level, allowed_signers);
    }

    /// Create a detached OpenSSH signature receipt for a verified session.
    ///
    /// # Errors
    /// Returns an error if signing fails or the key is invalid.
    pub fn sign(session: &Path, key: &Path, signer: &str) -> Result<PathBuf> {
        return create_ssh_receipt(session, key, signer).map_err(Error::from);
    }

    /// Copy the longest integrity-checked event prefix without modifying the source.
    ///
    /// # Errors
    /// Returns an error if session recovery fails.
    pub fn recover_session(session: &Path, destination: &Path) -> Result<RecoveryReport> {
        return recover_canonical_session(session, destination).map_err(Error::from);
    }

    /// Recover an individual damaged event log to a destination path.
    ///
    /// # Errors
    /// Returns an error if log recovery fails.
    pub fn recover_log(source_log: &Path, destination_log: &Path) -> Result<RecoveryReport> {
        return EventLog::recover_prefix(source_log, destination_log).map_err(Error::from);
    }
}
