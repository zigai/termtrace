//! Recording-actor control admission and command execution.

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use nix::{
    errno::Errno,
    sys::signal::{Signal, killpg},
    unistd::Pid,
};
use uuid::Uuid;
use zstd::stream::encode_all;

use crate::{
    Error, Result,
    control::{
        ActionInputResult, ActionPostcondition, ActionPostconditionResult, ActorBoundaries,
        CONTROL_PROTOCOL_VERSION, ControlCommand, ControlData, ControlErrorDetails, ControlRequest,
        ControlResponse, ControlResult, InputSequenceFailureDetails, PointerOperation,
        ProcessSignal, ScreenFrameFailureDetails, ScreenPostconditionQuality, StableCaptureQuality,
    },
    event::{ByteString, Event, InputDescriptor, InputKind, InputSource, OperationStatus},
    event_log::EventLogWriter,
    ghostty_vt::{
        FocusEvent, MouseAction, MouseButton, encode_focus, supported_geometry_cell_count,
    },
    observation::CaptureQuality,
    replay::semantic_state_hash,
    screen_frame::{
        ScreenFrame, ScreenFrameError, ScreenFrameLimitKind, ScreenFrameSource,
        ScreenProjectionRequest, project_screen_frame, validate_screen_region,
    },
};

use super::super::{
    input::{
        InputSequenceExecution, apply_input, apply_resize, encode_paste, interpolate_cell,
        mouse_button_name, semantic_key_input, semantic_key_name,
    },
    model::{SpawnedCommand, TerminalGeometry},
    presentation::{LiveTerminal, find_text_regions, resize_semantic, semantic_viewport_text},
    support::{
        actor_boundaries, control_error_response, control_rejected, finish_with_cleanup,
        terminal_error, validate_control_input,
    },
};
use super::{
    history::MutationHistory,
    screen::{
        MAX_SCREEN_WAITERS, ScreenWaiter, evaluate_screen_predicate, presented_state_hash,
        valid_blake3_hash, validate_screen_predicate,
    },
};

pub(in crate::record) struct PendingInputSequence {
    request: ControlRequest,
    response_sender: mpsc::SyncSender<ControlResponse>,
    command_hash: String,
    event_start_seq: u64,
    execution: InputSequenceExecution,
    postcondition: Option<ActionPostcondition>,
    projection: Option<ScreenProjectionRequest>,
    input: Option<ActionInputResult>,
    postcondition_started_at: Option<Instant>,
    matching_state: Option<(String, Instant)>,
}

impl PendingInputSequence {
    pub(in crate::record) fn next_deadline(&self) -> Instant {
        if self.input.is_none() {
            return self.execution.next_deadline();
        }
        let Some(postcondition) = &self.postcondition else {
            return Instant::now();
        };
        let started_at = self.postcondition_started_at.unwrap_or_else(Instant::now);
        return match postcondition {
            ActionPostcondition::None => Instant::now(),
            ActionPostcondition::Idle {
                stable_for_ns,
                timeout_ns,
            } => {
                let deadline = started_at + Duration::from_nanos(*timeout_ns);
                self.matching_state
                    .as_ref()
                    .map_or(deadline, |(_, since)| {
                        return *since + Duration::from_nanos(*stable_for_ns);
                    })
                    .min(deadline)
            }
            ActionPostcondition::Screen { timeout_ns, .. } => {
                started_at + Duration::from_nanos(*timeout_ns)
            }
        };
    }

    fn exclusive(&self) -> bool {
        return self.postcondition.is_some();
    }
}

pub(in crate::record) fn handle_control_request(
    spawned: &mut SpawnedCommand,
    session_id: Uuid,
    event_log: &mut EventLogWriter,
    request: ControlRequest,
    response_sender: mpsc::SyncSender<ControlResponse>,
    history: &mut MutationHistory,
    live: &mut LiveTerminal,
    screen_waiters: &mut Vec<ScreenWaiter>,
    pending_input_sequence: &mut Option<PendingInputSequence>,
) -> Result<()> {
    if request.expected_presented_state_hash.is_some()
        && matches!(
            request.command,
            ControlCommand::Ping
                | ControlCommand::Status
                | ControlCommand::Snapshot
                | ControlCommand::ScreenFrame { .. }
                | ControlCommand::WaitScreen { .. }
                | ControlCommand::CaptureStable { .. }
        )
    {
        let response = control_error_response(
            request.request_id,
            "invalid_observation_guard",
            "presented-state guards are only valid for mutations",
        );
        let _ = response_sender.send(response);
        return Ok(());
    }
    if pending_input_sequence
        .as_ref()
        .is_some_and(PendingInputSequence::exclusive)
    {
        let response = control_error_response(
            request.request_id,
            "action_busy",
            "an actor-ordered action is still applying input or evaluating its postcondition",
        );
        let _ = response_sender.send(response);
        return Ok(());
    }
    if let ControlCommand::WaitScreen {
        predicate,
        stable_for_ns,
        timeout_ns,
        projection,
    } = &request.command
    {
        if validate_screen_predicate(predicate).is_err()
            || *timeout_ns == 0
            || *timeout_ns > 3_600_000_000_000
            || *stable_for_ns > *timeout_ns
        {
            let response = control_error_response(
                request.request_id,
                "invalid_screen_wait",
                "screen predicate exceeds its text/regex/region/composition limits or durations do not satisfy stable <= timeout <= 1h",
            );
            let _ = response_sender.send(response);
            return Ok(());
        }
        if let Err(error) =
            validate_screen_region(projection, live.presented.cols, live.presented.rows)
        {
            let response = screen_frame_error_response(
                request.request_id,
                &error,
                presented_state_hash(live)?,
                event_log,
                "screen_wait_admission",
            );
            let _ = response_sender.send(response);
            return Ok(());
        }
        if screen_waiters.len() >= MAX_SCREEN_WAITERS {
            let response = control_error_response(
                request.request_id,
                "waiter_limit_reached",
                "session screen waiter limit has been reached",
            );
            let _ = response_sender.send(response);
            return Ok(());
        }
        screen_waiters.push(ScreenWaiter {
            request_id: request.request_id,
            predicate: Some(predicate.clone()),
            stable_for: Duration::from_nanos(*stable_for_ns),
            deadline: Instant::now() + Duration::from_nanos(*timeout_ns),
            matching_state: None,
            projection: *projection,
            response: response_sender,
        });
        return Ok(());
    }
    if let ControlCommand::CaptureStable {
        stable_for_ns,
        timeout_ns,
        projection,
    } = &request.command
    {
        if *stable_for_ns == 0
            || *timeout_ns == 0
            || *timeout_ns > 3_600_000_000_000
            || *stable_for_ns > *timeout_ns
        {
            let response = control_error_response(
                request.request_id,
                "invalid_stable_capture",
                "durations must satisfy stable <= timeout <= 1h and timeout must be nonzero",
            );
            let _ = response_sender.send(response);
            return Ok(());
        }
        if let Err(error) =
            validate_screen_region(projection, live.presented.cols, live.presented.rows)
        {
            let response = screen_frame_error_response(
                request.request_id,
                &error,
                presented_state_hash(live)?,
                event_log,
                "stable_capture_admission",
            );
            let _ = response_sender.send(response);
            return Ok(());
        }
        if screen_waiters.len() >= MAX_SCREEN_WAITERS {
            let response = control_error_response(
                request.request_id,
                "waiter_limit_reached",
                "session screen waiter limit has been reached",
            );
            let _ = response_sender.send(response);
            return Ok(());
        }
        screen_waiters.push(ScreenWaiter {
            request_id: request.request_id,
            predicate: None,
            stable_for: Duration::from_nanos(*stable_for_ns),
            deadline: Instant::now() + Duration::from_nanos(*timeout_ns),
            matching_state: None,
            projection: *projection,
            response: response_sender,
        });
        return Ok(());
    }
    let read_only = matches!(
        request.command,
        ControlCommand::Ping
            | ControlCommand::Status
            | ControlCommand::Snapshot
            | ControlCommand::ScreenFrame { .. }
            | ControlCommand::CaptureStable { .. }
    );
    if pending_input_sequence.is_some() && !read_only {
        let response = control_error_response(
            request.request_id,
            "input_sequence_busy",
            "another paced input sequence is still in progress",
        );
        let _ = response_sender.send(response);
        return Ok(());
    }
    let command_hash = if read_only {
        None
    } else {
        let (hash, prior) = history.lookup(
            request.request_id,
            &request.command,
            request.expected_presented_state_hash.as_deref(),
        )?;
        if let Some(response) = prior {
            let _ = response_sender.send(response);
            return Ok(());
        }
        if history.is_full() {
            let response = control_error_response(
                request.request_id,
                "mutation_limit_reached",
                "session mutation operation limit has been reached",
            );
            let _ = response_sender.send(response);
            return Ok(());
        }
        Some(hash)
    };

    let event_start_seq = event_log.next_seq();
    let current_state_hash = presented_state_hash(live)?;
    let guard = request.expected_presented_state_hash.as_deref();
    let stale = guard.is_some_and(|hash| {
        return valid_blake3_hash(hash) && hash != current_state_hash;
    });
    let invalid_guard = guard.is_some_and(|hash| return !valid_blake3_hash(hash));
    if !invalid_guard && !stale {
        let sequence = match &request.command {
            ControlCommand::InputSequence {
                atoms,
                pace_ns,
                include_atom_outcomes,
            } => Some((atoms, *pace_ns, *include_atom_outcomes, None, None)),
            ControlCommand::Act {
                atoms,
                pace_ns,
                include_atom_outcomes,
                postcondition,
                projection,
            } => Some((
                atoms,
                *pace_ns,
                *include_atom_outcomes,
                Some(postcondition.clone()),
                Some(*projection),
            )),
            _ => None,
        };
        if let Some((atoms, pace_ns, include_atom_outcomes, postcondition, projection)) = sequence {
            let validated_postcondition = postcondition
                .as_ref()
                .map(validate_action_postcondition)
                .transpose()
                .and_then(|_| {
                    if let Some(projection) = &projection {
                        validate_screen_region(
                            projection,
                            live.presented.cols,
                            live.presented.rows,
                        )
                        .map_err(Error::ScreenFrame)?;
                    }
                    return Ok(());
                });
            let execution = validated_postcondition.and_then(|_| {
                return InputSequenceExecution::new(
                    request.request_id,
                    atoms,
                    pace_ns,
                    include_atom_outcomes,
                );
            });
            match execution {
                Ok(execution) => {
                    *pending_input_sequence = Some(PendingInputSequence {
                        request,
                        response_sender,
                        command_hash: command_hash.ok_or_else(|| {
                            return terminal_error(
                                "schedule input sequence",
                                "missing mutation command hash",
                            );
                        })?,
                        event_start_seq,
                        execution,
                        postcondition,
                        input: None,
                        postcondition_started_at: None,
                        matching_state: None,
                        projection,
                    });
                    return Ok(());
                }
                Err(error) => {
                    let response = match &error {
                        Error::ScreenFrame(error) => screen_frame_error_response(
                            request.request_id,
                            error,
                            current_state_hash,
                            event_log,
                            "action_admission",
                        ),
                        _ => control_error_response(
                            request.request_id,
                            control_error_code(&error),
                            error.to_string(),
                        ),
                    };
                    if let Some(command_hash) = command_hash {
                        history.remember(
                            request.request_id,
                            &request.command,
                            command_hash,
                            event_start_seq,
                            event_log.next_seq(),
                            event_log.current_hash(),
                            response.clone(),
                        )?;
                    }
                    let _ = response_sender.send(response);
                    return match error {
                        Error::ControlRejected { .. } | Error::ScreenFrame(_) => Ok(()),
                        error => Err(error),
                    };
                }
            }
        }
    }
    let outcome = if invalid_guard {
        Err(control_rejected(
            "invalid_observation_guard",
            "expected presented-state hash must be 64 lowercase hexadecimal characters",
        ))
    } else if stale {
        Err(Error::ControlRejected {
            code: "stale_observation".to_owned(),
            message: format!("presented state changed; current state hash is {current_state_hash}"),
        })
    } else {
        execute_control_command(
            spawned,
            event_log,
            session_id,
            request.request_id,
            &request.command,
            live,
        )
    };
    let response = match (&outcome, stale) {
        (Err(Error::ControlRejected { code, message }), true) => ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: request.request_id,
            result: ControlResult::Error {
                code: code.clone(),
                message: message.clone(),
                details: Some(ControlErrorDetails {
                    current_presented_state_hash: current_state_hash,
                    boundaries: actor_boundaries(event_log),
                    scope: "actor_processed_presented_output".to_owned(),
                    input_sequence: None,
                    screen_frame: None,
                }),
            },
        },
        (
            Err(Error::InputSequenceIncomplete {
                sequence_id,
                failed_atom_index,
                failed_operation_id,
                atoms_applied,
                bytes_written,
                failed_atom_bytes_written,
                first_intent_event_seq,
                last_result_event_seq,
                reason,
            }),
            _,
        ) => {
            let atom_count = match &request.command {
                ControlCommand::InputSequence { atoms, .. } => {
                    u32::try_from(atoms.len()).unwrap_or(u32::MAX)
                }
                _ => 0,
            };
            ControlResponse {
                version: CONTROL_PROTOCOL_VERSION,
                request_id: request.request_id,
                result: ControlResult::Error {
                    code: "input_sequence_incomplete".to_owned(),
                    message: format!(
                        "input sequence `{sequence_id}` stopped at atom {failed_atom_index}: {reason}"
                    ),
                    details: Some(ControlErrorDetails {
                        current_presented_state_hash: current_state_hash,
                        boundaries: actor_boundaries(event_log),
                        scope: "input_sequence_attempted_prefix".to_owned(),
                        input_sequence: Some(InputSequenceFailureDetails {
                            sequence_id: *sequence_id,
                            atom_count,
                            atoms_applied: *atoms_applied,
                            failed_atom_index: *failed_atom_index,
                            failed_operation_id: *failed_operation_id,
                            bytes_written: *bytes_written,
                            failed_atom_bytes_written: *failed_atom_bytes_written,
                            first_intent_event_seq: *first_intent_event_seq,
                            last_result_event_seq: *last_result_event_seq,
                            resume_from_atom_index: failed_atom_index.saturating_add(1),
                        }),
                        screen_frame: None,
                    }),
                },
            }
        }
        (Ok(data), _) => ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: request.request_id,
            result: ControlResult::Ok { data: data.clone() },
        },
        (Err(Error::ScreenFrame(error)), _) => ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id: request.request_id,
            result: ControlResult::Error {
                code: screen_frame_error_code(error).to_owned(),
                message: error.to_string(),
                details: Some(ControlErrorDetails {
                    current_presented_state_hash: current_state_hash,
                    boundaries: actor_boundaries(event_log),
                    scope: "screen_frame_projection".to_owned(),
                    input_sequence: None,
                    screen_frame: Some(screen_frame_failure_details(error)),
                }),
            },
        },
        (Err(error), _) => control_error_response(
            request.request_id,
            control_error_code(error),
            error.to_string(),
        ),
    };
    if let Some(command_hash) = command_hash {
        history.remember(
            request.request_id,
            &request.command,
            command_hash,
            event_start_seq,
            event_log.next_seq(),
            event_log.current_hash(),
            response.clone(),
        )?;
    }
    if response_sender.send(response).is_err() {
        tracing::warn!(request_id = %request.request_id, "control response receiver disconnected");
    }
    match outcome {
        Ok(_)
        | Err(
            Error::ControlRejected { .. }
            | Error::InputSequenceIncomplete { .. }
            | Error::GhosttyVt { .. }
            | Error::ScreenFrame(_),
        ) => {
            return Ok(());
        }
        Err(error) => return Err(error),
    }
}

pub(in crate::record) fn advance_input_sequence(
    spawned: &mut SpawnedCommand,
    session_id: Uuid,
    event_log: &mut EventLogWriter,
    history: &mut MutationHistory,
    live: &mut LiveTerminal,
    pending_input_sequence: &mut Option<PendingInputSequence>,
    child_completed: bool,
    output_closed: bool,
) -> Result<()> {
    let Some(mut pending) = pending_input_sequence.take() else {
        return Ok(());
    };
    let screen_postcondition_ready = pending.input.is_some()
        && matches!(
            &pending.postcondition,
            Some(ActionPostcondition::Screen { .. })
        );
    if !screen_postcondition_ready && Instant::now() < pending.next_deadline() {
        *pending_input_sequence = Some(pending);
        return Ok(());
    }
    let terminal_outcome = if pending.input.is_some() {
        match evaluate_action_postcondition(
            &mut pending,
            event_log,
            session_id,
            live,
            child_completed,
            output_closed,
        ) {
            Ok(Some(data)) => Ok(data),
            Ok(None) => {
                *pending_input_sequence = Some(pending);
                return Ok(());
            }
            Err(error) => Err(error),
        }
    } else {
        match pending
            .execution
            .advance(spawned.writer.as_mut(), event_log, &mut live.semantic)
        {
            Ok(None) => {
                *pending_input_sequence = Some(pending);
                return Ok(());
            }
            Ok(Some(data)) if pending.postcondition.is_some() => {
                let input = action_input_result(data)?;
                pending.input = Some(input);
                pending.postcondition_started_at = Some(Instant::now());
                match evaluate_action_postcondition(
                    &mut pending,
                    event_log,
                    session_id,
                    live,
                    child_completed,
                    output_closed,
                ) {
                    Ok(Some(data)) => Ok(data),
                    Ok(None) => {
                        *pending_input_sequence = Some(pending);
                        return Ok(());
                    }
                    Err(error) => Err(error),
                }
            }
            Ok(Some(data)) => Ok(data),
            Err(error) => Err(error),
        }
    };
    let current_state_hash = presented_state_hash(live)?;
    let atom_count = match &pending.request.command {
        ControlCommand::InputSequence { atoms, .. } | ControlCommand::Act { atoms, .. } => {
            u32::try_from(atoms.len()).unwrap_or(u32::MAX)
        }
        _ => 0,
    };
    let response = input_sequence_response(
        pending.request.request_id,
        atom_count,
        &terminal_outcome,
        current_state_hash,
        event_log,
    );
    history.remember(
        pending.request.request_id,
        &pending.request.command,
        pending.command_hash,
        pending.event_start_seq,
        event_log.next_seq(),
        event_log.current_hash(),
        response.clone(),
    )?;
    if pending.response_sender.send(response).is_err() {
        tracing::warn!(
            request_id = %pending.request.request_id,
            "input sequence response receiver disconnected"
        );
    }
    return match terminal_outcome {
        Ok(_)
        | Err(
            Error::ControlRejected { .. }
            | Error::InputSequenceIncomplete { .. }
            | Error::GhosttyVt { .. }
            | Error::ScreenFrame(_),
        ) => Ok(()),
        Err(error) => Err(error),
    };
}

fn validate_action_postcondition(postcondition: &ActionPostcondition) -> Result<()> {
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

fn action_input_result(data: ControlData) -> Result<ActionInputResult> {
    let ControlData::InputSequenceApplied {
        sequence_id,
        atom_count,
        bytes_written,
        first_intent_event_seq,
        last_result_event_seq,
        pace_ns,
        atoms,
        ..
    } = data
    else {
        return Err(terminal_error(
            "complete actor-ordered action",
            "input sequence returned an unexpected payload",
        ));
    };
    return Ok(ActionInputResult {
        sequence_id,
        atom_count,
        bytes_written,
        first_intent_event_seq,
        last_result_event_seq,
        pace_ns,
        atoms,
    });
}

fn evaluate_action_postcondition(
    pending: &mut PendingInputSequence,
    event_log: &EventLogWriter,
    session_id: Uuid,
    live: &LiveTerminal,
    child_completed: bool,
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
            let deadline = now >= started_at + Duration::from_nanos(*timeout_ns);
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

fn actor_screen_frame(
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

fn input_sequence_response(
    request_id: Uuid,
    atom_count: u32,
    outcome: &Result<ControlData>,
    current_state_hash: String,
    event_log: &EventLogWriter,
) -> ControlResponse {
    return match outcome {
        Ok(data) => ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id,
            result: ControlResult::Ok { data: data.clone() },
        },
        Err(Error::InputSequenceIncomplete {
            sequence_id,
            failed_atom_index,
            failed_operation_id,
            atoms_applied,
            bytes_written,
            failed_atom_bytes_written,
            first_intent_event_seq,
            last_result_event_seq,
            reason,
        }) => ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id,
            result: ControlResult::Error {
                code: "input_sequence_incomplete".to_owned(),
                message: format!(
                    "input sequence `{sequence_id}` stopped at atom {failed_atom_index}: {reason}"
                ),
                details: Some(ControlErrorDetails {
                    current_presented_state_hash: current_state_hash,
                    boundaries: actor_boundaries(event_log),
                    scope: "input_sequence_attempted_prefix".to_owned(),
                    input_sequence: Some(InputSequenceFailureDetails {
                        sequence_id: *sequence_id,
                        atom_count,
                        atoms_applied: *atoms_applied,
                        failed_atom_index: *failed_atom_index,
                        failed_operation_id: *failed_operation_id,
                        bytes_written: *bytes_written,
                        failed_atom_bytes_written: *failed_atom_bytes_written,
                        first_intent_event_seq: *first_intent_event_seq,
                        last_result_event_seq: *last_result_event_seq,
                        resume_from_atom_index: failed_atom_index.saturating_add(1),
                    }),
                    screen_frame: None,
                }),
            },
        },
        Err(Error::ScreenFrame(error)) => ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id,
            result: ControlResult::Error {
                code: screen_frame_error_code(error).to_owned(),
                message: error.to_string(),
                details: Some(ControlErrorDetails {
                    current_presented_state_hash: current_state_hash,
                    boundaries: actor_boundaries(event_log),
                    scope: "action_terminating_screen_frame".to_owned(),
                    input_sequence: None,
                    screen_frame: Some(screen_frame_failure_details(error)),
                }),
            },
        },
        Err(Error::ControlRejected { code, message }) => ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id,
            result: ControlResult::Error {
                code: code.clone(),
                message: message.clone(),
                details: Some(ControlErrorDetails {
                    current_presented_state_hash: current_state_hash,
                    boundaries: actor_boundaries(event_log),
                    scope: "input_sequence_encoding".to_owned(),
                    input_sequence: None,
                    screen_frame: None,
                }),
            },
        },
        Err(error) => {
            control_error_response(request_id, control_error_code(error), error.to_string())
        }
    };
}

fn execute_control_command(
    spawned: &mut SpawnedCommand,
    event_log: &mut EventLogWriter,
    session_id: Uuid,
    request_id: Uuid,
    command: &ControlCommand,
    live: &mut LiveTerminal,
) -> Result<ControlData> {
    match command {
        ControlCommand::Ping => {
            return Ok(ControlData::Acknowledged {
                boundaries: actor_boundaries(event_log),
            });
        }
        ControlCommand::Status => {
            return Ok(ControlData::Status {
                pid: spawned.pid,
                boundaries: actor_boundaries(event_log),
            });
        }
        ControlCommand::Snapshot => {
            let encoded = serde_json::to_vec(&live.presented).map_err(|error| {
                return terminal_error("serialize presented state", error.to_string());
            })?;
            let compressed = encode_all(encoded.as_slice(), 3).map_err(|error| {
                return terminal_error("compress presented state", error.to_string());
            })?;
            return Ok(ControlData::Snapshot {
                semantic_state_zstd: ByteString::from(compressed),
                viewport_text: semantic_viewport_text(&live.presented),
                state_hash: semantic_state_hash(&live.presented)?,
                presented: true,
                cols: live.presented.cols,
                rows: live.presented.rows,
                cursor_column: live.presented.cursor.column,
                cursor_row: live.presented.cursor.row,
                cursor_visible: live.presented.cursor.visible,
                observed_monotonic_ns: event_log.elapsed_ns(),
                modes: live.presented.modes.clone(),
                boundaries: actor_boundaries(event_log),
            });
        }
        ControlCommand::ScreenFrame { projection } => {
            let boundaries = actor_boundaries(event_log);
            let frame = actor_screen_frame(
                session_id,
                event_log,
                live,
                boundaries,
                projection,
                CaptureQuality::Live,
            )?;
            return Ok(ControlData::ScreenFrame { frame });
        }
        ControlCommand::WaitScreen { .. } | ControlCommand::CaptureStable { .. } => {
            return Err(control_rejected(
                "invalid_screen_wait_dispatch",
                "screen waits and stable captures must be registered asynchronously",
            ));
        }
        ControlCommand::Input {
            data,
            input_kind,
            caller_intent,
        } => {
            validate_control_input(data.as_bytes(), caller_intent.as_deref())?;
            let modes = live.semantic.snapshot()?.modes;
            let mode_bytes = serde_json::to_vec(&modes).map_err(|error| {
                return terminal_error("serialize input modes", error.to_string());
            })?;
            let terminal_modes_hash = blake3::hash(&mode_bytes).to_hex().to_string();
            let semantic_input = match input_kind {
                InputKind::Text => "text",
                InputKind::Raw => "raw",
                InputKind::Keys => "keys",
                InputKind::Paste => "paste",
                InputKind::Focus => "focus",
                InputKind::Pointer => "pointer",
            };
            let applied = apply_input(
                spawned.writer.as_mut(),
                event_log,
                request_id,
                InputSource::Agent,
                *input_kind,
                match input_kind {
                    InputKind::Text => InputDescriptor::Text,
                    InputKind::Raw
                    | InputKind::Keys
                    | InputKind::Paste
                    | InputKind::Focus
                    | InputKind::Pointer => InputDescriptor::Raw,
                },
                Some(terminal_modes_hash),
                Some(semantic_input.to_owned()),
                caller_intent.clone(),
                data.as_bytes(),
            )?;
            return Ok(ControlData::InputApplied {
                operation_id: request_id,
                input_kind: *input_kind,
                data: data.clone(),
                bytes_written: applied.bytes_written,
                intent_event_seq: applied.intent_event_seq,
                result_event_seq: applied.result_event_seq,
                boundaries: actor_boundaries(event_log),
            });
        }
        ControlCommand::InputSequence { .. } | ControlCommand::Act { .. } => {
            return Err(terminal_error(
                "dispatch input sequence",
                "paced input sequence was not scheduled by the actor",
            ));
        }
        ControlCommand::Key { key, modifiers } => {
            let modes = live.semantic.snapshot()?.modes;
            let mode_bytes = serde_json::to_vec(&modes).map_err(|error| {
                return terminal_error("serialize keyboard modes", error.to_string());
            })?;
            let terminal_modes_hash = blake3::hash(&mode_bytes).to_hex().to_string();
            let encoded = live.semantic.encode_key(*key, *modifiers)?;
            if encoded.data.as_bytes().is_empty() {
                return Err(control_rejected(
                    "key_has_no_output",
                    "the requested key press produced no terminal bytes",
                ));
            }
            let applied = apply_input(
                spawned.writer.as_mut(),
                event_log,
                request_id,
                InputSource::Agent,
                InputKind::Keys,
                InputDescriptor::Key {
                    key: semantic_key_name(*key),
                    shift: modifiers.shift,
                    control: modifiers.control,
                    alt: modifiers.alt,
                    super_key: modifiers.super_key,
                    encoder: encoded.encoder,
                    terminal_modes_hash: terminal_modes_hash.clone(),
                },
                Some(terminal_modes_hash),
                Some(semantic_key_input(*key, *modifiers)),
                None,
                encoded.data.as_bytes(),
            )?;
            return Ok(ControlData::InputApplied {
                operation_id: request_id,
                input_kind: InputKind::Keys,
                data: encoded.data,
                bytes_written: applied.bytes_written,
                intent_event_seq: applied.intent_event_seq,
                result_event_seq: applied.result_event_seq,
                boundaries: actor_boundaries(event_log),
            });
        }
        ControlCommand::Paste { data } => {
            validate_control_input(data.as_bytes(), Some("paste"))?;
            let modes = live.semantic.snapshot()?.modes;
            let mode_bytes = serde_json::to_vec(&modes).map_err(|error| {
                return terminal_error("serialize paste modes", error.to_string());
            })?;
            let terminal_modes_hash = blake3::hash(&mode_bytes).to_hex().to_string();
            let encoded = encode_paste(data.as_bytes(), modes.bracketed_paste)?;
            let applied = apply_input(
                spawned.writer.as_mut(),
                event_log,
                request_id,
                InputSource::Agent,
                InputKind::Paste,
                InputDescriptor::Paste {
                    bracketed: modes.bracketed_paste,
                    terminal_modes_hash: terminal_modes_hash.clone(),
                },
                Some(terminal_modes_hash),
                Some("paste".to_owned()),
                None,
                &encoded,
            )?;
            return Ok(ControlData::InputApplied {
                operation_id: request_id,
                input_kind: InputKind::Paste,
                data: ByteString::new(encoded),
                bytes_written: applied.bytes_written,
                intent_event_seq: applied.intent_event_seq,
                result_event_seq: applied.result_event_seq,
                boundaries: actor_boundaries(event_log),
            });
        }
        ControlCommand::Focus { focused, strict } => {
            let modes = live.semantic.snapshot()?.modes;
            let mode_bytes = serde_json::to_vec(&modes).map_err(|error| {
                return terminal_error("serialize focus modes", error.to_string());
            })?;
            let terminal_modes_hash = blake3::hash(&mode_bytes).to_hex().to_string();
            let reporting_enabled = modes
                .values
                .iter()
                .any(|mode| return !mode.ansi && mode.number == 1004 && mode.enabled);
            if !reporting_enabled {
                if *strict {
                    return Err(control_rejected(
                        "focus_reporting_disabled",
                        "the application has not enabled DEC focus reporting mode 1004",
                    ));
                }
                return Ok(ControlData::InputNoop {
                    input_kind: InputKind::Focus,
                    reason: "focus_reporting_disabled".to_owned(),
                    terminal_modes_hash,
                    boundaries: actor_boundaries(event_log),
                });
            }
            let encoded = encode_focus(if *focused {
                FocusEvent::In
            } else {
                FocusEvent::Out
            })?;
            let applied = apply_input(
                spawned.writer.as_mut(),
                event_log,
                request_id,
                InputSource::Agent,
                InputKind::Focus,
                InputDescriptor::Focus {
                    focused: *focused,
                    encoder: "libghostty-focus-encoder-v1".to_owned(),
                    terminal_modes_hash: terminal_modes_hash.clone(),
                },
                Some(terminal_modes_hash),
                Some(if *focused { "focus-in" } else { "focus-out" }.to_owned()),
                None,
                encoded.as_bytes(),
            )?;
            return Ok(ControlData::InputApplied {
                operation_id: request_id,
                input_kind: InputKind::Focus,
                data: encoded,
                bytes_written: applied.bytes_written,
                intent_event_seq: applied.intent_event_seq,
                result_event_seq: applied.result_event_seq,
                boundaries: actor_boundaries(event_log),
            });
        }
        ControlCommand::Pointer {
            column,
            row,
            operation,
            strict,
        } => {
            if *column >= live.geometry.cols || *row >= live.geometry.rows {
                return Err(control_rejected(
                    "pointer_cell_out_of_bounds",
                    &format!(
                        "pointer cell {column},{row} is outside {}x{} geometry",
                        live.geometry.cols, live.geometry.rows
                    ),
                ));
            }
            let modes = live.semantic.snapshot()?.modes;
            let mode_bytes = serde_json::to_vec(&modes).map_err(|error| {
                return terminal_error("serialize pointer modes", error.to_string());
            })?;
            let terminal_modes_hash = blake3::hash(&mode_bytes).to_hex().to_string();
            let reporting_enabled = modes.values.iter().any(|mode| {
                return !mode.ansi && matches!(mode.number, 9 | 1000 | 1002 | 1003) && mode.enabled;
            });
            if !reporting_enabled {
                if *strict {
                    return Err(control_rejected(
                        "pointer_reporting_disabled",
                        "the application has not enabled a DEC mouse tracking mode",
                    ));
                }
                return Ok(ControlData::InputNoop {
                    input_kind: InputKind::Pointer,
                    reason: "pointer_reporting_disabled".to_owned(),
                    terminal_modes_hash,
                    boundaries: actor_boundaries(event_log),
                });
            }
            let mut encoded = Vec::new();
            let (operation_name, button_name, lines, end_cell, click_count) = match operation {
                PointerOperation::Click { button } => {
                    if matches!(button, MouseButton::WheelUp | MouseButton::WheelDown) {
                        return Err(control_rejected(
                            "invalid_pointer_button",
                            "wheel buttons are reserved for scroll operations",
                        ));
                    }
                    for action in [MouseAction::Press, MouseAction::Release] {
                        encoded.extend_from_slice(
                            live.semantic
                                .encode_mouse(
                                    action,
                                    *button,
                                    *column,
                                    *row,
                                    live.geometry.cols,
                                    live.geometry.rows,
                                    live.geometry.pixel_width,
                                    live.geometry.pixel_height,
                                )?
                                .as_bytes(),
                        );
                    }
                    ("click", mouse_button_name(*button), 0, None, 1)
                }
                PointerOperation::MultiClick { button, count } => {
                    if matches!(button, MouseButton::WheelUp | MouseButton::WheelDown)
                        || !(2..=4).contains(count)
                    {
                        return Err(control_rejected(
                            "invalid_multi_click",
                            "multi-click requires a non-wheel button and a count from 2 through 4",
                        ));
                    }
                    for _ in 0..*count {
                        for action in [MouseAction::Press, MouseAction::Release] {
                            encoded.extend_from_slice(
                                live.semantic
                                    .encode_mouse(
                                        action,
                                        *button,
                                        *column,
                                        *row,
                                        live.geometry.cols,
                                        live.geometry.rows,
                                        live.geometry.pixel_width,
                                        live.geometry.pixel_height,
                                    )?
                                    .as_bytes(),
                            );
                        }
                    }
                    ("multi_click", mouse_button_name(*button), 0, None, *count)
                }
                PointerOperation::Drag {
                    button,
                    to_column,
                    to_row,
                    steps,
                } => {
                    if matches!(button, MouseButton::WheelUp | MouseButton::WheelDown)
                        || *steps == 0
                        || *steps > 100
                        || *to_column >= live.geometry.cols
                        || *to_row >= live.geometry.rows
                    {
                        return Err(control_rejected(
                            "invalid_pointer_drag",
                            "drag requires a non-wheel button, an in-bounds destination, and 1 through 100 motion steps",
                        ));
                    }
                    encoded.extend_from_slice(
                        live.semantic
                            .encode_mouse(
                                MouseAction::Press,
                                *button,
                                *column,
                                *row,
                                live.geometry.cols,
                                live.geometry.rows,
                                live.geometry.pixel_width,
                                live.geometry.pixel_height,
                            )?
                            .as_bytes(),
                    );
                    let step_count = u32::from(*steps);
                    for step in 1..=step_count {
                        let drag_column = interpolate_cell(*column, *to_column, step, step_count);
                        let drag_row = interpolate_cell(*row, *to_row, step, step_count);
                        encoded.extend_from_slice(
                            live.semantic
                                .encode_mouse(
                                    MouseAction::Motion,
                                    *button,
                                    drag_column,
                                    drag_row,
                                    live.geometry.cols,
                                    live.geometry.rows,
                                    live.geometry.pixel_width,
                                    live.geometry.pixel_height,
                                )?
                                .as_bytes(),
                        );
                    }
                    encoded.extend_from_slice(
                        live.semantic
                            .encode_mouse(
                                MouseAction::Release,
                                *button,
                                *to_column,
                                *to_row,
                                live.geometry.cols,
                                live.geometry.rows,
                                live.geometry.pixel_width,
                                live.geometry.pixel_height,
                            )?
                            .as_bytes(),
                    );
                    (
                        "drag",
                        mouse_button_name(*button),
                        0,
                        Some((*to_column, *to_row)),
                        0,
                    )
                }
                PointerOperation::Scroll { lines } => {
                    if *lines == 0 || lines.unsigned_abs() > 100 {
                        return Err(control_rejected(
                            "invalid_scroll_lines",
                            "scroll lines must be between -100 and 100 and nonzero",
                        ));
                    }
                    let button = if *lines < 0 {
                        MouseButton::WheelUp
                    } else {
                        MouseButton::WheelDown
                    };
                    for _ in 0..lines.unsigned_abs() {
                        encoded.extend_from_slice(
                            live.semantic
                                .encode_mouse(
                                    MouseAction::Press,
                                    button,
                                    *column,
                                    *row,
                                    live.geometry.cols,
                                    live.geometry.rows,
                                    live.geometry.pixel_width,
                                    live.geometry.pixel_height,
                                )?
                                .as_bytes(),
                        );
                    }
                    ("scroll", mouse_button_name(button), *lines, None, 0)
                }
            };
            if encoded.is_empty() {
                if *strict {
                    return Err(control_rejected(
                        "pointer_has_no_output",
                        "the current tracking mode suppressed this pointer operation",
                    ));
                }
                return Ok(ControlData::InputNoop {
                    input_kind: InputKind::Pointer,
                    reason: "pointer_has_no_output".to_owned(),
                    terminal_modes_hash,
                    boundaries: actor_boundaries(event_log),
                });
            }
            let applied = apply_input(
                spawned.writer.as_mut(),
                event_log,
                request_id,
                InputSource::Agent,
                InputKind::Pointer,
                InputDescriptor::Pointer {
                    operation: operation_name.to_owned(),
                    button: button_name.to_owned(),
                    lines,
                    column: *column,
                    row: *row,
                    end_column: end_cell.map(|cell| return cell.0),
                    end_row: end_cell.map(|cell| return cell.1),
                    click_count,
                    cols: live.geometry.cols,
                    rows: live.geometry.rows,
                    pixel_width: live.geometry.pixel_width,
                    pixel_height: live.geometry.pixel_height,
                    encoder: "libghostty-mouse-encoder-v1".to_owned(),
                    terminal_modes_hash: terminal_modes_hash.clone(),
                },
                Some(terminal_modes_hash),
                Some(format!("pointer-{operation_name}")),
                None,
                &encoded,
            )?;
            return Ok(ControlData::InputApplied {
                operation_id: request_id,
                input_kind: InputKind::Pointer,
                data: ByteString::new(encoded),
                bytes_written: applied.bytes_written,
                intent_event_seq: applied.intent_event_seq,
                result_event_seq: applied.result_event_seq,
                boundaries: actor_boundaries(event_log),
            });
        }
        ControlCommand::Activate {
            text,
            unique,
            strict,
        } => {
            if text.is_empty() || text.len() > 4096 || text.contains('\n') {
                return Err(control_rejected(
                    "invalid_activation_text",
                    "activation text must contain 1 to 4096 bytes on one row",
                ));
            }
            let semantic_state = live.semantic.snapshot()?;
            if semantic_state.modes.synchronized_output {
                return Err(control_rejected(
                    "presentation_transaction_active",
                    "text activation is unavailable during synchronized output",
                ));
            }
            let state_hash = presented_state_hash(live)?;
            let matches = find_text_regions(&live.presented, text);
            let visible = matches
                .iter()
                .filter(|region| return !region.concealed)
                .collect::<Vec<_>>();
            if visible.is_empty() {
                let code = if matches.is_empty() {
                    "activation_text_not_found"
                } else {
                    "activation_text_concealed"
                };
                return Err(control_rejected(
                    code,
                    "activation requires a visible row-contained text match",
                ));
            }
            if *unique && visible.len() != 1 {
                let candidates = visible
                    .iter()
                    .map(|region| {
                        return format!(
                            "{}:{}-{}",
                            region.row, region.start_column, region.end_column
                        );
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                return Err(control_rejected(
                    "activation_text_ambiguous",
                    &format!("activation text matched multiple regions: {candidates}"),
                ));
            }
            let region = visible
                .first()
                .ok_or_else(|| return control_rejected("activation_text_not_found", "no match"))?;
            let target_column = region
                .start_column
                .saturating_add(region.end_column.saturating_sub(region.start_column + 1) / 2);
            let mode_bytes = serde_json::to_vec(&semantic_state.modes).map_err(|error| {
                return terminal_error("serialize activation modes", error.to_string());
            })?;
            let terminal_modes_hash = blake3::hash(&mode_bytes).to_hex().to_string();
            let reporting_enabled = semantic_state.modes.values.iter().any(|mode| {
                return !mode.ansi && matches!(mode.number, 9 | 1000 | 1002 | 1003) && mode.enabled;
            });
            if !reporting_enabled {
                if *strict {
                    return Err(control_rejected(
                        "pointer_reporting_disabled",
                        "the application has not enabled a DEC mouse tracking mode",
                    ));
                }
                return Ok(ControlData::InputNoop {
                    input_kind: InputKind::Pointer,
                    reason: "pointer_reporting_disabled".to_owned(),
                    terminal_modes_hash,
                    boundaries: actor_boundaries(event_log),
                });
            }
            let mut encoded = Vec::new();
            for action in [MouseAction::Press, MouseAction::Release] {
                encoded.extend_from_slice(
                    live.semantic
                        .encode_mouse(
                            action,
                            MouseButton::Left,
                            target_column,
                            region.row,
                            live.geometry.cols,
                            live.geometry.rows,
                            live.geometry.pixel_width,
                            live.geometry.pixel_height,
                        )?
                        .as_bytes(),
                );
            }
            if encoded.is_empty() {
                if *strict {
                    return Err(control_rejected(
                        "pointer_has_no_output",
                        "the current tracking mode suppressed the activation click",
                    ));
                }
                return Ok(ControlData::InputNoop {
                    input_kind: InputKind::Pointer,
                    reason: "pointer_has_no_output".to_owned(),
                    terminal_modes_hash,
                    boundaries: actor_boundaries(event_log),
                });
            }
            let applied = apply_input(
                spawned.writer.as_mut(),
                event_log,
                request_id,
                InputSource::Agent,
                InputKind::Pointer,
                InputDescriptor::Activate {
                    text: text.clone(),
                    start_column: region.start_column,
                    end_column: region.end_column,
                    row: region.row,
                    target_column,
                    presented_state_hash: state_hash.clone(),
                    cols: live.geometry.cols,
                    rows: live.geometry.rows,
                    encoder: "libghostty-mouse-encoder-v1".to_owned(),
                    terminal_modes_hash: terminal_modes_hash.clone(),
                },
                Some(terminal_modes_hash),
                Some("activate-visible-text".to_owned()),
                None,
                &encoded,
            )?;
            return Ok(ControlData::Activated {
                operation_id: request_id,
                text: text.clone(),
                start_column: region.start_column,
                end_column: region.end_column,
                row: region.row,
                target_column,
                presented_state_hash: state_hash,
                data: ByteString::new(encoded),
                bytes_written: applied.bytes_written,
                intent_event_seq: applied.intent_event_seq,
                result_event_seq: applied.result_event_seq,
                boundaries: actor_boundaries(event_log),
            });
        }
        ControlCommand::Resize {
            cols,
            rows,
            pixel_width,
            pixel_height,
        } => {
            let geometry = TerminalGeometry {
                cols: *cols,
                rows: *rows,
                pixel_width: *pixel_width,
                pixel_height: *pixel_height,
            };
            validate_control_geometry(geometry)?;
            apply_resize(spawned.master.as_mut(), event_log, request_id, geometry)?;
            resize_semantic(&mut live.semantic, geometry)?;
            live.geometry = geometry;
            let latest = live.semantic.snapshot()?;
            if !latest.modes.synchronized_output {
                live.presented = latest;
            }
            return Ok(ControlData::Acknowledged {
                boundaries: actor_boundaries(event_log),
            });
        }
        ControlCommand::Signal { signal } => {
            apply_signal(spawned.pid, event_log, request_id, *signal, false)?;
            return Ok(ControlData::Acknowledged {
                boundaries: actor_boundaries(event_log),
            });
        }
        ControlCommand::Mark { label } => {
            let label = label.trim();
            if label.is_empty() || label.len() > 256 {
                return Err(control_rejected(
                    "invalid_marker",
                    "marker must contain 1 to 256 UTF-8 bytes",
                ));
            }
            let event_seq = event_log.append(Event::Marker {
                operation_id: request_id,
                label: label.to_owned(),
            })?;
            return Ok(ControlData::Marked {
                label: label.to_owned(),
                event_seq,
                boundaries: actor_boundaries(event_log),
            });
        }
        ControlCommand::Stop => {
            apply_signal(
                spawned.pid,
                event_log,
                request_id,
                ProcessSignal::Terminate,
                false,
            )?;
            return Ok(ControlData::Acknowledged {
                boundaries: actor_boundaries(event_log),
            });
        }
    }
}

fn validate_control_geometry(geometry: TerminalGeometry) -> Result<()> {
    if supported_geometry_cell_count(geometry.cols, geometry.rows).is_none()
        || geometry.pixel_width == 0
        || geometry.pixel_height == 0
    {
        return Err(control_rejected(
            "invalid_geometry",
            "terminal geometry exceeds the supported row, column, or cell limit, or has zero pixels",
        ));
    }
    return Ok(());
}

pub(in crate::record) fn apply_signal(
    pid: Option<u32>,
    event_log: &mut EventLogWriter,
    operation_id: Uuid,
    signal: ProcessSignal,
    missing_is_ok: bool,
) -> Result<()> {
    let (signal, name) = nix_signal(signal);
    event_log.append(Event::SignalIntent {
        operation_id,
        number: signal as i32,
        name: name.to_owned(),
    })?;
    let signal_result = pid
        .and_then(|value| return i32::try_from(value).ok())
        .ok_or(Errno::ESRCH)
        .and_then(|value| return killpg(Pid::from_raw(value), signal));
    let status = if signal_result.is_ok() {
        OperationStatus::Applied
    } else {
        OperationStatus::Failed
    };
    let result_write = event_log
        .append(Event::SignalResult {
            operation_id,
            status,
            error_code: signal_result
                .as_ref()
                .err()
                .map(|error| format!("os_errno_{}", *error as i32)),
        })
        .map(|_| ());
    let Some(error) = signal_result.err() else {
        return result_write;
    };
    if missing_is_ok && error == Errno::ESRCH {
        return result_write;
    }
    return finish_with_cleanup(
        Err(control_rejected(
            "signal_failed",
            &format!("signal child process group failed: {error}"),
        )),
        result_write,
    );
}
pub(super) fn screen_frame_error_code(error: &ScreenFrameError) -> &'static str {
    return match error {
        ScreenFrameError::InvalidRegion { .. } => "invalid_screen_region",
        ScreenFrameError::ResourceLimit { .. } => "screen_frame_resource_limit",
        ScreenFrameError::InconsistentSemanticState { .. } | ScreenFrameError::Serialization => {
            "screen_frame_projection_failed"
        }
    };
}

fn screen_frame_error_response(
    request_id: Uuid,
    error: &ScreenFrameError,
    current_state_hash: String,
    event_log: &EventLogWriter,
    scope: &str,
) -> ControlResponse {
    return ControlResponse {
        version: CONTROL_PROTOCOL_VERSION,
        request_id,
        result: ControlResult::Error {
            code: screen_frame_error_code(error).to_owned(),
            message: error.to_string(),
            details: Some(ControlErrorDetails {
                current_presented_state_hash: current_state_hash,
                boundaries: actor_boundaries(event_log),
                scope: scope.to_owned(),
                input_sequence: None,
                screen_frame: Some(screen_frame_failure_details(error)),
            }),
        },
    };
}

pub(super) fn screen_frame_failure_details(error: &ScreenFrameError) -> ScreenFrameFailureDetails {
    return match error {
        ScreenFrameError::InvalidRegion {
            region,
            cols,
            rows,
            reason,
        } => ScreenFrameFailureDetails {
            detail: None,
            region: *region,
            region_reason: Some(*reason),
            cols: Some(*cols),
            rows: Some(*rows),
            limit_kind: None,
            requested: None,
            maximum: None,
        },
        ScreenFrameError::ResourceLimit {
            limit,
            requested,
            maximum,
            detail,
            region,
        } => ScreenFrameFailureDetails {
            detail: Some(*detail),
            region: Some(*region),
            region_reason: None,
            cols: None,
            rows: None,
            limit_kind: Some(
                match limit {
                    ScreenFrameLimitKind::CellCount => "cell_count",
                    ScreenFrameLimitKind::SerializedBytes => "serialized_bytes",
                    ScreenFrameLimitKind::ResponseBytes => "response_bytes",
                }
                .to_owned(),
            ),
            requested: Some(*requested),
            maximum: Some(*maximum),
        },
        ScreenFrameError::InconsistentSemanticState { .. } | ScreenFrameError::Serialization => {
            ScreenFrameFailureDetails {
                detail: None,
                region: None,
                region_reason: None,
                cols: None,
                rows: None,
                limit_kind: None,
                requested: None,
                maximum: None,
            }
        }
    };
}

fn nix_signal(signal: ProcessSignal) -> (Signal, &'static str) {
    match signal {
        ProcessSignal::Interrupt => return (Signal::SIGINT, "SIGINT"),
        ProcessSignal::Terminate => return (Signal::SIGTERM, "SIGTERM"),
        ProcessSignal::Hangup => return (Signal::SIGHUP, "SIGHUP"),
        ProcessSignal::User1 => return (Signal::SIGUSR1, "SIGUSR1"),
        ProcessSignal::User2 => return (Signal::SIGUSR2, "SIGUSR2"),
    }
}

fn control_error_code(error: &Error) -> &str {
    match error {
        Error::ControlRejected { code, .. } => return code,
        Error::InputSequenceIncomplete { .. } => return "input_sequence_incomplete",
        Error::ScreenFrame(ScreenFrameError::InvalidRegion { .. }) => {
            return "invalid_screen_region";
        }
        Error::ScreenFrame(ScreenFrameError::ResourceLimit { .. }) => {
            return "screen_frame_resource_limit";
        }
        Error::ScreenFrame(_) => return "screen_frame_projection_failed",
        Error::InputWriteFailed { .. } => return "input_write_failed",
        Error::GhosttyVt { operation, .. } if operation.contains("key") => {
            return "key_encoding_failed";
        }
        Error::GhosttyVt { operation, .. } if operation.contains("mouse") => {
            return "pointer_encoding_failed";
        }
        Error::GhosttyVt { .. } => return "terminal_encoding_failed",
        Error::EventLogPoisoned { .. } | Error::InvalidEventLog { .. } => return "event_log_failed",
        Error::Filesystem { .. }
        | Error::ControlUnavailable { .. }
        | Error::ControlTimeout { .. }
        | Error::ControlTransport { .. } => return "io_failed",
        Error::TerminalOperation { .. } => return "terminal_operation_failed",
        _ => return "internal_error",
    }
}
