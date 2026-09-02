use super::*;

pub(in crate::recording) struct PendingInputSequence {
    pub(super) request: ControlRequest,
    pub(super) response_sender: mpsc::SyncSender<ControlResponse>,
    pub(super) command_hash: String,
    pub(super) event_start_seq: u64,
    pub(super) execution: InputSequenceExecution,
    pub(super) postcondition: Option<ActionPostcondition>,
    pub(super) projection: Option<ScreenProjectionRequest>,
    pub(super) input: Option<ActionInputResult>,
    pub(super) postcondition_started_at: Option<Instant>,
    pub(super) matching_state: Option<(String, Instant)>,
}

impl PendingInputSequence {
    pub(in crate::recording) fn next_deadline(&self) -> Instant {
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

    pub(super) fn exclusive(&self) -> bool {
        return self.postcondition.is_some();
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
        let screen_postcondition_ready = pending.input.is_some()
            && matches!(
                &pending.postcondition,
                Some(ActionPostcondition::Screen { .. })
            );
        if !screen_postcondition_ready && !output_closed && Instant::now() < pending.next_deadline()
        {
            self.pending_input_sequence = Some(pending);
            return Ok(());
        }
        let terminal_outcome = if pending.input.is_some() {
            match evaluate_action_postcondition(
                &mut pending,
                &self.event_log,
                self.session_id,
                &self.live,
                child_exited,
                output_closed,
            ) {
                Ok(Some(data)) => Ok(data),
                Ok(None) => {
                    self.pending_input_sequence = Some(pending);
                    return Ok(());
                }
                Err(error) => Err(error),
            }
        } else {
            match pending.execution.advance(
                self.spawned.writer.as_mut(),
                &mut self.event_log,
                &mut self.live.semantic,
            ) {
                Ok(None) => {
                    self.pending_input_sequence = Some(pending);
                    return Ok(());
                }
                Ok(Some(data)) if pending.postcondition.is_some() => {
                    let input = action_input_result(data)?;
                    pending.input = Some(input);
                    pending.postcondition_started_at = Some(Instant::now());
                    match evaluate_action_postcondition(
                        &mut pending,
                        &self.event_log,
                        self.session_id,
                        &self.live,
                        child_exited,
                        output_closed,
                    ) {
                        Ok(Some(data)) => Ok(data),
                        Ok(None) => {
                            self.pending_input_sequence = Some(pending);
                            return Ok(());
                        }
                        Err(error) => Err(error),
                    }
                }
                Ok(Some(data)) => Ok(data),
                Err(error) => Err(error),
            }
        };
        let current_state_hash = presented_state_hash(&self.live)?;
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
            &self.event_log,
        );
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
}
pub(super) fn action_input_result(data: ControlData) -> Result<ActionInputResult> {
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
pub(super) fn input_sequence_response(
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
