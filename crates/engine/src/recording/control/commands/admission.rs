use super::*;

impl SessionActor<'_> {
    pub(in crate::recording) fn handle_control_request(
        &mut self,
        request: ControlRequest,
        response_sender: mpsc::SyncSender<ControlResponse>,
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
        if self
            .pending_input_sequence
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
            if let Err(error) = validate_screen_region(
                projection,
                self.live.presented.cols,
                self.live.presented.rows,
            ) {
                let response = screen_frame_error_response(
                    request.request_id,
                    &error,
                    presented_state_hash(&self.live)?,
                    &self.event_log,
                    "screen_wait_admission",
                );
                let _ = response_sender.send(response);
                return Ok(());
            }
            if self.screen_waiters.len() >= MAX_SCREEN_WAITERS {
                let response = control_error_response(
                    request.request_id,
                    "waiter_limit_reached",
                    "session screen waiter limit has been reached",
                );
                let _ = response_sender.send(response);
                return Ok(());
            }
            self.screen_waiters.push(ScreenWaiter {
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
            if let Err(error) = validate_screen_region(
                projection,
                self.live.presented.cols,
                self.live.presented.rows,
            ) {
                let response = screen_frame_error_response(
                    request.request_id,
                    &error,
                    presented_state_hash(&self.live)?,
                    &self.event_log,
                    "stable_capture_admission",
                );
                let _ = response_sender.send(response);
                return Ok(());
            }
            if self.screen_waiters.len() >= MAX_SCREEN_WAITERS {
                let response = control_error_response(
                    request.request_id,
                    "waiter_limit_reached",
                    "session screen waiter limit has been reached",
                );
                let _ = response_sender.send(response);
                return Ok(());
            }
            self.screen_waiters.push(ScreenWaiter {
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
        if self.pending_input_sequence.is_some() && !read_only {
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
            let (hash, prior) = self.mutation_history.lookup(
                request.request_id,
                &request.command,
                request.expected_presented_state_hash.as_deref(),
            )?;
            if let Some(response) = prior {
                let _ = response_sender.send(response);
                return Ok(());
            }
            if self.mutation_history.is_full() {
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

        let event_start_seq = self.event_log.next_seq();
        let current_state_hash = presented_state_hash(&self.live)?;
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
            if let Some((atoms, pace_ns, include_atom_outcomes, postcondition, projection)) =
                sequence
            {
                let validated_postcondition = postcondition
                    .as_ref()
                    .map(validate_action_postcondition)
                    .transpose()
                    .and_then(|_| {
                        if let Some(projection) = &projection {
                            validate_screen_region(
                                projection,
                                self.live.presented.cols,
                                self.live.presented.rows,
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
                        self.pending_input_sequence = Some(PendingInputSequence {
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
                                &self.event_log,
                                "action_admission",
                            ),
                            _ => control_error_response(
                                request.request_id,
                                control_error_code(&error),
                                error.to_string(),
                            ),
                        };
                        if let Some(command_hash) = command_hash {
                            self.mutation_history.remember(
                                request.request_id,
                                &request.command,
                                command_hash,
                                event_start_seq,
                                self.event_log.next_seq(),
                                self.event_log.current_hash(),
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
                message: format!(
                    "presented state changed; current state hash is {current_state_hash}"
                ),
            })
        } else {
            self.execute_control_command(request.request_id, &request.command)
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
                        boundaries: actor_boundaries(&self.event_log),
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
                            boundaries: actor_boundaries(&self.event_log),
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
                        boundaries: actor_boundaries(&self.event_log),
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
            self.mutation_history.remember(
                request.request_id,
                &request.command,
                command_hash,
                event_start_seq,
                self.event_log.next_seq(),
                self.event_log.current_hash(),
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
}
