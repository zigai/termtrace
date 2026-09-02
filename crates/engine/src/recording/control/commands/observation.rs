use super::*;

pub(super) fn validate_action_postcondition(postcondition: &ActionPostcondition) -> Result<()> {
    match postcondition {
        ActionPostcondition::None => return Ok(()),
        ActionPostcondition::Idle {
            stable_for_ns,
            timeout_ns,
        } => {
            if *stable_for_ns > 0
                && *timeout_ns > 0
                && *timeout_ns <= 3_600_000_000_000
                && *stable_for_ns <= *timeout_ns
            {
                return Ok(());
            }
        }
        ActionPostcondition::Screen {
            predicate,
            timeout_ns,
        } => {
            if *timeout_ns > 0
                && *timeout_ns <= 3_600_000_000_000
                && validate_screen_predicate(predicate).is_ok()
            {
                return Ok(());
            }
        }
    }
    return Err(control_rejected(
        "invalid_action_postcondition",
        "action postcondition exceeds its predicate limits or durations do not satisfy stable <= timeout <= 1h",
    ));
}
pub(super) fn evaluate_action_postcondition(
    pending: &mut PendingInputSequence,
    event_log: &EventLogWriter,
    session_id: Uuid,
    live: &LiveTerminal,
    child_exited: bool,
    output_closed: bool,
) -> Result<Option<ControlData>> {
    let input = pending.input.clone().ok_or_else(|| {
        return terminal_error(
            "evaluate actor-ordered action",
            "postcondition started without applied input",
        );
    })?;
    let postcondition = pending.postcondition.as_ref().ok_or_else(|| {
        return terminal_error(
            "evaluate actor-ordered action",
            "missing action postcondition",
        );
    })?;
    let projection = pending.projection.as_ref().ok_or_else(|| {
        return terminal_error(
            "evaluate actor-ordered action",
            "missing action screen projection",
        );
    })?;
    if matches!(postcondition, ActionPostcondition::None) {
        let boundaries = actor_boundaries(event_log);
        let frame = actor_screen_frame(
            session_id,
            event_log,
            live,
            boundaries.clone(),
            projection,
            CaptureQuality::Live,
        )?;
        return Ok(Some(ControlData::ActionApplied {
            input,
            postcondition: ActionPostconditionResult::None {
                matched: true,
                frame,
            },
            boundaries,
        }));
    }
    let state_hash = presented_state_hash(live)?;
    let viewport_text = semantic_viewport_text(&live.presented);
    let now = Instant::now();
    let child_completed = child_exited && output_closed;
    let started_at = pending.postcondition_started_at.unwrap_or(now);
    return match postcondition {
        ActionPostcondition::None => Err(terminal_error(
            "evaluate actor-ordered action",
            "unexpected empty action postcondition",
        )),
        ActionPostcondition::Idle {
            stable_for_ns,
            timeout_ns,
        } => {
            match &pending.matching_state {
                Some((prior_hash, _)) if prior_hash == &state_hash => {}
                _ => pending.matching_state = Some((state_hash, now)),
            }
            let stable = pending.matching_state.as_ref().is_some_and(|(_, since)| {
                return since.elapsed() >= Duration::from_nanos(*stable_for_ns);
            });
            let deadline = !child_exited && now >= started_at + Duration::from_nanos(*timeout_ns);
            if !stable && !child_completed && !output_closed && !deadline {
                return Ok(None);
            }
            let quality = if stable {
                StableCaptureQuality::Stable
            } else if child_completed {
                StableCaptureQuality::ChildCompleted
            } else if output_closed {
                StableCaptureQuality::OutputClosed
            } else {
                StableCaptureQuality::Deadline
            };
            let capture_quality = if stable {
                CaptureQuality::Stable
            } else if deadline {
                CaptureQuality::Incomplete
            } else {
                CaptureQuality::Live
            };
            let boundaries = actor_boundaries(event_log);
            let frame = actor_screen_frame(
                session_id,
                event_log,
                live,
                boundaries.clone(),
                projection,
                capture_quality,
            )?;
            Ok(Some(ControlData::ActionApplied {
                input,
                postcondition: ActionPostconditionResult::Idle {
                    matched: stable,
                    quality,
                    frame,
                },
                boundaries,
            }))
        }
        ActionPostcondition::Screen {
            predicate,
            timeout_ns,
        } => {
            let matched =
                evaluate_screen_predicate(predicate, &live.presented, &viewport_text, &state_hash);
            let deadline = !child_exited && now >= started_at + Duration::from_nanos(*timeout_ns);
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
            let boundaries = actor_boundaries(event_log);
            let frame = actor_screen_frame(
                session_id,
                event_log,
                live,
                boundaries.clone(),
                projection,
                capture_quality,
            )?;
            Ok(Some(ControlData::ActionApplied {
                input,
                postcondition: ActionPostconditionResult::Screen {
                    matched,
                    quality,
                    frame,
                },
                boundaries,
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
    ) -> Option<Result<ControlData>> {
        let outcome = match command {
            ControlCommand::Ping => Ok(ControlData::Acknowledged {
                boundaries: actor_boundaries(&self.event_log),
            }),
            ControlCommand::Status => Ok(ControlData::Status {
                pid: self.spawned.pid,
                boundaries: actor_boundaries(&self.event_log),
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
                let state_hash = match semantic_state_hash(&self.live.presented) {
                    Ok(state_hash) => state_hash,
                    Err(error) => return Some(Err(Error::ScreenFrame(error))),
                };
                Ok(ControlData::Snapshot {
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
                .map(|frame| return ControlData::ScreenFrame { frame })
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
