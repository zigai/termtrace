//! Contract and fault tests for ownership, lifecycle, context, and error classifications.

use nix::{
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use std::{process::Command, thread, time::Duration};
use tempfile::TempDir;
use termtrace_model::{
    execution::event_log::CanonicalBoundary,
    protocol::control::InputSequenceFailureDetails,
    terminal::observation::{ActorBoundaries, StatePoint},
};
use termtrace_service::{
    context::OperationContext,
    error::{Error, ErrorKind, MutationEffect, RetrySafety},
    recording::{DetachedRecording, Recording},
    session::Session,
    types::{Operation, OperationStatus, SessionId, StateSelector},
};
use tokio::runtime::Builder as TokioBuilder;

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

    let session = Session::from_directory(&session_dir).expect("session");
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

    let session = Session::from_directory(&session_dir).expect("session");
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
