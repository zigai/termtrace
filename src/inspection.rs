//! Agent-facing state inspection and bounded live waits.

use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    control::{ActorBoundaries, ControlCommand, ControlData, ScreenPredicate, request},
    event::Event,
    event_log::EventLog,
    ghostty_vt::SemanticState,
    replay::{StateInspection, StateSelector, inspect},
    session::{SessionCompletion, validate_completed_log},
};

/// Successful raw-output wait evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WaitOutcome {
    /// Event containing the end of the first observed match.
    pub event_seq: u64,
    /// End offset in the concatenated output stream.
    pub output_end_offset: u64,
    /// Monotonic timestamp of the containing event.
    pub monotonic_ns: u64,
}

/// Successful presented-screen wait evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenWaitOutcome {
    /// Predicate that matched.
    pub predicate: ScreenPredicate,
    /// Observation scope, fixed to `screen`.
    pub scope: String,
    /// Full presentation-eligible semantic state hash.
    pub state_hash: String,
    /// Whether the observed state was presentation eligible.
    pub presented: bool,
    /// First session-monotonic timestamp at which this unchanged state matched.
    pub first_match_monotonic_ns: u64,
    /// Last session-monotonic timestamp through which the state remained stable.
    pub stable_through_monotonic_ns: u64,
    /// Requested stability duration.
    pub stable_for_ns: u64,
    /// Actor-consistent canonical source boundaries.
    pub boundaries: ActorBoundaries,
    /// Whether completion was present when the result was returned.
    pub child_completed: bool,
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
    /// `stable` or `matched_unsettled`.
    pub completion_quality: String,
}

/// Wait for an exact predicate on the latest presentation-eligible viewport.
///
/// # Errors
///
/// Returns an error when the predicate is empty, the actor becomes unavailable,
/// the child completes before a match, or the monotonic deadline expires.
pub fn wait_for_screen(
    session_directory: impl AsRef<Path>,
    predicate: ScreenPredicate,
    stable_for: Duration,
    timeout: Duration,
) -> Result<ScreenWaitOutcome> {
    let stable_for_ns = u64::try_from(stable_for.as_nanos()).unwrap_or(u64::MAX);
    let timeout_ns = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX);
    let result = request(
        session_directory,
        ControlCommand::WaitScreen {
            predicate: predicate.clone(),
            stable_for_ns,
            timeout_ns,
        },
    )?;
    let ControlData::ScreenWait {
        state_hash,
        first_match_monotonic_ns,
        stable_through_monotonic_ns,
        boundaries,
        cols,
        rows,
        completion_quality,
        child_completed,
        ..
    } = result
    else {
        return Err(wait_error(
            "actor returned an unexpected screen-wait response",
        ));
    };
    return Ok(ScreenWaitOutcome {
        predicate,
        scope: "screen".to_owned(),
        state_hash,
        presented: true,
        first_match_monotonic_ns,
        stable_through_monotonic_ns,
        stable_for_ns,
        boundaries,
        child_completed,
        cols,
        rows,
        completion_quality,
    });
}

/// Reconstruct one complete semantic state from a session.
///
/// # Errors
///
/// Returns an error when the event log is invalid or replay fails.
pub fn inspect_session(
    session_directory: impl AsRef<Path>,
    selector: StateSelector,
) -> Result<StateInspection> {
    let log = EventLog::read(session_directory.as_ref().join("events.jsonl"))?;
    return inspect(&log, selector);
}

/// Render a complete semantic state as compact agent-readable text.
pub fn state_text(state: &SemanticState) -> String {
    let mut output = String::new();
    for (index, row) in state.grid.iter().enumerate() {
        let mut line = String::new();
        for cell in &row.cells {
            if cell.grapheme.is_empty() {
                line.push(' ');
            } else {
                line.push_str(&cell.grapheme);
            }
        }
        output.push_str(line.trim_end());
        if index + 1 < state.grid.len() {
            output.push('\n');
        }
    }
    return output;
}

/// Wait until exact bytes appear in the canonical output stream.
///
/// A status query is issued before each read so buffered output reaches a
/// durable control boundary. Temporary active-writer truncation races are
/// retried; completed invalid logs are still rejected.
///
/// # Errors
///
/// Returns an error when the condition is empty, timeout expires, the child
/// exits first, or the session cannot be validated.
pub fn wait_for_output(
    session_directory: impl AsRef<Path>,
    needle: &[u8],
    timeout: Duration,
) -> Result<WaitOutcome> {
    if needle.is_empty() {
        return Err(wait_error("wait condition must not be empty"));
    }
    let session = session_directory.as_ref();
    let event_path = session.join("events.jsonl");
    let deadline = Instant::now() + timeout;
    loop {
        let control_available = request(session, ControlCommand::Status).is_ok();
        match EventLog::read(&event_path) {
            Ok(log) => {
                if let Some(outcome) = find_output(&log, needle) {
                    return Ok(outcome);
                }
                if log
                    .events
                    .last()
                    .is_some_and(|record| matches!(record.event, Event::Exit { .. }))
                {
                    return Err(wait_error("child exited before output matched"));
                }
            }
            Err(Error::InvalidEventLog { .. }) if control_available => {}
            Err(error) => return Err(error),
        }
        if Instant::now() >= deadline {
            return Err(wait_error("timed out before output matched"));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Wait until a recording publishes completion and verify it against events.
///
/// # Errors
///
/// Returns an error when timeout expires or completion disagrees with the log.
pub fn wait_for_completion(
    session_directory: impl AsRef<Path>,
    timeout: Duration,
) -> Result<SessionCompletion> {
    let session = session_directory.as_ref();
    let event_path = session.join("events.jsonl");
    let completion_path = session.join("completion.json");
    let deadline = Instant::now() + timeout;
    loop {
        if completion_path.is_file() {
            let log = EventLog::read(&event_path)?;
            return validate_completed_log(session, &log);
        }
        if Instant::now() >= deadline {
            return Err(wait_error("timed out before session completion"));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn find_output(log: &EventLog, needle: &[u8]) -> Option<WaitOutcome> {
    let mut output = Vec::new();
    for record in &log.events {
        if let Event::Output { data } = &record.event {
            output.extend_from_slice(data.as_bytes());
            if let Some(offset) = find_subslice(&output, needle) {
                return Some(WaitOutcome {
                    event_seq: record.seq,
                    output_end_offset: u64::try_from(offset + needle.len()).unwrap_or(u64::MAX),
                    monotonic_ns: record.monotonic_ns,
                });
            }
        }
    }
    return None;
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    return haystack
        .windows(needle.len())
        .position(|window| return window == needle);
}

fn wait_error(message: &str) -> Error {
    return Error::ControlRejected {
        code: "wait_failed".to_owned(),
        message: message.to_owned(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_matches_across_output_chunks() {
        assert_eq!(find_subslice(b"split across", b"it ac"), Some(3));
        assert_eq!(find_subslice(b"missing", b"other"), None);
    }
}
