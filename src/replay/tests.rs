use uuid::Uuid;

use super::*;
use crate::event::{ByteString, EventLogHeader, EventRecord};

fn output_log(data: &[u8]) -> EventLog {
    return EventLog {
        header: EventLogHeader::new(
            Uuid::now_v7(),
            "0".repeat(64),
            0,
            80,
            24,
            800,
            480,
            "xterm-ghostty",
        ),
        events: vec![EventRecord {
            seq: 0,
            monotonic_ns: 1,
            prev_hash: "0".repeat(64),
            hash: "1".repeat(64),
            event: Event::Output {
                data: ByteString::from(data),
            },
        }],
        pending_operations: Vec::new(),
    };
}

#[test]
fn replay_enforces_output_and_capture_work_budgets() {
    let log = output_log(b"abc");
    let output_error = replay_with_limits(
        &log,
        None,
        ReplayLimits {
            output_bytes: 2,
            ..DEFAULT_REPLAY_LIMITS
        },
        |_, _, _| return Ok(()),
    )
    .expect_err("output budget must reject before byte replay");
    assert!(output_error.to_string().contains("output-byte limit"));

    let capture_error = replay_with_limits(
        &log,
        None,
        ReplayLimits {
            capture_attempts: 1,
            ..DEFAULT_REPLAY_LIMITS
        },
        |_, _, _| return Ok(()),
    )
    .expect_err("capture budget must reject byte-granular amplification");
    assert!(capture_error.to_string().contains("capture-attempt limit"));

    let cell_work_error = replay_with_limits(
        &log,
        None,
        ReplayLimits {
            cell_work: 1_919,
            ..DEFAULT_REPLAY_LIMITS
        },
        |_, _, _| return Ok(()),
    )
    .expect_err("cell work must be charged before snapshot capture");
    assert!(cell_work_error.to_string().contains("cell-work limit"));
}
