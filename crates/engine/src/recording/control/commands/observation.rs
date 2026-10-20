use super::{
    action, actor_boundaries, control_rejected, encode_all, presented_state_hash,
    project_screen_frame, reaction_window, semantic_viewport_text, terminal_error,
    ActionPostcondition, ActionPostconditionResult, ActorBoundaries, ByteString, CaptureQuality,
    CompiledScreenPredicate, ControlCommand, ControlPayload, Duration, Error, EventLogWriter,
    Instant, LiveTerminal, Result, ScreenFrame, ScreenFrameSource, ScreenPostconditionQuality,
    ScreenProjectionRequest, SessionActor, StableCaptureQuality, Uuid,
};
use nix::time::{clock_gettime, ClockId};
use termtrace_model::protocol::control::{ClockSample, ReactionRequirement};
use termtrace_model::terminal::screen::FrameDetail;

fn system_monotonic_ns() -> Result<u64> {
    let time = clock_gettime(ClockId::CLOCK_MONOTONIC)
        .map_err(|error| return terminal_error("sample monotonic clock", error.to_string()))?;

    return u64::try_from(i128::from(time.tv_sec()) * 1_000_000_000 + i128::from(time.tv_nsec()))
        .map_err(|error| return terminal_error("represent monotonic clock", error.to_string()));
}

fn sample_clock(event_log: &EventLogWriter) -> Result<ClockSample> {
    let before = system_monotonic_ns()?;
    let session_monotonic_ns = event_log.elapsed_ns();
    let after = system_monotonic_ns()?;
    let width = after
        .checked_sub(before)
        .ok_or_else(|| return terminal_error("sample monotonic clock", "clock moved backwards"))?;

    return Ok(ClockSample {
        session_monotonic_ns,
        system_monotonic_ns: before + width / 2,
        uncertainty_ns: width.div_ceil(2),
    });
}

pub(super) enum AdmittedPostcondition {
    None,
    Idle {
        stable_for_ns: u64,
        timeout_ns: u64,
        reaction: ReactionRequirement,
    },
    Screen {
        predicate: CompiledScreenPredicate,
        timeout_ns: u64,
    },
}

pub(super) struct AdmittedAction {
    pub(super) postcondition: AdmittedPostcondition,
    pub(super) projection: ScreenProjectionRequest,
    pub(super) baseline_state_hash: String,
    pub(super) baseline_output_bytes: u64,
}

pub(super) fn admitted_postcondition(
    postcondition: &ActionPostcondition,
    predicate: Option<CompiledScreenPredicate>,
) -> Result<AdmittedPostcondition> {
    return match (postcondition, predicate) {
        (ActionPostcondition::None, _) => Ok(AdmittedPostcondition::None),
        (
            ActionPostcondition::Idle {
                stable_for_ns,
                timeout_ns,
                reaction,
            },
            _,
        ) => Ok(AdmittedPostcondition::Idle {
            stable_for_ns: *stable_for_ns,
            timeout_ns: *timeout_ns,
            reaction: reaction.clone(),
        }),
        (ActionPostcondition::Screen { timeout_ns, .. }, Some(predicate)) => {
            Ok(AdmittedPostcondition::Screen {
                predicate,
                timeout_ns: *timeout_ns,
            })
        }
        (ActionPostcondition::Screen { .. }, None) => Err(terminal_error(
            "admit action postcondition",
            "validated screen postcondition has no compiled predicate",
        )),
    };
}

pub(super) fn evaluate_action_postcondition(
    waiting: &mut action::WaitingAction,
    event_log: &EventLogWriter,
    session_id: Uuid,
    live: &LiveTerminal,
    output_bytes: u64,
    child_exited: bool,
    output_closed: bool,
) -> Result<Option<ActionPostconditionResult>> {
    let now = Instant::now();
    let child_completed = child_exited && output_closed;
    let started_at = waiting.started_at;
    let projection = &waiting.action.projection;

    return match &waiting.action.postcondition {
        AdmittedPostcondition::None => {
            let (frame, state_hash) = if projection.detail == FrameDetail::None {
                (None, Some(presented_state_hash(live)?))
            } else {
                let frame = actor_screen_frame(
                    session_id,
                    event_log,
                    live,
                    actor_boundaries(event_log),
                    projection,
                    CaptureQuality::Live,
                )?;

                let hash = frame.state_hash.clone();
                (Some(frame), Some(hash))
            };

            Ok(Some(ActionPostconditionResult::None {
                matched: true,
                state_hash,
                frame,
            }))
        }
        AdmittedPostcondition::Idle {
            stable_for_ns,
            timeout_ns,
            reaction,
        } => {
            let state_hash = presented_state_hash(live)?;
            let baseline = reaction
                .changed_from
                .as_ref()
                .unwrap_or(&waiting.action.baseline_state_hash);

            let reacted = waiting.matching_state.is_some()
                || &state_hash != baseline
                || output_bytes > waiting.action.baseline_output_bytes;
            match &waiting.matching_state {
                _ if !reacted => {}
                Some((prior_hash, _)) if prior_hash == &state_hash => {}
                _ => waiting.matching_state = Some((state_hash.clone(), now)),
            }

            let stable = waiting.matching_state.as_ref().is_some_and(|(_, since)| {
                return since.elapsed() >= Duration::from_nanos(*stable_for_ns);
            });

            let unchanged = !reacted
                && now >= started_at + reaction_window(*stable_for_ns, reaction.timeout_ns);

            let deadline = now >= started_at + Duration::from_nanos(*timeout_ns);
            if !stable && !unchanged && !child_completed && !output_closed && !deadline {
                return Ok(None);
            }

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

            let settled = quality.is_settled();
            let capture_quality = if settled {
                CaptureQuality::Stable
            } else if deadline {
                CaptureQuality::Incomplete
            } else {
                CaptureQuality::Live
            };

            let (frame, state_hash) = if projection.detail == FrameDetail::None {
                (None, Some(state_hash))
            } else {
                let frame = actor_screen_frame(
                    session_id,
                    event_log,
                    live,
                    actor_boundaries(event_log),
                    projection,
                    capture_quality,
                )?;

                let hash = frame.state_hash.clone();
                (Some(frame), Some(hash))
            };

            Ok(Some(ActionPostconditionResult::Idle {
                matched: settled,
                quality,
                state_hash,
                frame,
            }))
        }
        AdmittedPostcondition::Screen {
            predicate,
            timeout_ns,
        } => {
            let state_hash = presented_state_hash(live)?;
            let viewport_text = semantic_viewport_text(&live.presented);
            let matched = predicate.matches(&live.presented, &viewport_text, &state_hash);
            let deadline = now >= started_at + Duration::from_nanos(*timeout_ns);

            if !matched && !child_completed && !output_closed && !deadline {
                return Ok(None);
            }

            let quality = if matched {
                ScreenPostconditionQuality::MatchedUnsettled
            } else if child_completed {
                ScreenPostconditionQuality::ChildCompleted
            } else if output_closed {
                ScreenPostconditionQuality::OutputClosed
            } else {
                ScreenPostconditionQuality::Deadline
            };

            let capture_quality = if deadline && !matched {
                CaptureQuality::Incomplete
            } else {
                CaptureQuality::Live
            };

            let (frame, state_hash) = if projection.detail == FrameDetail::None {
                (None, Some(state_hash))
            } else {
                let frame = actor_screen_frame(
                    session_id,
                    event_log,
                    live,
                    actor_boundaries(event_log),
                    projection,
                    capture_quality,
                )?;

                let hash = frame.state_hash.clone();
                (Some(frame), Some(hash))
            };

            Ok(Some(ActionPostconditionResult::Screen {
                matched,
                quality,
                state_hash,
                frame,
            }))
        }
    };
}

pub(super) fn actor_screen_frame(
    session_id: Uuid,
    event_log: &EventLogWriter,
    live: &LiveTerminal,
    boundaries: ActorBoundaries,
    projection: &ScreenProjectionRequest,
    capture_quality: CaptureQuality,
) -> Result<ScreenFrame> {
    let source = ScreenFrameSource::Actor {
        session_id,
        boundaries,
        observed_monotonic_ns: event_log.elapsed_ns(),
    };

    return project_screen_frame(&live.presented, projection, source, true, capture_quality)
        .map_err(Error::ScreenFrame);
}

impl SessionActor<'_> {
    pub(super) fn execute_observation_command(
        &self,
        _request_id: Uuid,
        command: &ControlCommand,
    ) -> Option<Result<ControlPayload>> {
        let outcome = match command {
            ControlCommand::Ping => Ok(ControlPayload::Acknowledged {
                boundaries: actor_boundaries(&self.event_log),
            }),
            ControlCommand::EventCursor { next_event_seq } => {
                let (device, inode, header_hash) = self.event_log.source_identity();
                let (start, durable) = self.event_log.follow_cursors(*next_event_seq);

                Ok(ControlPayload::EventCursor {
                    device,
                    inode,
                    header_hash,
                    start,
                    durable,
                })
            }
            ControlCommand::Status => sample_clock(&self.event_log).map(|clock_sample| {
                return ControlPayload::Status {
                    pid: self.spawned.pid,
                    boundaries: actor_boundaries(&self.event_log),
                    clock_sample,
                };
            }),
            ControlCommand::Snapshot => {
                let encoded = match serde_json::to_vec(&self.live.presented) {
                    Ok(encoded) => encoded,
                    Err(error) => {
                        return Some(Err(terminal_error(
                            "serialize presented state",
                            error.to_string(),
                        )));
                    }
                };

                let compressed = match encode_all(encoded.as_slice(), 3) {
                    Ok(compressed) => compressed,
                    Err(error) => {
                        return Some(Err(terminal_error(
                            "compress presented state",
                            error.to_string(),
                        )));
                    }
                };

                let state_hash = match presented_state_hash(&self.live) {
                    Ok(state_hash) => state_hash,
                    Err(error) => return Some(Err(error)),
                };

                Ok(ControlPayload::Snapshot {
                    semantic_state_zstd: ByteString::from(compressed),
                    viewport_text: semantic_viewport_text(&self.live.presented),
                    state_hash,
                    presented: true,
                    cols: self.live.presented.cols,
                    rows: self.live.presented.rows,
                    cursor_column: self.live.presented.cursor.column,
                    cursor_row: self.live.presented.cursor.row,
                    cursor_visible: self.live.presented.cursor.visible,
                    observed_monotonic_ns: self.event_log.elapsed_ns(),
                    modes: self.live.presented.modes.clone(),
                    boundaries: actor_boundaries(&self.event_log),
                })
            }
            ControlCommand::ScreenFrame { projection } => {
                let boundaries = actor_boundaries(&self.event_log);

                actor_screen_frame(
                    self.session_id,
                    &self.event_log,
                    &self.live,
                    boundaries,
                    projection,
                    CaptureQuality::Live,
                )
                .map(|frame| return ControlPayload::ScreenFrame { frame })
            }
            ControlCommand::WaitScreen { .. } | ControlCommand::CaptureStable { .. } => {
                Err(control_rejected(
                    "invalid_screen_wait_dispatch",
                    "screen waits and stable captures must be registered asynchronously",
                ))
            }
            _ => return None,
        };

        return Some(outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_idle_matching_state_transitions() {
        let mut matching_state: Option<(String, Instant)> = None;
        let t0 = Instant::now();
        let hash_a = "state_hash_a".to_owned();

        match &matching_state {
            Some((prior_hash, _)) if prior_hash == &hash_a => {}
            _ => matching_state = Some((hash_a.clone(), t0)),
        }

        assert_eq!(
            matching_state.as_ref().map(|(h, _)| return h.as_str()),
            Some("state_hash_a")
        );
        assert_eq!(matching_state.as_ref().map(|(_, s)| return *s), Some(t0));

        let t1 = t0 + Duration::from_millis(50);
        match &matching_state {
            Some((prior_hash, _)) if prior_hash == &hash_a => {}
            _ => matching_state = Some((hash_a, t1)),
        }

        assert_eq!(matching_state.as_ref().map(|(_, s)| return *s), Some(t0));

        let t2 = t0 + Duration::from_millis(100);
        let hash_b = "state_hash_b".to_owned();

        match &matching_state {
            Some((prior_hash, _)) if prior_hash == &hash_b => {}
            _ => matching_state = Some((hash_b, t2)),
        }

        assert_eq!(
            matching_state.as_ref().map(|(h, _)| return h.as_str()),
            Some("state_hash_b")
        );
        assert_eq!(matching_state.as_ref().map(|(_, s)| return *s), Some(t2));

        let stable_for_ns = 200_000_000u64;
        let since = matching_state
            .as_ref()
            .map(|(_, s)| return *s)
            .expect("present");

        let is_stable_before = (t2 + Duration::from_millis(150)).duration_since(since)
            >= Duration::from_nanos(stable_for_ns);

        assert!(!is_stable_before);

        let is_stable_after = (t2 + Duration::from_millis(250)).duration_since(since)
            >= Duration::from_nanos(stable_for_ns);

        assert!(is_stable_after);
    }
}
