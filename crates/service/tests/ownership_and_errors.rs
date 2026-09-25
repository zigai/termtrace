//! Contract tests for attaching, reading unreachable sessions, and error classification.

use std::{collections::BTreeMap, ffi::OsString, path::Path, time::Duration};
use tempfile::TempDir;
use termtrace_evidence::{
    provenance::{collect_execution_provenance, RecorderBuildIdentity},
    session::canonical_manifest_bytes,
};
use termtrace_model::{
    execution::{
        environment::ChildEnvironmentPolicy,
        event_log::DurabilityPolicy,
        layout::{self, SessionLayout},
        provenance::ProvenancePolicy,
        session::{
            GeometryProvenance, ResourceLimits, SessionGeometry, SessionManifest, SessionPhase,
            SESSION_MANIFEST_FORMAT, SESSION_MANIFEST_VERSION,
        },
    },
    protocol::mutation::{MutationEffect, Retry},
    terminal::observation::StatePoint,
    value::ByteString,
};
use termtrace_service::{Error, ErrorKind, Session, StopRequest};
use uuid::Uuid;

#[allow(clippy::expect_used, reason = "test setup helper")]
fn write_manifest(dir: &Path) -> Uuid {
    let session_id = Uuid::now_v7();
    let manifest = SessionManifest {
        failure_policy: Default::default(),
        format: SESSION_MANIFEST_FORMAT.to_owned(),
        version: SESSION_MANIFEST_VERSION,
        session_id,
        creation_alias: None,
        lineage: None,
        import_provenance: None,
        tmux_pane: None,
        started_unix_ns: 1_000_000,
        command: vec![ByteString::from(&b"true"[..])],
        working_directory: Some(ByteString::from(&b"/tmp"[..])),
        geometry: SessionGeometry {
            cols: 80,
            rows: 24,
            pixel_width: 720,
            pixel_height: 432,
        },
        geometry_provenance: GeometryProvenance::default(),
        ghostty_visual: None,
        child_environment: ChildEnvironmentPolicy::default(),
        execution_provenance: collect_execution_provenance(
            &ProvenancePolicy::default(),
            &BTreeMap::new(),
            &[OsString::from("/bin/true")],
            Path::new("/"),
            RecorderBuildIdentity {
                version: env!("CARGO_PKG_VERSION"),
                source_revision: "test",
                semantic_engine_revision: "test",
            },
        ),
        termtrace_version: env!("CARGO_PKG_VERSION").to_owned(),
        operating_system: "linux".to_owned(),
        architecture: "test".to_owned(),
        event_log: layout::EVENT_LOG.to_owned(),
        control_socket: format!("termtrace-control-{session_id}"),
        request_journal: layout::MUTATION_JOURNAL.to_owned(),
        contains_sensitive_terminal_data: true,
        durability_policy: DurabilityPolicy::default(),
        resource_limits: ResourceLimits::default(),
    };
    let bytes = canonical_manifest_bytes(&manifest).expect("bytes");
    fs_err::write(SessionLayout::new(dir).manifest(), bytes).expect("write manifest");

    return session_id;
}

#[test]
fn attach_classifies_missing_directories_and_manifests() {
    let temporary = TempDir::new().expect("temporary directory");

    let missing = Session::attach(temporary.path().join("absent"), "termtrace")
        .err()
        .expect("missing directory");
    assert_eq!(missing.kind(), ErrorKind::Filesystem);

    let unpublished = Session::attach(temporary.path(), "termtrace")
        .err()
        .expect("missing manifest");
    assert_eq!(unpublished.kind(), ErrorKind::Evidence);
}

#[test]
fn an_unreachable_incomplete_session_reports_unavailable_without_mutation() {
    let temporary = TempDir::new().expect("temporary directory");
    let session_id = write_manifest(temporary.path());
    let mut session = Session::attach(temporary.path(), "termtrace").expect("attach");
    assert_eq!(session.session_id(), session_id);
    assert!(!session.launched());
    assert_eq!(session.phase().phase, SessionPhase::Incomplete);

    let observed = session
        .observe(StatePoint::LatestPresented, false)
        .expect_err("no actor and no completion");
    assert_eq!(observed.kind(), ErrorKind::ControlUnavailable);
    assert_eq!(observed.retry(), Retry::NewRequest);

    let historical = session
        .observe(StatePoint::StateId { state_id: 0 }, false)
        .expect_err("history needs completion");
    assert_eq!(historical.kind(), ErrorKind::InvalidOperation);

    let stopped = session
        .stop(StopRequest::default(), Some(Duration::from_millis(50)))
        .expect_err("stop cannot reach the actor");
    assert_eq!(stopped.effect(), MutationEffect::NotApplied);
}

#[test]
fn actor_rejection_codes_classify_without_losing_the_code() {
    let rejected = |code: &str, effect| {
        return Error::Control(termtrace_control::Error::Rejected {
            code: code.to_owned(),
            message: "rejected".to_owned(),
            effect,
            details: None,
        });
    };
    let stale = rejected("stale_observation", MutationEffect::NotApplied);
    assert_eq!(stale.kind(), ErrorKind::StaleState);
    assert_eq!(stale.code(), "stale_observation");
    assert_eq!(stale.retry(), Retry::NewRequest);

    let partial = rejected(
        "input_sequence_incomplete",
        MutationEffect::Partial { completed_atoms: 2 },
    );
    assert_eq!(partial.kind(), ErrorKind::PartialInput);
    assert_eq!(
        partial.effect(),
        MutationEffect::Partial { completed_atoms: 2 }
    );
    assert_eq!(partial.retry(), Retry::InspectFirst);

    let busy = rejected("action_busy", MutationEffect::NotApplied);
    assert_eq!(busy.kind(), ErrorKind::ActionBusy);
    let other = rejected("invalid_input", MutationEffect::NotApplied);
    assert_eq!(other.kind(), ErrorKind::ControlRejected);
    assert_eq!(other.code(), "invalid_input");

    let local = Error::InvalidOperation {
        reason: "caller contract".to_owned(),
    };

    assert_eq!(local.code(), "invalid_operation");
    assert!(local.details().is_none());
}
