//! Presented-screen predicates, stable captures, and asynchronous waiters.

use super::super::{
    actor::SessionActor,
    presentation::LiveTerminal,
    support::{actor_boundaries, control_error_response, deliver_response, terminal_error},
};
use super::commands::screen_frame_error_response;
use crate::Result;
use std::{
    collections::BTreeMap,
    result::Result as StdResult,
    sync::mpsc,
    time::{Duration, Instant},
};
use termtrace_model::{
    protocol::control::{
        ControlData, ControlResponse, ControlResult, ScreenMode, ScreenPredicate,
        ScreenWaitQuality, StableCaptureQuality, StateHashComparison, CONTROL_PROTOCOL_VERSION,
    },
    protocol::mutation::MutationEffect,
    terminal::observation::CaptureQuality,
    terminal::screen::{ScreenFrameSource, ScreenProjectionRequest, ScreenRegion},
    terminal::state::SemanticState,
    value::ByteString,
};
use termtrace_terminal::{project_screen_frame, semantic_viewport_text};
use uuid::Uuid;
use zstd::stream::encode_all;

pub(super) const MAX_SCREEN_WAITERS: usize = 128;

pub(in crate::recording) struct ScreenWaiter {
    pub(super) request_id: Uuid,
    pub(super) predicate: Option<CompiledScreenPredicate>,
    pub(super) stable_for: Duration,
    pub(super) deadline: Instant,
    pub(super) projection: ScreenProjectionRequest,
    pub(super) matching_state: Option<(String, Instant, u64)>,
    pub(super) response: mpsc::SyncSender<ControlResponse>,
}

pub(super) fn presented_state_hash(live: &LiveTerminal) -> Result<String> {
    return live.presented_state_hash();
}

pub(super) fn valid_blake3_hash(hash: &str) -> bool {
    return hash.len() == 64
        && hash
            .as_bytes()
            .iter()
            .all(|byte| return byte.is_ascii_digit() || (b'a'..=b'f').contains(byte));
}

fn predicate_needs_viewport_text(predicate: &ScreenPredicate) -> bool {
    match predicate {
        ScreenPredicate::Contains { .. }
        | ScreenPredicate::Absent { .. }
        | ScreenPredicate::MatchesRegex { .. }
        | ScreenPredicate::ExcludesRegex { .. } => return true,
        ScreenPredicate::All { predicates } | ScreenPredicate::Any { predicates } => {
            return predicates.iter().any(predicate_needs_viewport_text);
        }
        _ => return false,
    }
}

impl SessionActor<'_> {
    pub(in crate::recording) fn evaluate_screen_waiters(
        &mut self,
        child_exited: bool,
        output_closed: bool,
    ) -> Result<()> {
        if self.screen_waiters.is_empty() {
            return Ok(());
        }

        let state_hash = self.live.presented_state_hash()?;
        let mut viewport_text: Option<String> = None;
        let observed_monotonic_ns = self.event_log.elapsed_ns();
        let now = Instant::now();

        let mut index = 0;
        while index < self.screen_waiters.len() {
            let Some(waiter) = self.screen_waiters.get_mut(index) else {
                break;
            };
            let matched = waiter.predicate.as_ref().is_none_or(|predicate| {
                let text = if predicate_needs_viewport_text(&predicate.predicate) {
                    viewport_text
                        .get_or_insert_with(|| return semantic_viewport_text(&self.live.presented))
                        .as_str()
                } else {
                    ""
                };

                return evaluate_screen_predicate(
                    predicate,
                    &self.live.presented,
                    text,
                    &state_hash,
                );
            });

            if matched {
                match &waiter.matching_state {
                    Some((prior_hash, _, _)) if prior_hash == &state_hash => {}
                    _ => {
                        waiter.matching_state =
                            Some((state_hash.clone(), now, observed_monotonic_ns));
                    }
                }
            } else {
                waiter.matching_state = None;
            }

            let stable = waiter
                .matching_state
                .as_ref()
                .is_some_and(|(_, since, _)| return since.elapsed() >= waiter.stable_for);

            let child_completed = child_exited && output_closed;
            let deadline = now >= waiter.deadline;
            let finished = stable || child_completed || output_closed || deadline;

            if !finished {
                index += 1;
                continue;
            }

            let waiter = self.screen_waiters.remove(index);
            let boundaries = actor_boundaries(&self.event_log);
            let capture_quality = if stable {
                CaptureQuality::Stable
            } else if deadline {
                CaptureQuality::Incomplete
            } else {
                CaptureQuality::Live
            };

            let source = ScreenFrameSource::Actor {
                session_id: self.session_id,
                boundaries: boundaries.clone(),
                observed_monotonic_ns,
            };
            let frame = match project_screen_frame(
                &self.live.presented,
                &waiter.projection,
                source,
                true,
                capture_quality,
            ) {
                Ok(frame) => frame,
                Err(error) => {
                    let response = screen_frame_error_response(
                        waiter.request_id,
                        &error,
                        state_hash.clone(),
                        &self.event_log,
                        "screen_wait_terminating_frame",
                        MutationEffect::NotApplied,
                    );

                    deliver_response(&waiter.response, response);

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
                    predicate: predicate.predicate,
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

                let encoded = serde_json::to_vec(&self.live.presented).map_err(|error| {
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

            deliver_response(&waiter.response, response);
        }

        return Ok(());
    }
}

impl SessionActor<'_> {
    pub(in crate::recording) fn fail_screen_waiters(&mut self, code: &str, message: &str) {
        for waiter in self.screen_waiters.drain(..) {
            let response = control_error_response(
                waiter.request_id,
                code,
                message,
                MutationEffect::NotApplied,
            );

            deliver_response(&waiter.response, response);
        }
    }
}

/// An admitted predicate with bounded regular expressions compiled once.
pub struct CompiledScreenPredicate {
    predicate: ScreenPredicate,
    regexes: BTreeMap<String, regex::Regex>,
}

/// Validate a bounded screen predicate and retain its compiled expressions.
///
/// # Errors
///
/// Returns a description of the first violation: composition deeper than 4 levels, more than 32
/// nodes, or other than 1-16 children; empty or oversized text or patterns; an invalid regular
/// expression; an empty or inverted region; an unconstrained cursor predicate; or a state hash that
/// is not lowercase BLAKE3 hex.
pub fn compile_screen_predicate(
    predicate: &ScreenPredicate,
) -> StdResult<CompiledScreenPredicate, String> {
    fn visit(
        predicate: &ScreenPredicate,
        depth: usize,
        nodes: &mut usize,
        regexes: &mut BTreeMap<String, regex::Regex>,
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

                if !regexes.contains_key(pattern) {
                    let regex = regex::RegexBuilder::new(pattern)
                        .size_limit(1024 * 1024)
                        .build()
                        .map_err(|error| return format!("invalid bounded screen regex: {error}"))?;
                    regexes.insert(pattern.clone(), regex);
                }
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
                    visit(child, depth + 1, nodes, regexes)?;
                }
            }
            ScreenPredicate::Mode { .. } => {}
        }

        return Ok(());
    }

    let mut nodes = 0;
    let mut regexes = BTreeMap::new();
    visit(predicate, 1, &mut nodes, &mut regexes)?;

    return Ok(CompiledScreenPredicate {
        predicate: predicate.clone(),
        regexes,
    });
}

/// Evaluate a validated predicate against one reconstructed semantic state.
#[must_use]
pub fn evaluate_screen_predicate(
    predicate: &CompiledScreenPredicate,
    state: &SemanticState,
    viewport_text: &str,
    state_hash: &str,
) -> bool {
    return evaluate(
        &predicate.predicate,
        &predicate.regexes,
        state,
        viewport_text,
        state_hash,
    );
}

fn evaluate(
    predicate: &ScreenPredicate,
    regexes: &BTreeMap<String, regex::Regex>,
    state: &SemanticState,
    viewport_text: &str,
    state_hash: &str,
) -> bool {
    return match predicate {
        ScreenPredicate::Contains { text } => viewport_text.contains(text),
        ScreenPredicate::Absent { text } => !viewport_text.contains(text),
        ScreenPredicate::MatchesRegex { pattern } => regexes
            .get(pattern)
            .is_some_and(|regex| return regex.is_match(viewport_text)),
        ScreenPredicate::ExcludesRegex { pattern } => regexes
            .get(pattern)
            .is_some_and(|regex| return !regex.is_match(viewport_text)),
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
            return evaluate(child, regexes, state, viewport_text, state_hash);
        }),
        ScreenPredicate::Any { predicates } => predicates.iter().any(|child| {
            return evaluate(child, regexes, state, viewport_text, state_hash);
        }),
    };
}

fn screen_row_text(state: &SemanticState, row: u16) -> Option<String> {
    let row = state.grid.get(usize::from(row))?;
    let mut text = String::new();

    for cell in &row.cells {
        text.push_str(if cell.grapheme.is_empty() { " " } else { &cell.grapheme });
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
            text.push_str(if cell.grapheme.is_empty() { " " } else { &cell.grapheme });
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
