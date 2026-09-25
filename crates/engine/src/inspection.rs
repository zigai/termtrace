//! Save-independent state inspection and bounded evidence waits.

use crate::completion;
use crate::replay::{inspect_prefix_stream, inspect_session_stream, ValidatedSession};
use crate::{Error, Result};
use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};
use termtrace_evidence::{
    event_log::{EventLogCursor, EventLogSummary},
    session::validate_completed_session,
};
pub use termtrace_model::terminal::inspection::WaitOutcome;
use termtrace_model::{
    execution::event::Event,
    execution::layout::SessionLayout,
    execution::session::SessionCompletion,
    terminal::replay::{StateInspection, StateSelector},
};

/// Reconstruct one complete semantic state directly from canonical evidence.
pub fn inspect_session(
    session_directory: impl AsRef<Path>,
    selector: StateSelector,
) -> Result<StateInspection> {
    let session = session_directory.as_ref();
    if completion::published(session)?.is_some() {
        let validated = ValidatedSession::open(session)?;
        let inspection = inspect_session_stream(session, selector, None)?;
        validated.check()?;

        return Ok(inspection);
    }

    EventLogSummary::read(SessionLayout::new(session).event_log())?;

    return inspect_prefix_stream(session, selector);
}

/// Wait until exact bytes appear in the bounded canonical output stream.
pub fn wait_for_output(
    session_directory: impl AsRef<Path>,
    needle: &[u8],
    timeout: Duration,
) -> Result<WaitOutcome> {
    if needle.is_empty() {
        return Err(wait_error("wait condition must not be empty"));
    }

    let session = session_directory.as_ref();
    let event_path = SessionLayout::new(session).event_log();
    let completion_path = SessionLayout::new(session).completion();
    let deadline = Instant::now() + timeout;
    let mut cursor = EventLogCursor::open(&event_path)?;
    let mut matcher = OutputMatcher::new(needle);

    loop {
        while let Some(record) = cursor.next_record()? {
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
        }

        if completion_path.is_file() {
            if let Some(line) = cursor.incomplete_line() {
                return Err(Error::InvalidEventLog {
                    path: event_path,
                    line,
                    reason: "truncated line without newline terminator".to_owned(),
                });
            }
            return Err(wait_error("session completed before output matched"));
        }

        let now = Instant::now();
        if now >= deadline {
            return Err(wait_error("timed out before output matched"));
        }

        thread::sleep(Duration::from_millis(50).min(deadline.saturating_duration_since(now)));
    }
}

/// Wait until completion is published and verify it against canonical events.
pub fn wait_for_completion(
    session_directory: impl AsRef<Path>,
    timeout: Duration,
) -> Result<SessionCompletion> {
    let session = session_directory.as_ref();
    let completion_path = SessionLayout::new(session).completion();
    let deadline = Instant::now() + timeout;

    loop {
        if completion_path.is_file() {
            return Ok(validate_completed_session(session)?);
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
    #[expect(
        clippy::indexing_slicing,
        reason = "needle is non-empty and matched < needle.len()"
    )]
    fn new(needle: &'needle [u8]) -> Self {
        let mut prefix_lengths: Vec<usize> = vec![0; needle.len()];

        let mut matched = 0;
        for index in 1..needle.len() {
            let byte = needle[index];
            while matched > 0 && needle[matched] != byte {
                matched = prefix_lengths[matched - 1];
            }

            if needle[matched] == byte {
                matched += 1;
            }

            prefix_lengths[index] = matched;
        }

        return Self {
            needle,
            prefix_lengths,
            matched: 0,
            output_offset: 0,
        };
    }

    #[expect(
        clippy::indexing_slicing,
        reason = "self.matched < self.needle.len() and output_offset is bounded by event data"
    )]
    fn push(&mut self, bytes: &[u8]) -> Option<u64> {
        for &byte in bytes {
            while self.matched > 0 && self.needle[self.matched] != byte {
                self.matched = self.prefix_lengths[self.matched - 1];
            }

            if self.needle[self.matched] == byte {
                self.matched += 1;
            }

            self.output_offset = self.output_offset.saturating_add(1);

            if self.matched == self.needle.len() {
                return Some(self.output_offset);
            }
        }

        return None;
    }
}

fn wait_error(message: impl Into<String>) -> Error {
    return Error::ControlRejected {
        code: "wait_failed".to_owned(),
        message: message.into(),
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
