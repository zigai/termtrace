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
    control::{ControlCommand, ControlData, ScreenPredicate, ScreenWaitQuality, request},
    event::Event,
    event_log::{EventLog, EventLogCursor},
    ghostty_vt::SemanticState,
    replay::{StateInspection, StateSelector, inspect},
    screen_frame::{ScreenFrame, ScreenProjectionRequest},
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

/// Exact presented-screen wait evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ScreenWaitOutcome {
    /// Predicate evaluated by the actor.
    pub predicate: ScreenPredicate,
    /// Whether the predicate matched before termination.
    pub matched: bool,
    /// First matching actor timestamp, when matched.
    pub first_match_monotonic_ns: Option<u64>,
    /// Actor timestamp of the returned frame.
    pub observed_monotonic_ns: u64,
    /// Requested stability duration.
    pub stable_for_ns: u64,
    /// Closed completion classification.
    pub quality: ScreenWaitQuality,
    /// Exact satisfying or terminating frame.
    pub frame: ScreenFrame,
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
            projection: ScreenProjectionRequest::default(),
        },
    )?;
    let ControlData::ScreenWait {
        matched,
        first_match_monotonic_ns,
        observed_monotonic_ns,
        stable_for_ns,
        quality,
        frame,
        ..
    } = result
    else {
        return Err(wait_error(
            "actor returned an unexpected screen-wait response",
        ));
    };
    return Ok(ScreenWaitOutcome {
        predicate,
        matched,
        first_match_monotonic_ns,
        observed_monotonic_ns,
        stable_for_ns,
        quality,
        frame,
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
    let mut cursor = EventLogCursor::open(&event_path)?;
    let mut matcher = OutputMatcher::new(needle);
    loop {
        let control_available = request(session, ControlCommand::Status).is_ok();
        loop {
            let Some(record) = cursor.next_record()? else {
                if let Some(line) = cursor.incomplete_line()
                    && !control_available
                {
                    return Err(Error::InvalidEventLog {
                        path: event_path,
                        line,
                        reason: "truncated line without newline terminator".to_owned(),
                    });
                }
                break;
            };
            if let Event::Output { data } = &record.event
                && let Some(output_end_offset) = matcher.push(data.as_bytes())
            {
                return Ok(WaitOutcome {
                    event_seq: record.seq,
                    output_end_offset,
                    monotonic_ns: record.monotonic_ns,
                });
            }
            if matches!(record.event, Event::Exit { .. }) {
                return Err(wait_error("child exited before output matched"));
            }
            if Instant::now() >= deadline {
                return Err(wait_error("timed out before output matched"));
            }
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(wait_error("timed out before output matched"));
        }
        thread::sleep(Duration::from_millis(50).min(deadline.saturating_duration_since(now)));
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

struct OutputMatcher<'needle> {
    needle: &'needle [u8],
    prefix_lengths: Vec<usize>,
    matched: usize,
    output_offset: u64,
}

impl<'needle> OutputMatcher<'needle> {
    fn new(needle: &'needle [u8]) -> Self {
        let mut prefix_lengths: Vec<usize> = vec![0; needle.len()];
        let mut matched = 0;
        for index in 1..needle.len() {
            let Some(byte) = needle.get(index).copied() else {
                break;
            };
            while matched > 0 && needle.get(matched).copied() != Some(byte) {
                matched = prefix_lengths
                    .get(matched.saturating_sub(1))
                    .copied()
                    .unwrap_or(0);
            }
            if needle.get(matched).copied() == Some(byte) {
                matched = matched.saturating_add(1);
            }
            if let Some(length) = prefix_lengths.get_mut(index) {
                *length = matched;
            }
        }
        return Self {
            needle,
            prefix_lengths,
            matched: 0,
            output_offset: 0,
        };
    }

    fn push(&mut self, bytes: &[u8]) -> Option<u64> {
        for byte in bytes {
            while self.matched > 0 && self.needle.get(self.matched) != Some(byte) {
                self.matched = self
                    .prefix_lengths
                    .get(self.matched.saturating_sub(1))
                    .copied()
                    .unwrap_or(0);
            }
            if self.needle.get(self.matched) == Some(byte) {
                self.matched = self.matched.saturating_add(1);
            }
            self.output_offset = self.output_offset.saturating_add(1);
            if self.matched == self.needle.len() {
                return Some(self.output_offset);
            }
        }
        return None;
    }
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
        let mut matching = OutputMatcher::new(b"it ac");
        assert_eq!(matching.push(b"split "), None);
        assert_eq!(matching.push(b"across"), Some(8));

        let mut missing = OutputMatcher::new(b"other");
        assert_eq!(missing.push(b"missing"), None);
    }
}
