//! Contract and fault tests for ownership, lifecycle, context, and error classifications.

use nix::{
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::Path,
    process::Command,
    thread,
    time::Duration,
};
use tempfile::TempDir;
use termtrace_evidence::{
    provenance::{collect_execution_provenance, RecorderBuildIdentity},
    session::canonical_manifest_bytes,
};
use termtrace_model::{
    execution::{
        environment::ChildEnvironmentPolicy,
        event_log::{CanonicalBoundary, DurabilityPolicy},
        provenance::ProvenancePolicy,
        session::{
            GeometryProvenance, ResourceLimits, SessionGeometry, SessionManifest,
            SESSION_MANIFEST_FORMAT, SESSION_MANIFEST_VERSION,
        },
    },
    protocol::control::InputSequenceFailureDetails,
    terminal::observation::{ActorBoundaries, StatePoint},
    value::ByteString,
};
use termtrace_service::{
    context::OperationContext,
    error::{Error, ErrorKind, MutationEffect, RetrySafety},
    recording::{DetachedRecording, Recording},
    session::Session,
    types::{Operation, OperationStatus, SessionId, StateSelector},
};
use tokio::runtime::Builder as TokioBuilder;
use uuid::Uuid;

#[allow(clippy::expect_used, reason = "test setup helper")]
fn setup_session(dir: &Path) -> Session {
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
        event_log: "events.jsonl".to_owned(),
        control_socket: format!("termtrace-control-{session_id}"),
        request_journal: "requests.jsonl".to_owned(),
        contains_sensitive_terminal_data: true,
        durability_policy: DurabilityPolicy::default(),
        resource_limits: ResourceLimits::default(),
    };
    let bytes = canonical_manifest_bytes(&manifest).expect("bytes");
    fs_err::write(dir.join("session.json"), bytes).expect("write manifest");
    return Session::from_directory(dir).expect("session");
}

#[test]
fn test_operation_context_deadlines_and_intent() {
    let context = OperationContext::new()
        .with_intent("test-audit-intent")
        .with_timeout(Duration::from_millis(100));
    assert_eq!(context.intent.as_deref(), Some("test-audit-intent"));
    assert!(context.remaining_duration().is_some());
    assert!(context.remaining_duration().unwrap() <= Duration::from_millis(100));
    assert!(context.check_deadline("pre-test").is_ok());

    thread::sleep(Duration::from_millis(110));

    assert_eq!(context.remaining_duration(), Some(Duration::ZERO));

    let err = context.check_deadline("post-test").unwrap_err();
    assert_eq!(err.kind(), ErrorKind::ControlTimeout);
}

#[test]
fn test_session_id_validation_rejects_traversal_and_empty() {
    assert!(SessionId::new("").is_err());
    assert!(SessionId::new("   ").is_err());
    assert!(SessionId::new("path/traversal").is_err());
    assert!(SessionId::new("path\\traversal").is_err());
    assert!(SessionId::new("../escaped").is_err());

    let valid = SessionId::new("canonical-session-123").expect("valid session id");
    assert_eq!(valid.as_str(), "canonical-session-123");
    assert_eq!(format!("{valid}"), "canonical-session-123");
}

#[test]
fn test_state_selector_to_state_point_mapping() {
    assert_eq!(
        StateSelector::LatestPresented.to_state_point(),
        StatePoint::LatestPresented
    );
    assert_eq!(
        StateSelector::StateId(42).to_state_point(),
        StatePoint::StateId { state_id: 42 }
    );
    assert_eq!(
        StateSelector::StateHash("abc".to_owned()).to_state_point(),
        StatePoint::StateHash {
            state_hash: "abc".to_owned()
        }
    );
    assert_eq!(
        StateSelector::Marker("checkpoint".to_owned()).to_state_point(),
        StatePoint::Marker {
            label: "checkpoint".to_owned()
        }
    );
    assert_eq!(
        StateSelector::Offset(Duration::from_millis(500)).to_state_point(),
        StatePoint::MonotonicNs {
            monotonic_ns: 500_000_000
        }
    );
}

#[test]
fn test_error_kind_codes_mutation_effects_and_retry_safety() {
    let stale = Error::StaleState {
        expected_state_hash: "0".repeat(64),
        current_state_hash: "1".repeat(64),
        boundaries: Box::new(ActorBoundaries {
            durable: CanonicalBoundary {
                next_event_seq: 0,
                output_bytes: 0,
                event_hash: "0".repeat(64),
            },
            observed: CanonicalBoundary {
                next_event_seq: 0,
                output_bytes: 0,
                event_hash: "0".repeat(64),
            },
        }),
        retry_safety: RetrySafety::NewRequest,
    };

    assert_eq!(stale.kind(), ErrorKind::StaleState);
    assert_eq!(stale.code(), "stale_state");
    assert_eq!(
        stale.mutation_effect(),
        MutationEffect::RejectedBeforeMutation
    );
    assert_eq!(stale.retry_safety(), RetrySafety::NewRequest);

    let partial = Error::PartialInput {
        details: InputSequenceFailureDetails {
            sequence_id: uuid::Uuid::nil(),
            atom_count: 5,
            atoms_applied: 2,
            failed_atom_index: 2,
            failed_operation_id: uuid::Uuid::nil(),
            bytes_written: 10,
            failed_atom_bytes_written: 0,
            first_intent_event_seq: 1,
            last_result_event_seq: 2,
            resume_from_atom_index: 2,
        },
        retry_safety: RetrySafety::SameRequestOnly,
        uncertain: false,
    };

    assert_eq!(partial.kind(), ErrorKind::PartialInput);
    assert_eq!(partial.code(), "partial_input");
    assert_eq!(
        partial.mutation_effect(),
        MutationEffect::PartiallyApplied {
            completed_prefix: 2
        }
    );
    assert_eq!(partial.retry_safety(), RetrySafety::SameRequestOnly);
}

#[test]
fn test_recording_ownership_child_drop_and_detach() {
    let temporary = TempDir::new().expect("temporary directory");
    let session_dir = temporary.path().join("mock-session");
    fs_err::create_dir_all(&session_dir).expect("session dir");

    let session = setup_session(&session_dir);
    assert_eq!(session.path(), session_dir);

    // Spawn a sleep process
    let child = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");

    let pid = child.id();

    // Verify child is alive
    assert!(kill(Pid::from_raw(pid as i32), None).is_ok());

    // Create owned recording
    let recording = Recording::new(session.clone(), Some(child), temporary.path().to_path_buf());
    assert_eq!(recording.session(), session);
    assert_eq!(recording.evidence_root(), temporary.path());

    // Drop recording -> child should be reaped
    drop(recording);

    // Verify child is dead or gone
    thread::sleep(Duration::from_millis(50));

    let status = kill(Pid::from_raw(pid as i32), None);
    assert!(
        status.is_err(),
        "child process should have been terminated on recording drop"
    );
}

#[test]
fn test_recording_detach_disowns_child() {
    let temporary = TempDir::new().expect("temporary directory");
    let session_dir = temporary.path().join("detached-session");
    fs_err::create_dir_all(&session_dir).expect("session dir");

    let session = setup_session(&session_dir);
    let child = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("spawn sleep");

    let pid = child.id();
    let recording = Recording::new(session.clone(), Some(child), temporary.path().to_path_buf());
    let detached: DetachedRecording = recording.detach().expect("detach");
    assert_eq!(detached.session(), session);

    // Verify child is still alive after detach
    assert!(kill(Pid::from_raw(pid as i32), None).is_ok());

    // Clean up detached process
    let _ = kill(Pid::from_raw(pid as i32), Signal::SIGKILL);
}

#[test]
fn test_operation_handle_lifecycle() {
    let op: Operation<i32> = Operation::running("job-123", "processing artifacts");
    assert_eq!(op.id(), "job-123");
    assert_eq!(
        op.status(),
        &OperationStatus::Running {
            percent: None,
            message: "processing artifacts".to_owned()
        }
    );

    let completed_op = Operation::completed("job-456", 42);
    assert_eq!(completed_op.status(), &OperationStatus::Completed);
    assert_eq!(completed_op.into_result(), Some(42));
}

#[test]
fn test_blocking_context_execution_and_nested_runtime_rejection() {
    use termtrace_service::blocking::BlockingContext;

    // Normal synchronous caller can run an async future
    let value = BlockingContext::block_on(async {
        return 123 + 456;
    })
    .expect("block_on should succeed outside async runtime");
    assert_eq!(value, 579);

    // Inside an async runtime, block_on must reject with typed error rather than panicking
    let runtime = TokioBuilder::new_current_thread()
        .enable_all()
        .build()
        .expect("current thread runtime");

    let rejection = runtime.block_on(async {
        return BlockingContext::block_on(async {
            return "nested";
        });
    });
    assert!(rejection.is_err());

    let err = rejection.unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidOperation);
    assert!(err.to_string().contains("async runtime context"));
}
