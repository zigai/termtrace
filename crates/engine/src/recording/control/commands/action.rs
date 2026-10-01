use super::{
    actor_boundaries, control_error_code, control_error_response, evaluate_action_postcondition,
    mpsc, mutation_failure_effect, presented_state_hash, screen_frame_error, ActionInputResult,
    AdmittedAction, AdmittedPostcondition, ControlCommand, ControlErrorDetails, ControlPayload,
    ControlRequest, ControlResponse, ControlResult, Duration, Error, EventLogWriter,
    InputSequenceExecution, InputSequenceFailureDetails, Instant, MutationEffect, Result,
    SessionActor, Uuid, CONTROL_PROTOCOL_VERSION,
};

pub(in crate::recording) struct PendingInputSequence {
    pub(super) request: ControlRequest,
    pub(super) response_sender: mpsc::SyncSender<ControlResponse>,
    pub(super) command_hash: String,
    pub(super) event_start_seq: u64,
    pub(super) phase: InputSequencePhase,
}

pub(super) enum InputSequencePhase {
    Applying {
        execution: InputSequenceExecution,
        action: Option<AdmittedAction>,
    },
    Waiting(WaitingAction),
}

pub(super) struct WaitingAction {
    pub(super) input: ActionInputResult,
    pub(super) action: AdmittedAction,
    pub(super) started_at: Instant,
    pub(super) matching_state: Option<(String, Instant)>,
}

impl PendingInputSequence {
    pub(in crate::recording) fn next_deadline(&self) -> Instant {
        return match &self.phase {
            InputSequencePhase::Applying { execution, .. } => execution.next_deadline(),
            InputSequencePhase::Waiting(waiting) => match &waiting.action.postcondition {
                AdmittedPostcondition::None => Instant::now(),
                AdmittedPostcondition::Idle {
                    stable_for_ns,
                    timeout_ns,
                } => {
                    let deadline = waiting.started_at + Duration::from_nanos(*timeout_ns);

                    return waiting
                        .matching_state
                        .as_ref()
                        .map_or(waiting.started_at, |(_, since)| {
                            return *since + Duration::from_nanos(*stable_for_ns);
                        })
                        .min(deadline);
                }
                AdmittedPostcondition::Screen { timeout_ns, .. } => {
                    return waiting.started_at + Duration::from_nanos(*timeout_ns);
                }
            },
        };
    }

    pub(super) fn exclusive(&self) -> bool {
        return !matches!(
            &self.phase,
            InputSequencePhase::Applying { action: None, .. }
        );
    }
}

impl SessionActor<'_> {
    pub(in crate::recording) fn advance_input_sequence(
        &mut self,
        child_exited: bool,
        output_closed: bool,
    ) -> Result<()> {
        let Some(mut pending) = self.pending_input_sequence.take() else {
            return Ok(());
        };
        let is_applying = matches!(&pending.phase, InputSequencePhase::Applying { .. });
        if is_applying && !output_closed && Instant::now() < pending.next_deadline() {
            self.pending_input_sequence = Some(pending);

            return Ok(());
        }

        let terminal_outcome = loop {
            match pending.phase {
                InputSequencePhase::Applying {
                    mut execution,
                    action,
                } => {
                    match execution.advance(
                        self.spawned.writer.as_mut(),
                        &mut self.event_log,
                        &mut self.live.semantic,
                    ) {
                        Ok(None) => {
                            pending.phase = InputSequencePhase::Applying { execution, action };
                            self.pending_input_sequence = Some(pending);

                            return Ok(());
                        }
                        Ok(Some(input)) => {
                            if let Some(action) = action {
                                pending.phase = InputSequencePhase::Waiting(WaitingAction {
                                    input,
                                    action,
                                    started_at: Instant::now(),
                                    matching_state: None,
                                });
                            } else {
                                break Ok(ControlPayload::InputSequenceApplied {
                                    sequence_id: input.sequence_id,
                                    atom_count: input.atom_count,
                                    bytes_written: input.bytes_written,
                                    first_intent_event_seq: input.first_intent_event_seq,
                                    last_result_event_seq: input.last_result_event_seq,
                                    pace_ns: input.pace_ns,
                                    atoms: input.atoms,
                                    boundaries: actor_boundaries(&self.event_log),
                                });
                            }
                        }
                        Err(error) => {
                            let recorded_events =
                                self.event_log.next_seq() != pending.event_start_seq;
                            let effect = mutation_failure_effect(&error, recorded_events);

                            break Err((error, effect));
                        }
                    }
                }
                InputSequencePhase::Waiting(mut waiting) => {
                    match evaluate_action_postcondition(
                        &mut waiting,
                        &self.event_log,
                        self.session_id,
                        &self.live,
                        child_exited,
                        output_closed,
                    ) {
                        Ok(Some(postcondition)) => {
                            break Ok(ControlPayload::ActionApplied {
                                input: waiting.input,
                                postcondition,
                                boundaries: actor_boundaries(&self.event_log),
                            });
                        }
                        Ok(None) => {
                            pending.phase = InputSequencePhase::Waiting(waiting);
                            self.pending_input_sequence = Some(pending);

                            return Ok(());
                        }
                        Err(error) => break Err((error, MutationEffect::Applied)),
                    }
                }
            }
        };
        let current_state_hash = presented_state_hash(&self.live)?;
        let atom_count = match &pending.request.command {
            ControlCommand::InputSequence { atoms, .. } | ControlCommand::Act { atoms, .. } => {
                u32::try_from(atoms.len()).unwrap_or(u32::MAX)
            }
            _ => 0,
        };

        let (response, terminal_error) = match terminal_outcome {
            Ok(data) => (
                ControlResponse {
                    version: CONTROL_PROTOCOL_VERSION,
                    request_id: pending.request.request_id,
                    result: ControlResult::Ok { data },
                },
                None,
            ),
            Err((error, effect)) => {
                let response = input_sequence_error_response(
                    pending.request.request_id,
                    atom_count,
                    &error,
                    effect,
                    current_state_hash,
                    &self.event_log,
                );

                (response, Some(error))
            }
        };
        self.mutation_history.remember(
            pending.request.request_id,
            &pending.request.command,
            pending.command_hash,
            pending.event_start_seq,
            self.event_log.next_seq(),
            self.event_log.current_hash(),
            response.clone(),
        )?;

        if pending.response_sender.send(response).is_err() {
            tracing::warn!(
                request_id = %pending.request.request_id,
                "input sequence response receiver disconnected"
            );
        }

        return match terminal_error {
            None
            | Some(
                Error::ControlRejected { .. }
                | Error::InputSequenceIncomplete { .. }
                | Error::GhosttyVt { .. }
                | Error::InvalidTerminalText { .. }
                | Error::ScreenFrame(_),
            ) => Ok(()),
            Some(error) => Err(error),
        };
    }
}

pub(super) fn input_sequence_error_response(
    request_id: Uuid,
    atom_count: u32,
    outcome: &Error,
    effect: MutationEffect,
    current_state_hash: String,
    event_log: &EventLogWriter,
) -> ControlResponse {
    return match outcome {
        Error::InputSequenceIncomplete {
            sequence_id,
            failed_atom_index,
            failed_operation_id,
            atoms_applied,
            bytes_written,
            failed_atom_bytes_written,
            first_intent_event_seq,
            last_result_event_seq,
            reason,
        } => ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id,
            result: ControlResult::Error {
                code: "input_sequence_incomplete".to_owned(),
                message: format!(
                    "input sequence `{sequence_id}` stopped at atom {failed_atom_index}: {reason}"
                ),
                effect,
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
        Error::ScreenFrame(error) => screen_frame_error(
            request_id,
            error,
            current_state_hash,
            event_log,
            "action_terminating_screen_frame",
            effect,
        ),
        Error::ControlRejected { code, message } => ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id,
            result: ControlResult::Error {
                code: code.clone(),
                message: message.clone(),
                effect,
                details: Some(ControlErrorDetails {
                    current_presented_state_hash: current_state_hash,
                    boundaries: actor_boundaries(event_log),
                    scope: "input_sequence_encoding".to_owned(),
                    input_sequence: None,
                    screen_frame: None,
                }),
            },
        },
        error => control_error_response(
            request_id,
            control_error_code(error),
            error.to_string(),
            effect,
        ),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::control::compile_screen_predicate;
    use std::sync::mpsc;
    use termtrace_model::{
        protocol::control::{ActionInputResult, ScreenPredicate},
        terminal::screen::{FrameDetail, ScreenProjectionRequest},
    };

    fn test_waiting_action(
        postcondition: AdmittedPostcondition,
        started_at: Instant,
        matching_state: Option<(String, Instant)>,
    ) -> WaitingAction {
        return WaitingAction {
            input: ActionInputResult {
                sequence_id: Uuid::nil(),
                atom_count: 1,
                bytes_written: 4,
                first_intent_event_seq: 1,
                last_result_event_seq: 2,
                pace_ns: 0,
                atoms: Vec::new(),
            },
            action: AdmittedAction {
                postcondition,
                projection: ScreenProjectionRequest {
                    detail: FrameDetail::None,
                    region: None,
                },
            },
            started_at,
            matching_state,
        };
    }

    fn test_pending(phase: InputSequencePhase) -> PendingInputSequence {
        let (sender, _) = mpsc::sync_channel(1);

        return PendingInputSequence {
            request: ControlRequest::new(ControlCommand::Snapshot),
            response_sender: sender,
            command_hash: "test".to_owned(),
            event_start_seq: 1,
            phase,
        };
    }

    #[test]
    fn test_waiting_idle_next_deadline_samples_immediately_when_unmatched() {
        let started_at = Instant::now();
        let pending = test_pending(InputSequencePhase::Waiting(test_waiting_action(
            AdmittedPostcondition::Idle {
                stable_for_ns: 100_000_000,
                timeout_ns: 5_000_000_000,
            },
            started_at,
            None,
        )));
        assert_eq!(pending.next_deadline(), started_at);
    }

    #[test]
    fn test_waiting_idle_next_deadline_respects_stability_and_timeout() {
        let started_at = Instant::now();
        let since = started_at + Duration::from_millis(50);
        let stable_for_ns = 200_000_000;
        let timeout_ns = 1_000_000_000;

        let pending = test_pending(InputSequencePhase::Waiting(test_waiting_action(
            AdmittedPostcondition::Idle {
                stable_for_ns,
                timeout_ns,
            },
            started_at,
            Some(("hash_a".to_owned(), since)),
        )));
        assert_eq!(
            pending.next_deadline(),
            since + Duration::from_nanos(stable_for_ns)
        );

        let late_since = started_at + Duration::from_millis(900);
        let pending_capped = test_pending(InputSequencePhase::Waiting(test_waiting_action(
            AdmittedPostcondition::Idle {
                stable_for_ns,
                timeout_ns,
            },
            started_at,
            Some(("hash_b".to_owned(), late_since)),
        )));
        assert_eq!(
            pending_capped.next_deadline(),
            started_at + Duration::from_nanos(timeout_ns)
        );
    }

    #[test]
    fn test_waiting_screen_next_deadline_is_timeout() {
        let started_at = Instant::now();
        let timeout_ns = 3_000_000_000;
        let predicate = compile_screen_predicate(&ScreenPredicate::Contains {
            text: "done".to_owned(),
        })
        .expect("valid test predicate");

        let pending = test_pending(InputSequencePhase::Waiting(test_waiting_action(
            AdmittedPostcondition::Screen {
                predicate,
                timeout_ns,
            },
            started_at,
            None,
        )));
        assert_eq!(
            pending.next_deadline(),
            started_at + Duration::from_nanos(timeout_ns)
        );
    }

    #[test]
    fn test_pacing_gate_does_not_block_waiting_phase() {
        let started_at = Instant::now();
        let pending = test_pending(InputSequencePhase::Waiting(test_waiting_action(
            AdmittedPostcondition::Idle {
                stable_for_ns: 100_000_000,
                timeout_ns: 10_000_000_000,
            },
            started_at,
            Some(("hash_1".to_owned(), started_at)),
        )));

        let is_applying = matches!(&pending.phase, InputSequencePhase::Applying { .. });
        let output_closed = false;
        let blocked = is_applying && !output_closed && Instant::now() < pending.next_deadline();

        assert!(!blocked);
    }
}
