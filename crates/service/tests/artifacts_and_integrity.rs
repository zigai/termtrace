//! Integration tests for `ArtifactService` and `IntegrityService`.

mod support;

use assert_cmd::cargo::cargo_bin;
use std::sync::atomic::AtomicBool;
use support::record_session;
use tempfile::TempDir;
use termtrace_model::{
    artifact::{
        bundle::{BundleProfile, BundleVerificationPolicy},
        presentation::{EditClip, EditPlan, EditSelector, PresentationSource},
        save::SaveFormat,
    },
    execution::session::VerificationLevel,
    terminal::observation::StatePoint,
};
use termtrace_service::{
    artifacts::{
        ArtifactService, EditSource, VideoMode, VideoOptions, VideoOutcome, VideoRenderer,
    },
    integrity::IntegrityService,
    system::SystemService,
};

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

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn native_edit_render_captures_native_pixels_without_a_semantic_fallback() {
    let temp = TempDir::new().expect("tempdir");
    let session = temp.path().join("native-edit.termtrace");
    record_session(&session, "printf 'NATIVE-EDIT\n'; sleep 1");

    assert!(
        !session.join("visual").exists(),
        "no native capture before the render"
    );

    let edit = EditPlan::new(vec![EditClip::new(
        EditSelector::monotonic_ns(0),
        EditSelector::monotonic_ns(500_000_000),
    )]);

    let options = VideoOptions {
        renderer: VideoRenderer::Native,
        fps: Some(30),
        ..VideoOptions::default()
    };
    let request = options
        .request(
            session,
            EditSource::Plan {
                edit: Box::new(edit),
                resource_base: None,
            },
            VideoMode::Render {
                out: temp.path().join("native.mp4"),
            },
        )
        .expect("request");

    let cancelled = AtomicBool::new(false);
    let outcome = ArtifactService::video(
        &request,
        &cargo_bin("termtrace"),
        &mut |id| return SystemService::require(id, &cancelled),
        &mut |_| {},
    )
    .expect("native render");
    let VideoOutcome::Rendered(render) = outcome else {
        panic!("render returned another outcome");
    };

    assert!(matches!(
        render.plan.source,
        PresentationSource::Native { .. }
    ));

    let manifest: serde_json::Value = serde_json::from_slice(
        &fs_err::read(render.sidecar.join("manifest.json")).expect("sidecar manifest"),
    )
    .expect("manifest JSON");
    assert_eq!(manifest["truth_layer"], "presentation_output");
    assert_eq!(manifest["source_truth_layer"], "native_replay");
}
