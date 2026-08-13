//! Presented-screen predicates, stable captures, and asynchronous waiters.

use std::{
    result::Result as StdResult,
    sync::mpsc,
    time::{Duration, Instant},
};

use uuid::Uuid;
use zstd::stream::encode_all;

use crate::{
    Result,
    control::{
        CONTROL_PROTOCOL_VERSION, ControlData, ControlErrorDetails, ControlResponse, ControlResult,
        ScreenMode, ScreenPredicate, ScreenRegion, ScreenWaitQuality, StableCaptureQuality,
        StateHashComparison,
    },
    event::ByteString,
    event_log::EventLogWriter,
    ghostty_vt::SemanticState,
    observation::CaptureQuality,
    replay::semantic_state_hash,
    screen_frame::{ScreenFrameSource, ScreenProjectionRequest, project_screen_frame},
};

use super::super::{
    presentation::{LiveTerminal, semantic_viewport_text},
    support::{actor_boundaries, control_error_response, terminal_error},
};
use super::commands::{screen_frame_error_code, screen_frame_failure_details};

pub(super) const MAX_SCREEN_WAITERS: usize = 128;

pub(in crate::record) struct ScreenWaiter {
    pub(super) request_id: Uuid,
    pub(super) predicate: Option<ScreenPredicate>,
    pub(super) stable_for: Duration,
    pub(super) deadline: Instant,
    pub(super) projection: ScreenProjectionRequest,
    pub(super) matching_state: Option<(String, Instant, u64)>,
    pub(super) response: mpsc::SyncSender<ControlResponse>,
}

pub(super) fn presented_state_hash(live: &LiveTerminal) -> Result<String> {
    return semantic_state_hash(&live.presented);
}

pub(super) fn valid_blake3_hash(hash: &str) -> bool {
    return hash.len() == 64
        && hash
            .as_bytes()
            .iter()
            .all(|byte| return byte.is_ascii_digit() || (b'a'..=b'f').contains(byte));
}

pub(in crate::record) fn evaluate_screen_waiters(
    session_id: Uuid,
    event_log: &EventLogWriter,
    live: &LiveTerminal,
    waiters: &mut Vec<ScreenWaiter>,
    child_completed: bool,
    output_closed: bool,
) -> Result<()> {
    if waiters.is_empty() {
        return Ok(());
    }
    let state_hash = semantic_state_hash(&live.presented)?;
    let viewport_text = semantic_viewport_text(&live.presented);
    let observed_monotonic_ns = event_log.elapsed_ns();
    let now = Instant::now();
    let mut index = 0;
    while index < waiters.len() {
        let Some(waiter) = waiters.get_mut(index) else {
            break;
        };
        let matched = waiter.predicate.as_ref().is_none_or(|predicate| {
            return evaluate_screen_predicate(
                predicate,
                &live.presented,
                &viewport_text,
                &state_hash,
            );
        });
        if matched {
            match &waiter.matching_state {
                Some((prior_hash, _, _)) if prior_hash == &state_hash => {}
                _ => {
                    waiter.matching_state = Some((state_hash.clone(), now, observed_monotonic_ns));
                }
            }
        } else {
            waiter.matching_state = None;
        }
        let stable = waiter
            .matching_state
            .as_ref()
            .is_some_and(|(_, since, _)| return since.elapsed() >= waiter.stable_for);
        let deadline = now >= waiter.deadline;
        let finished = stable || child_completed || output_closed || deadline;
        if !finished {
            index += 1;
            continue;
        }

        let waiter = waiters.remove(index);
        let boundaries = actor_boundaries(event_log);
        let capture_quality = if stable {
            CaptureQuality::Stable
        } else if deadline {
            CaptureQuality::Incomplete
        } else {
            CaptureQuality::Live
        };
        let source = ScreenFrameSource::Actor {
            session_id,
            boundaries: boundaries.clone(),
            observed_monotonic_ns,
        };
        let frame = match project_screen_frame(
            &live.presented,
            &waiter.projection,
            source,
            true,
            capture_quality,
        ) {
            Ok(frame) => frame,
            Err(error) => {
                let response = ControlResponse {
                    version: CONTROL_PROTOCOL_VERSION,
                    request_id: waiter.request_id,
                    result: ControlResult::Error {
                        code: screen_frame_error_code(&error).to_owned(),
                        message: error.to_string(),
                        details: Some(ControlErrorDetails {
                            current_presented_state_hash: state_hash.clone(),
                            boundaries,
                            scope: "screen_wait_terminating_frame".to_owned(),
                            input_sequence: None,
                            screen_frame: Some(screen_frame_failure_details(&error)),
                        }),
                    },
                };
                let _ = waiter.response.send(response);
                continue;
            }
        };
        let data = if let Some(predicate) = waiter.predicate {
            let quality = if stable && waiter.stable_for.is_zero() {
                ScreenWaitQuality::MatchedUnsettled
            } else if stable {
                ScreenWaitQuality::Matched
            } else if child_completed {
                ScreenWaitQuality::ChildCompleted
            } else if output_closed {
                ScreenWaitQuality::OutputClosed
            } else {
                ScreenWaitQuality::Deadline
            };
            ControlData::ScreenWait {
                predicate,
                matched,
                first_match_monotonic_ns: waiter
                    .matching_state
                    .as_ref()
                    .map(|(_, _, value)| return *value),
                observed_monotonic_ns,
                stable_for_ns: u64::try_from(waiter.stable_for.as_nanos()).unwrap_or(u64::MAX),
                quality,
                frame,
            }
        } else {
            let quality = if stable {
                StableCaptureQuality::Stable
            } else if child_completed {
                StableCaptureQuality::ChildCompleted
            } else if output_closed {
                StableCaptureQuality::OutputClosed
            } else {
                StableCaptureQuality::Deadline
            };
            let encoded = serde_json::to_vec(&live.presented).map_err(|error| {
                return terminal_error("serialize presented state", error.to_string());
            })?;
            let compressed = encode_all(encoded.as_slice(), 3).map_err(|error| {
                return terminal_error("compress presented state", error.to_string());
            })?;
            ControlData::StableSnapshot {
                frame,
                semantic_state_zstd: ByteString::from(compressed),

                first_observed_monotonic_ns: waiter
                    .matching_state
                    .as_ref()
                    .map_or(observed_monotonic_ns, |(_, _, value)| return *value),
                observed_monotonic_ns,
                stable_for_ns: u64::try_from(waiter.stable_for.as_nanos()).unwrap_or(u64::MAX),
                quality,
            }
        };
        let response = ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: waiter.request_id,
            result: ControlResult::Ok { data },
        };
        let _ = waiter.response.send(response);
    }
    return Ok(());
}

pub(in crate::record) fn fail_screen_waiters(
    waiters: &mut Vec<ScreenWaiter>,
    code: &str,
    message: &str,
) {
    for waiter in waiters.drain(..) {
        let response = control_error_response(waiter.request_id, code, message);
        let _ = waiter.response.send(response);
    }
}

pub(super) fn validate_screen_predicate(predicate: &ScreenPredicate) -> StdResult<(), String> {
    fn visit(
        predicate: &ScreenPredicate,
        depth: usize,
        nodes: &mut usize,
    ) -> StdResult<(), String> {
        *nodes = nodes.saturating_add(1);
        if depth > 4 || *nodes > 32 {
            return Err("screen predicate composition is too deep or large".to_owned());
        }
        match predicate {
            ScreenPredicate::Contains { text }
            | ScreenPredicate::Absent { text }
            | ScreenPredicate::RowContains { text, .. }
            | ScreenPredicate::RowAbsent { text, .. } => {
                if text.is_empty() || text.len() > 16 * 1024 {
                    return Err("screen predicate text must contain 1-16384 bytes".to_owned());
                }
            }
            ScreenPredicate::MatchesRegex { pattern }
            | ScreenPredicate::ExcludesRegex { pattern } => {
                if pattern.is_empty() || pattern.len() > 1024 {
                    return Err("screen regular expression must contain 1-1024 bytes".to_owned());
                }
                regex::RegexBuilder::new(pattern)
                    .size_limit(1024 * 1024)
                    .build()
                    .map_err(|error| return format!("invalid bounded screen regex: {error}"))?;
            }
            ScreenPredicate::RegionContains { region, text }
            | ScreenPredicate::RegionAbsent { region, text } => {
                if text.is_empty() || text.len() > 16 * 1024 {
                    return Err("screen predicate text must contain 1-16384 bytes".to_owned());
                }
                if region.left >= region.right || region.top >= region.bottom {
                    return Err("screen region must be non-empty and half-open".to_owned());
                }
            }
            ScreenPredicate::Cursor {
                column,
                row,
                visible,
                shape,
            } => {
                if column.is_none() && row.is_none() && visible.is_none() && shape.is_none() {
                    return Err("cursor predicate must constrain at least one property".to_owned());
                }
            }
            ScreenPredicate::StateHash { hash, .. } => {
                if !valid_blake3_hash(hash) {
                    return Err("state predicate requires a lowercase BLAKE3 hash".to_owned());
                }
            }
            ScreenPredicate::All { predicates } | ScreenPredicate::Any { predicates } => {
                if predicates.is_empty() || predicates.len() > 16 {
                    return Err("predicate composition must contain 1-16 children".to_owned());
                }
                for child in predicates {
                    visit(child, depth + 1, nodes)?;
                }
            }
            ScreenPredicate::Mode { .. } => {}
        }
        return Ok(());
    }

    let mut nodes = 0;
    return visit(predicate, 1, &mut nodes);
}

pub(super) fn evaluate_screen_predicate(
    predicate: &ScreenPredicate,
    state: &SemanticState,
    viewport_text: &str,
    state_hash: &str,
) -> bool {
    return match predicate {
        ScreenPredicate::Contains { text } => viewport_text.contains(text),
        ScreenPredicate::Absent { text } => !viewport_text.contains(text),
        ScreenPredicate::MatchesRegex { pattern } => {
            regex::Regex::new(pattern).is_ok_and(|regex| return regex.is_match(viewport_text))
        }
        ScreenPredicate::ExcludesRegex { pattern } => {
            regex::Regex::new(pattern).is_ok_and(|regex| return !regex.is_match(viewport_text))
        }
        ScreenPredicate::RowContains { row, text } => {
            screen_row_text(state, *row).is_some_and(|value| return value.contains(text))
        }
        ScreenPredicate::RowAbsent { row, text } => {
            screen_row_text(state, *row).is_some_and(|value| return !value.contains(text))
        }
        ScreenPredicate::RegionContains { region, text } => {
            screen_region_text(state, *region).is_some_and(|value| return value.contains(text))
        }
        ScreenPredicate::RegionAbsent { region, text } => {
            screen_region_text(state, *region).is_some_and(|value| return !value.contains(text))
        }
        ScreenPredicate::Cursor {
            column,
            row,
            visible,
            shape,
        } => {
            column.is_none_or(|value| return state.cursor.column == Some(value))
                && row.is_none_or(|value| return state.cursor.row == Some(value))
                && visible.is_none_or(|value| return state.cursor.visible == value)
                && shape.is_none_or(|value| return state.cursor.style == value)
        }
        ScreenPredicate::Mode { mode, enabled } => screen_mode(state, *mode) == *enabled,
        ScreenPredicate::StateHash { hash, comparison } => match comparison {
            StateHashComparison::Unchanged => state_hash == hash,
            StateHashComparison::Changed => state_hash != hash,
        },
        ScreenPredicate::All { predicates } => predicates.iter().all(|child| {
            return evaluate_screen_predicate(child, state, viewport_text, state_hash);
        }),
        ScreenPredicate::Any { predicates } => predicates.iter().any(|child| {
            return evaluate_screen_predicate(child, state, viewport_text, state_hash);
        }),
    };
}

fn screen_row_text(state: &SemanticState, row: u16) -> Option<String> {
    let row = state.grid.get(usize::from(row))?;
    let mut text = String::new();
    for cell in &row.cells {
        text.push_str(if cell.grapheme.is_empty() {
            " "
        } else {
            &cell.grapheme
        });
    }
    return Some(text.trim_end().to_owned());
}

fn screen_region_text(state: &SemanticState, region: ScreenRegion) -> Option<String> {
    if region.right > state.cols || region.bottom > state.rows {
        return None;
    }
    let mut text = String::new();
    for row_index in region.top..region.bottom {
        let row = state.grid.get(usize::from(row_index))?;
        for column in region.left..region.right {
            let cell = row.cells.get(usize::from(column))?;
            text.push_str(if cell.grapheme.is_empty() {
                " "
            } else {
                &cell.grapheme
            });
        }
        while text.ends_with(' ') {
            text.pop();
        }
        if row_index + 1 < region.bottom {
            text.push('\n');
        }
    }
    return Some(text);
}

fn screen_mode(state: &SemanticState, mode: ScreenMode) -> bool {
    return match mode {
        ScreenMode::AlternateScreen => state.modes.alternate_screen,
        ScreenMode::BracketedPaste => state.modes.bracketed_paste,
        ScreenMode::Wraparound => state.modes.wraparound,
        ScreenMode::SynchronizedOutput => state.modes.synchronized_output,
        ScreenMode::Ansi { number } => state
            .modes
            .values
            .iter()
            .find(|mode| return mode.ansi && mode.number == number)
            .is_some_and(|mode| return mode.enabled),
        ScreenMode::Dec { number } => state
            .modes
            .values
            .iter()
            .find(|mode| return !mode.ansi && mode.number == number)
            .is_some_and(|mode| return mode.enabled),
    };
}
