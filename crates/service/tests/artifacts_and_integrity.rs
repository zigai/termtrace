//! Integration tests for `ArtifactService` and `IntegrityService`.

mod support;

use support::record_session;
use tempfile::TempDir;
use termtrace_model::{
    artifact::{
        bundle::{BundleProfile, BundleVerificationPolicy},
        save::SaveFormat,
    },
    execution::session::VerificationLevel,
    terminal::observation::StatePoint,
};
use termtrace_service::{artifacts::ArtifactService, integrity::IntegrityService};

#[test]
fn test_artifact_save_bundle_and_verify() {
    let temp = TempDir::new().expect("tempdir");
    let session_path = temp.path().join("sample-artifact.termtrace");
    record_session(&session_path, "printf 'ARTIFACT-TEST\n'; sleep 0.05");

    // Test Save
    let save_dir = temp.path().join("saved-state");
    let outcome = ArtifactService::save(
        &session_path,
        &save_dir,
        StatePoint::LatestPresented,
        &[SaveFormat::Txt],
        false,
    )
    .expect("save state");
    assert_eq!(outcome.directory, save_dir);
    assert!(save_dir.join("save-manifest.json").is_file());

    // Test Create Bundle
    let bundle_dir = temp.path().join("bundle-output");
    let manifest =
        ArtifactService::create_bundle(&session_path, &bundle_dir, BundleProfile::Forensic)
            .expect("create bundle");

    assert!(bundle_dir.join("manifest.json").is_file());
    assert_eq!(manifest.profile, BundleProfile::Forensic);

    // Test Verify Bundle
    let verified = ArtifactService::verify_bundle(
        &bundle_dir,
        BundleVerificationPolicy::new(&session_path, BundleProfile::Forensic),
    )
    .expect("verify bundle");
    assert_eq!(verified.profile, BundleProfile::Forensic);
}

#[test]
fn test_integrity_verify_and_recover() {
    let temp = TempDir::new().expect("tempdir");
    let session_path = temp.path().join("sample-integrity.termtrace");
    record_session(&session_path, "printf 'ARTIFACT-TEST\n'; sleep 0.05");

    // Verify session
    let report = IntegrityService::verify(&session_path, VerificationLevel::Content, None);
    assert!(report.valid);
    assert_eq!(report.level, VerificationLevel::Content);

    // Recover session
    let recovered_dir = temp.path().join("recovered.termtrace");
    let recover_report =
        IntegrityService::recover_session(&session_path, &recovered_dir).expect("recover");
    assert!(recover_report.valid_events > 0);
}
