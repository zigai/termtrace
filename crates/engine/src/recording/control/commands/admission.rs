use super::{
    action, actor_boundaries, admitted_postcondition, control_error_code, control_error_response,
    control_rejected, deliver_response, is_lowercase_blake3_hex, mpsc, mutation_failure_effect,
    presented_state_hash, reaction_window, screen_frame_error, terminal_error,
    validate_screen_region, AdmittedAction, ControlCommand, ControlErrorDetails, ControlRequest,
    ControlResponse, ControlResult, Duration, Error, InputSequenceExecution, Instant,
    MutationEffect, PendingInputSequence, Result, ScreenWaiter, SessionActor, WaiterReaction,
    CONTROL_PROTOCOL_VERSION, MAX_SCREEN_WAITERS,
};
use termtrace_model::terminal::screen::screen_match_text;
use termtrace_model::{execution::event::InputKind, protocol::control::InputSequenceAtom};

impl SessionActor<'_> {
    pub(in crate::recording) fn handle_control_request(
        &mut self,
        request: ControlRequest,
        response_sender: mpsc::SyncSender<ControlResponse>,
    ) -> Result<()> {
        let read_only = !request.command.is_mutation();

        if request.expected_presented_state_hash.is_some() && read_only {
            let response = control_error_response(
                request.request_id,
                "invalid_observation_guard",
                "presented-state guards are only valid for mutations",
                MutationEffect::NotApplied,
            );

            deliver_response(&response_sender, response);
            return Ok(());
        }

        let validated = match request.command.validate() {
            Ok(validated) => validated,
            Err(error) => {
                let response = control_error_response(
                    request.request_id,
                    error.code(),
                    error.to_string(),
                    MutationEffect::NotApplied,
                );

                deliver_response(&response_sender, response);
                return Ok(());
            }
        };

        let command_hash = if read_only {
            None
        } else {
            let (hash, prior) = self.mutation_history.lookup(
                request.request_id,
                &request.command,
                request.expected_presented_state_hash.as_deref(),
            )?;
            if let Some(response) = prior {
                deliver_response(&response_sender, response);
                return Ok(());
            }

            if let Some(response) = self.pending_outer_resize_retry(&request, &hash) {
                deliver_response(&response_sender, response);
                return Ok(());
            }

            if self.mutation_history.is_full() {
                let response = control_error_response(
                    request.request_id,
                    "mutation_limit_reached",
                    "session mutation operation limit has been reached",
                    MutationEffect::NotApplied,
                );

                deliver_response(&response_sender, response);
                return Ok(());
            }

            Some(hash)
        };

        let stopping = matches!(request.command, ControlCommand::Stop);
        if !read_only
            && !stopping
            && self
                .pending_input_sequence
                .as_ref()
                .is_some_and(PendingInputSequence::exclusive)
        {
            let response = control_error_response(
                request.request_id,
                "action_busy",
                "an actor-ordered action is still applying input or evaluating its postcondition",
                MutationEffect::NotApplied,
            );

            deliver_response(&response_sender, response);
            return Ok(());
        }

        if let ControlCommand::WaitScreen {
            stable_for_ns,
            timeout_ns,
            projection,
            ..
        } = &request.command
        {
            if let Err(error) = validate_screen_region(
                projection,
                self.live.presented.cols,
                self.live.presented.rows,
            ) {
                let response = screen_frame_error(
                    request.request_id,
                    &error,
                    presented_state_hash(&self.live)?,
                    &self.event_log,
                    "screen_wait_admission",
                    MutationEffect::NotApplied,
                );

                deliver_response(&response_sender, response);
                return Ok(());
            }

            if self.screen_waiters.len() >= MAX_SCREEN_WAITERS {
                let response = control_error_response(
                    request.request_id,
                    "waiter_limit_reached",
                    "session screen waiter limit has been reached",
                    MutationEffect::NotApplied,
                );

                deliver_response(&response_sender, response);
                return Ok(());
            }

            self.screen_waiters.push(ScreenWaiter {
                request_id: request.request_id,
                predicate: validated.into_predicate(),
                stable_for: Duration::from_nanos(*stable_for_ns),
                deadline: Instant::now() + Duration::from_nanos(*timeout_ns),
                matching_state: None,
                reaction: None,
                projection: *projection,
                response: response_sender,
            });
            return Ok(());
        }

        if let ControlCommand::CaptureStable {
            stable_for_ns,
            timeout_ns,
            projection,
            reaction,
        } = &request.command
        {
            if let Err(error) = validate_screen_region(
                projection,
                self.live.presented.cols,
                self.live.presented.rows,
            ) {
                let response = screen_frame_error(
                    request.request_id,
                    &error,
                    presented_state_hash(&self.live)?,
                    &self.event_log,
                    "stable_capture_admission",
                    MutationEffect::NotApplied,
                );

                deliver_response(&response_sender, response);
                return Ok(());
            }

            if self.screen_waiters.len() >= MAX_SCREEN_WAITERS {
                let response = control_error_response(
                    request.request_id,
                    "waiter_limit_reached",
                    "session screen waiter limit has been reached",
                    MutationEffect::NotApplied,
                );

                deliver_response(&response_sender, response);
                return Ok(());
            }

            let now = Instant::now();
            let reaction = match reaction {
                Some(reaction) => Some(WaiterReaction {
                    changed_from: match &reaction.changed_from {
                        Some(hash) => hash.clone(),
                        None => presented_state_hash(&self.live)?,
                    },
                    output_bytes: self.progress.output_bytes,
                    deadline: now + reaction_window(*stable_for_ns, reaction.timeout_ns),
                }),
                None => None,
            };
            self.screen_waiters.push(ScreenWaiter {
                request_id: request.request_id,
                predicate: None,
                stable_for: Duration::from_nanos(*stable_for_ns),
                deadline: now + Duration::from_nanos(*timeout_ns),
                matching_state: None,
                reaction,
                projection: *projection,
                response: response_sender,
            });
            return Ok(());
        }

        if self.pending_input_sequence.is_some() && !read_only && !stopping {
            let response = control_error_response(
                request.request_id,
                "input_sequence_busy",
                "another paced input sequence is still in progress",
                MutationEffect::NotApplied,
            );

            deliver_response(&response_sender, response);
            return Ok(());
        }

        let event_start_seq = self.event_log.next_seq();
        let current_state_hash = presented_state_hash(&self.live)?;
        let guard = request.expected_presented_state_hash.as_deref();
        let stale = guard.is_some_and(|hash| {
            return is_lowercase_blake3_hex(hash) && hash != current_state_hash;
        });
        let invalid_guard = guard.is_some_and(|hash| return !is_lowercase_blake3_hex(hash));

        if !invalid_guard && !stale {
            if stopping {
                self.interrupt_input_sequence(
                    "action_stopped",
                    "stop preempted the pending action",
                )?;
            }

            let sequence = match &request.command {
                ControlCommand::InputSequence {
                    atoms,
                    pace_ns,
                    include_atom_outcomes,
                } => Some((atoms, *pace_ns, *include_atom_outcomes, None)),
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
                    Some((postcondition, projection)),
                )),
                _ => None,
            };

            if let Some((atoms, pace_ns, include_atom_outcomes, action)) = sequence {
                let admitted_action = action
                    .map(|(postcondition, projection)| {
                        let postcondition =
                            admitted_postcondition(postcondition, validated.into_predicate())?;
                        let excluded_matches =
                            if let super::AdmittedPostcondition::Screen { predicate, .. } =
                                &postcondition
                            {
                                let (before, _) = screen_match_text(&self.live.presented);
                                let mut typed = String::new();
                                let mut atom_matches = 0;
                                for atom in atoms {
                                    match atom {
                                        InputSequenceAtom::Bytes {
                                            data,
                                            input_kind: InputKind::Text,
                                            ..
                                        }
                                        | InputSequenceAtom::Paste { data, .. } => {
                                            let text = String::from_utf8_lossy(data.as_bytes());
                                            atom_matches += predicate
                                                .text_matches(&text)
                                                .map_or(0, |matches| return matches.len());

                                            typed.push_str(&text);
                                        }
                                        _ => {}
                                    }
                                }

                                predicate.minimum_text_matches().map_or_else(
                                    || {
                                        return predicate
                                            .text_matches(&before)
                                            .map_or(0, |matches| return matches.len())
                                            + atom_matches.max(
                                                predicate
                                                    .text_matches(&typed)
                                                    .map_or(0, |matches| return matches.len()),
                                            );
                                    },
                                    |minimum| return minimum.saturating_sub(1),
                                )
                            } else {
                                0
                            };

                        validate_screen_region(
                            projection,
                            self.live.presented.cols,
                            self.live.presented.rows,
                        )
                        .map_err(Error::ScreenFrame)?;

                        return Ok(AdmittedAction {
                            postcondition,
                            projection: *projection,
                            baseline_state_hash: current_state_hash.clone(),
                            baseline_output_bytes: self.progress.output_bytes,
                            excluded_matches,
                        });
                    })
                    .transpose();

                let execution = admitted_action.and_then(|action| {
                    let execution = InputSequenceExecution::new(
                        request.request_id,
                        atoms,
                        pace_ns,
                        include_atom_outcomes,
                    )?;

                    self.mutation_history
                        .reserve_command(request.request_id, &request.command)?;

                    return Ok((execution, action));
                });

                match execution {
                    Ok((execution, action)) => {
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
                            phase: action::InputSequencePhase::Applying { execution, action },
                        });

                        return Ok(());
                    }
                    Err(error) => {
                        let response = match &error {
                            Error::ScreenFrame(error) => screen_frame_error(
                                request.request_id,
                                error,
                                current_state_hash,
                                &self.event_log,
                                "action_admission",
                                MutationEffect::NotApplied,
                            ),
                            _ => control_error_response(
                                request.request_id,
                                control_error_code(&error),
                                error.to_string(),
                                MutationEffect::NotApplied,
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

                        deliver_response(&response_sender, response);

                        return match error {
                            Error::ControlRejected { .. } | Error::ScreenFrame(_) => Ok(()),
                            error => Err(error),
                        };
                    }
                }
            }
        }

        if !invalid_guard
            && !stale
            && self.owns_outer_resize()
            && matches!(request.command, ControlCommand::Resize { .. })
        {
            let command_hash = command_hash.ok_or_else(|| {
                return terminal_error(
                    "schedule outer-terminal resize",
                    "missing mutation command hash",
                );
            })?;

            return self.admit_outer_resize(request, command_hash, response_sender);
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
        } else if read_only {
            self.execute_control_command(request.request_id, &validated)
        } else {
            self.mutation_history
                .reserve_command(request.request_id, &request.command)
                .and_then(|()| {
                    return self.execute_control_command(request.request_id, &validated);
                })
        };

        let effect = match &outcome {
            Err(error) if !read_only => {
                mutation_failure_effect(error, self.event_log.next_seq() != event_start_seq)
            }
            _ => MutationEffect::NotApplied,
        };

        let response = match (&outcome, stale) {
            (Err(Error::ControlRejected { code, message }), true) => ControlResponse {
                version: CONTROL_PROTOCOL_VERSION,
                request_id: request.request_id,
                result: ControlResult::Error {
                    code: code.clone(),
                    message: message.clone(),
                    effect,
                    details: Some(ControlErrorDetails {
                        current_presented_state_hash: current_state_hash,
                        boundaries: actor_boundaries(&self.event_log),
                        scope: "actor_processed_presented_output".to_owned(),
                        input_sequence: None,
                        screen_frame: None,
                    }),
                },
            },
            (Ok(data), _) => ControlResponse {
                version: CONTROL_PROTOCOL_VERSION,
                request_id: request.request_id,
                result: ControlResult::Ok { data: data.clone() },
            },
            (Err(Error::ScreenFrame(error)), _) => screen_frame_error(
                request.request_id,
                error,
                current_state_hash,
                &self.event_log,
                "screen_frame_projection",
                effect,
            ),
            (Err(error), _) => control_error_response(
                request.request_id,
                control_error_code(error),
                error.to_string(),
                effect,
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
                | Error::GhosttyVt { .. }
                | Error::InvalidTerminalText { .. }
                | Error::ScreenFrame(_),
            ) => {
                return Ok(());
            }
            Err(error) => return Err(error),
        }
    }
}
