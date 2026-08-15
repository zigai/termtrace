//! Policy-bound deterministic evidence archives.

use std::path::Path;

use termtrace_evidence::bundle::{archive as create_archive, verify_archive};
use termtrace_model::{
    artifact::bundle::BundleManifest, artifact::bundle::BundleVerificationPolicy,
};

use crate::{Result, bundle};

/// Create a deterministic Zstandard-compressed USTAR evidence archive.
///
/// The source directory is verified against the caller-owned canonical session
/// and exact export profile before the no-replace archive publication begins.
///
/// # Errors
///
/// Returns an error when the source violates the verification policy, the
/// destination exists, or the archive cannot be durably published.
pub fn create(
    directory: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    policy: &BundleVerificationPolicy,
) -> Result<()> {
    let directory = directory.as_ref();
    bundle::verify_profile(directory, policy)?;
    let identity = bundle::identity_for_policy(policy)?;
    create_archive(directory, destination.as_ref(), Some(&identity))?;
    return Ok(());
}

/// Verify a compressed evidence archive against a caller-owned canonical
/// session and exact export profile.
///
/// Extraction is private, bounded, and rejects links, special files, duplicate
/// paths, path traversal, oversized payloads, and trailing archive data.
///
/// # Errors
///
/// Returns an error when extraction is unsafe, payload integrity fails, or the
/// archive identity or member policy differs from the requested policy.
pub fn verify(
    archive: impl AsRef<Path>,
    policy: &BundleVerificationPolicy,
) -> Result<BundleManifest> {
    let identity = bundle::identity_for_policy(policy)?;
    let manifest = verify_archive(archive.as_ref(), Some(&identity))?;
    bundle::verify_archived_manifest(&manifest, policy)?;
    return Ok(manifest);
}
