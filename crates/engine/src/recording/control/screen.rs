//! Asynchronous presented-screen waiters and stable captures.

use super::super::{
    actor::SessionActor,
    presentation::LiveTerminal,
    support::{actor_boundaries, control_error_response, deliver_response, terminal_error},
};
use super::commands::screen_frame_error;
use crate::Result;
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use termtrace_model::{
    protocol::control::{
        CompiledScreenPredicate, ControlPayload, ControlResponse, ControlResult, ScreenWaitQuality,
        StableCaptureQuality, CONTROL_PROTOCOL_VERSION,
    },
    protocol::mutation::MutationEffect,
    terminal::observation::CaptureQuality,
    terminal::screen::{ScreenFrameSource, ScreenProjectionRequest},
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
    pub(super) reaction: Option<WaiterReaction>,
    pub(super) response: mpsc::SyncSender<ControlResponse>,
}

pub(in crate::recording) struct WaiterReaction {
    pub(super) changed_from: String,
    pub(super) output_bytes: u64,
    pub(super) deadline: Instant,
}

pub(super) fn reaction_window(stable_for_ns: u64, reaction_timeout_ns: u64) -> Duration {
    return Duration::from_nanos(reaction_timeout_ns.max(stable_for_ns));
}

pub(super) fn presented_state_hash(live: &LiveTerminal) -> Result<String> {
    return live.presented_state_hash();
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

            let output_bytes = self.progress.output_bytes;
            let reacted = waiter.matching_state.is_some()
                || waiter.reaction.as_ref().is_none_or(|reaction| {
                    return reaction.changed_from != state_hash
                        || output_bytes > reaction.output_bytes;
                });

            let unchanged = !reacted
                && waiter
                    .reaction
                    .as_ref()
                    .is_some_and(|reaction| return now >= reaction.deadline);

            let matched = reacted
                && waiter.predicate.as_ref().is_none_or(|predicate| {
                    let text = if predicate.needs_viewport_text() {
                        viewport_text
                            .get_or_insert_with(|| {
                                return semantic_viewport_text(&self.live.presented);
                            })
                            .as_str()
                    } else {
                        ""
                    };

                    return predicate.matches(&self.live.presented, text, &state_hash);
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
            let finished = stable || unchanged || child_completed || output_closed || deadline;

            if !finished {
                index += 1;
                continue;
            }

            let waiter = self.screen_waiters.remove(index);
            let boundaries = actor_boundaries(&self.event_log);
            let capture_quality = if stable || unchanged {
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
                    let response = screen_frame_error(
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

                ControlPayload::ScreenWait {
                    predicate: predicate.into_predicate(),
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
                } else if !reacted {
                    StableCaptureQuality::Unchanged
                } else {
                    StableCaptureQuality::Deadline
                };

                let encoded = serde_json::to_vec(&self.live.presented).map_err(|error| {
                    return terminal_error("serialize presented state", error.to_string());
                })?;
                let compressed = encode_all(encoded.as_slice(), 3).map_err(|error| {
                    return terminal_error("compress presented state", error.to_string());
                })?;

                ControlPayload::StableSnapshot {
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
