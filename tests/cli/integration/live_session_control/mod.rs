//! live session control CLI integration tests.

use std::os::unix::net::{SocketAddr, UnixListener};

use crate::support::*;
use termtrace_evidence::session::{read_completion, read_manifest};
use termtrace_model::{
    protocol::control::PointerOperation, terminal::observation::CaptureQuality,
    terminal::state::MouseButton,
};
use termtrace_sdk::locator::LocatorError;
mod capture;
mod concurrency;
mod environment;
mod guards;
mod transport;

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when its recording fixture is invalid"
)]
fn assert_control_events(recording: &EventLog, session: &Path) {
    assert!(recording.events.iter().any(|record| matches!(
        &record.event,
        Event::InputIntent {
            source: InputSource::Agent,
            input_kind: InputKind::Text,
            caller_intent: Some(intent),
            ..
        } if intent == "complete shell read"
    )));
    assert_eq!(
        recording
            .events
            .iter()
            .filter(|record| matches!(
                &record.event,
                Event::Marker { label, .. } if label == "deduplicated"
            ))
            .count(),
        1
    );
    assert!(recording.events.iter().any(|record| matches!(
        record.event,
        Event::ResizeIntent {
            cols: 100,
            rows: 30,
            ..
        }
    )));
    assert!(recording.events.iter().any(|record| matches!(
        &record.event,
        Event::Marker { label, .. } if label == "before-input"
    )));
    assert!(recording.pending_operations.is_empty());

    let resize_operation = recording
        .events
        .iter()
        .find_map(|record| match record.event {
            Event::ResizeIntent {
                operation_id,
                cols: 100,
                rows: 30,
                ..
            } => return Some(operation_id),
            _ => return None,
        })
        .expect("resize intent");
    let resize_result_time = recording
        .events
        .iter()
        .find_map(|record| match record.event {
            Event::ResizeResult {
                operation_id,
                status: OperationStatus::Applied,
                ..
            } if operation_id == resize_operation => return Some(record.monotonic_ns),
            _ => return None,
        })
        .expect("resize result");
    let ranged = index_session_range(session, Some(resize_result_time), None)
        .expect("range crossing resize intent/result");
    let ranged_states = read_zstd_text(&ranged.directory.join("states.jsonl.zst"));
    let ranged_states = ranged_states
        .lines()
        .map(|line| return serde_json::from_str::<StateRecord>(line).expect("state JSON"))
        .collect::<Vec<_>>();
    assert!(
        ranged_states
            .iter()
            .any(|state| return state.delta.geometry == Some((100, 30)))
    );
    assert!(ranged_states.iter().all(|state| {
        let Some(event_sequence) = state.event_seq else {
            return true;
        };
        return recording.events.iter().any(|record| {
            return record.seq == event_sequence
                && matches!(
                    record.event,
                    Event::Output { .. }
                        | Event::ResizeResult {
                            status: OperationStatus::Applied,
                            ..
                        }
                );
        });
    }));
}
