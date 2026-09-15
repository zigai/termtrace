use super::args::SendArgs;
use super::input::input_sequence_atom_for_cli;
use super::*;
use std::mem::take;
use termtrace_model::{
    protocol::control::{PointerOperation, ProcessSignal, ScreenPredicate},
    terminal::screen::FrameDetail,
    terminal::state::MouseButton,
};

fn parse_click_str(s: &str) -> Result<(u16, u16)> {
    let (x_str, y_str) = s
        .split_once(',')
        .ok_or_else(|| miette::miette!("invalid click coordinates '{s}'; expected X,Y"))?;
    let col = x_str.trim().parse::<u16>().into_diagnostic()?;
    let row = y_str.trim().parse::<u16>().into_diagnostic()?;

    return Ok((col, row));
}

fn parse_resize_str(s: &str) -> Result<(u16, u16, u16, u16)> {
    let (w_str, h_str) = s
        .split_once('x')
        .or_else(|| return s.split_once('X'))
        .or_else(|| return s.split_once(','))
        .ok_or_else(|| {
            return miette::miette!(
                "invalid resize format '{s}'; expected WxH e.g. 80x24 or 80,24"
            );
        })?;
    let cols = w_str.trim().parse::<u16>().into_diagnostic()?;
    let rows = h_str.trim().parse::<u16>().into_diagnostic()?;
    let pixel_width = cols.saturating_mul(12);
    let pixel_height = rows.saturating_mul(24);

    return Ok((cols, rows, pixel_width, pixel_height));
}

fn parse_signal_str(s: &str) -> Result<ProcessSignal> {
    let normalized = s.trim().to_ascii_uppercase();
    let name = normalized.strip_prefix("SIG").unwrap_or(&normalized);

    return match name {
        "INT" | "INTERRUPT" => Ok(ProcessSignal::Interrupt),
        "TERM" | "TERMINATE" => Ok(ProcessSignal::Terminate),
        "HUP" | "HANGUP" => Ok(ProcessSignal::Hangup),
        "USR1" | "USER1" => Ok(ProcessSignal::User1),
        "USR2" | "USER2" => Ok(ProcessSignal::User2),
        _ => Err(miette::miette!(
            "unknown signal '{s}'; expected INT, TERM, HUP, USR1, or USR2"
        )),
    };
}

pub(in crate::cli) fn run_send(
    response: &mut ResponseWriter,
    arguments: SendArgs,
) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session.clone())?;
    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(response, &resolved, "send", ExitCode::FAILURE);
    }

    let request_id = arguments.request_id.unwrap_or_else(uuid::Uuid::now_v7);
    let mut stdin_bytes: Option<Vec<u8>> = None;
    if arguments.stdin || arguments.atoms.iter().any(|a| return a == "-") {
        let mut data = Vec::new();
        io::stdin().read_to_end(&mut data).into_diagnostic()?;
        stdin_bytes = Some(data);
    }

    enum SendOp {
        Control(ControlCommand, &'static str),
        PacedAtoms(Vec<InputSequenceAtom>),
    }

    let mut ops: Vec<SendOp> = Vec::new();
    let mut current_atoms: Vec<InputSequenceAtom> = Vec::new();

    if let Some(resize) = &arguments.resize {
        let (cols, rows, pixel_width, pixel_height) = parse_resize_str(resize)?;
        ops.push(SendOp::Control(
            ControlCommand::Resize {
                cols,
                rows,
                pixel_width,
                pixel_height,
            },
            "resize",
        ));
    }

    if let Some(signal_str) = &arguments.signal {
        let signal = parse_signal_str(signal_str)?;
        ops.push(SendOp::Control(ControlCommand::Signal { signal }, "signal"));
    }

    if let Some(mark) = &arguments.mark {
        ops.push(SendOp::Control(
            ControlCommand::Mark {
                label: mark.clone(),
            },
            "mark",
        ));
    }

    for atom in &arguments.atoms {
        if atom == "-" {
            let data = stdin_bytes.clone().unwrap_or_default();
            current_atoms.push(InputSequenceAtom::Bytes {
                operation_id: derive_input_operation_id(request_id, current_atoms.len()),
                data: ByteString::new(data),
                input_kind: InputKind::Raw,
                caller_intent: arguments.intent.clone(),
            });
        } else if let Some(coords) = atom.strip_prefix("click:") {
            if !current_atoms.is_empty() {
                ops.push(SendOp::PacedAtoms(take(&mut current_atoms)));
            }

            let (col, row) = parse_click_str(coords)?;
            ops.push(SendOp::Control(
                ControlCommand::Pointer {
                    column: col,
                    row,
                    operation: PointerOperation::Click {
                        button: MouseButton::Left,
                    },
                    strict: false,
                },
                "pointer",
            ));
        } else if let Some(text) = atom.strip_prefix("activate:") {
            if !current_atoms.is_empty() {
                ops.push(SendOp::PacedAtoms(take(&mut current_atoms)));
            }

            ops.push(SendOp::Control(
                ControlCommand::Activate {
                    text: text.to_owned(),
                    unique: false,
                    strict: false,
                },
                "activate",
            ));
        } else if let Some(wh) = atom.strip_prefix("resize:") {
            if !current_atoms.is_empty() {
                ops.push(SendOp::PacedAtoms(take(&mut current_atoms)));
            }

            let (cols, rows, pixel_width, pixel_height) = parse_resize_str(wh)?;
            ops.push(SendOp::Control(
                ControlCommand::Resize {
                    cols,
                    rows,
                    pixel_width,
                    pixel_height,
                },
                "resize",
            ));
        } else if let Some(sig) = atom.strip_prefix("signal:") {
            if !current_atoms.is_empty() {
                ops.push(SendOp::PacedAtoms(take(&mut current_atoms)));
            }

            let signal = parse_signal_str(sig)?;
            ops.push(SendOp::Control(ControlCommand::Signal { signal }, "signal"));
        } else if let Some(label) = atom.strip_prefix("mark:") {
            if !current_atoms.is_empty() {
                ops.push(SendOp::PacedAtoms(take(&mut current_atoms)));
            }

            ops.push(SendOp::Control(
                ControlCommand::Mark {
                    label: label.to_owned(),
                },
                "mark",
            ));
        } else if arguments.pace.is_zero() || !atom.starts_with("text:") {
            let operation_id = derive_input_operation_id(request_id, current_atoms.len());
            current_atoms.push(input_sequence_atom_for_cli(
                atom,
                operation_id,
                arguments.intent.clone(),
            )?);
        } else {
            let text = atom.strip_prefix("text:").unwrap_or_default();

            for scalar in text.chars() {
                current_atoms.push(InputSequenceAtom::Bytes {
                    operation_id: derive_input_operation_id(request_id, current_atoms.len()),
                    data: ByteString::new(scalar.to_string().into_bytes()),
                    input_kind: InputKind::Text,
                    caller_intent: arguments.intent.clone(),
                });
            }
        }
    }

    if let Some(text) = &arguments.text {
        if arguments.pace.is_zero() {
            current_atoms.push(InputSequenceAtom::Bytes {
                operation_id: derive_input_operation_id(request_id, current_atoms.len()),
                data: ByteString::new(text.as_bytes().to_vec()),
                input_kind: InputKind::Text,
                caller_intent: arguments.intent.clone(),
            });
        } else {
            for scalar in text.chars() {
                current_atoms.push(InputSequenceAtom::Bytes {
                    operation_id: derive_input_operation_id(request_id, current_atoms.len()),
                    data: ByteString::new(scalar.to_string().into_bytes()),
                    input_kind: InputKind::Text,
                    caller_intent: arguments.intent.clone(),
                });
            }
        }
    }

    if let Some(path) = &arguments.file {
        let bytes = fs_err::read(path).into_diagnostic()?;
        current_atoms.push(InputSequenceAtom::Bytes {
            operation_id: derive_input_operation_id(request_id, current_atoms.len()),
            data: ByteString::new(bytes),
            input_kind: InputKind::Raw,
            caller_intent: arguments.intent.clone(),
        });
    }

    if arguments.stdin && !arguments.atoms.iter().any(|a| return a == "-") {
        let bytes = stdin_bytes.unwrap_or_default();
        current_atoms.push(InputSequenceAtom::Bytes {
            operation_id: derive_input_operation_id(request_id, current_atoms.len()),
            data: ByteString::new(bytes),
            input_kind: InputKind::Raw,
            caller_intent: arguments.intent.clone(),
        });
    }

    if !current_atoms.is_empty() {
        ops.push(SendOp::PacedAtoms(current_atoms));
    }

    if ops.is_empty() {
        return Err(miette::miette!(
            "send requires at least one input atom or flag"
        ));
    }

    let postcondition = action_postcondition(&arguments);
    let total_ops = ops.len();

    for (index, op) in ops.into_iter().enumerate() {
        let is_last = index + 1 == total_ops;
        match op {
            SendOp::Control(cmd, label) => {
                let mut request = ControlRequest::new(cmd);
                request.request_id = request_id;
                request
                    .expected_presented_state_hash
                    .clone_from(&arguments.if_state);
                let control_response = match send_request_response(&resolved.path, &request) {
                    Ok(resp) => resp,
                    Err(error) => {
                        if arguments.json {
                            write_json_pretty(
                                response,
                                &serde_json::json!({
                                    "v": 1,
                                    "ok": false,
                                    "type": label,
                                    "message": error.to_string(),
                                    "request_id": request_id,
                                }),
                            )?;
                        } else {
                            write_stdout(response, format!("Send {label} failed: {error}\n"))?;
                        }

                        return Ok(ExitCode::FAILURE);
                    }
                };

                if let ControlResult::Error {
                    code,
                    message,
                    details,
                } = control_response.result
                {
                    let output = action_error_response(
                        control_response.request_id,
                        code,
                        message,
                        details.as_ref(),
                        resolved.path.join("completion.json").is_file(),
                    );

                    if arguments.json {
                        write_json_pretty(response, &output)?;
                    } else {
                        write_stdout(
                            response,
                            format!(
                                "Send {label} error: {}\n",
                                output
                                    .get("message")
                                    .and_then(|m| return m.as_str())
                                    .unwrap_or("unknown error")
                            ),
                        )?;
                    }

                    return Ok(ExitCode::FAILURE);
                }

                if is_last {
                    let (matched, post_kind, outcome) = match postcondition {
                        ActionPostcondition::Idle {
                            stable_for_ns,
                            timeout_ns,
                        } => {
                            let capture_req = ControlRequest::new(ControlCommand::CaptureStable {
                                stable_for_ns,
                                timeout_ns,
                                projection: action_projection(&arguments),
                            });

                            match send_request_response(&resolved.path, &capture_req) {
                                Ok(resp) => match resp.result {
                                    ControlResult::Ok { data } => {
                                        let matched = matches!(
                                            data,
                                            ControlData::StableSnapshot { quality, .. }
                                                if quality == StableCaptureQuality::Stable
                                        );
                                        (
                                            matched,
                                            "idle",
                                            if matched { "matched" } else { "unmatched" },
                                        )
                                    }
                                    ControlResult::Error { .. } => (false, "idle", "error"),
                                },
                                Err(_) => (false, "idle", "error"),
                            }
                        }
                        ActionPostcondition::Screen {
                            predicate,
                            timeout_ns,
                        } => {
                            let wait_req = ControlRequest::new(ControlCommand::WaitScreen {
                                predicate,
                                stable_for_ns: 0,
                                timeout_ns,
                                projection: action_projection(&arguments),
                            });

                            match send_request_response(&resolved.path, &wait_req) {
                                Ok(resp) => match resp.result {
                                    ControlResult::Ok { data } => {
                                        let matched = matches!(
                                            data,
                                            ControlData::ScreenWait { matched, .. } if matched
                                        );
                                        (
                                            matched,
                                            "screen",
                                            if matched { "matched" } else { "deadline" },
                                        )
                                    }
                                    ControlResult::Error { .. } => (false, "screen", "error"),
                                },
                                Err(_) => (false, "screen", "deadline"),
                            }
                        }
                        ActionPostcondition::None => (true, "none", "matched"),
                    };

                    if arguments.json {
                        write_json_pretty(
                            response,
                            &serde_json::json!({
                                "v": 1,
                                "ok": matched,
                                "type": label,
                                "request_id": request_id,
                                "postcondition": {
                                    "kind": post_kind,
                                    "matched": matched,
                                    "outcome": outcome,
                                }
                            }),
                        )?;
                    } else if matched {
                        write_stdout(
                            response,
                            format!("Send command completed ({label}); postcondition {post_kind} matched={matched}, outcome={outcome}\n"),
                        )?;
                    } else {
                        write_stdout(
                            response,
                            format!("Send command failed ({label}); postcondition {post_kind} matched={matched}, outcome={outcome}\n"),
                        )?;
                    }

                    return Ok(if matched { ExitCode::SUCCESS } else { ExitCode::FAILURE });
                }
            }
            SendOp::PacedAtoms(atoms) => {
                let op_postcondition =
                    if is_last { postcondition.clone() } else { ActionPostcondition::None };

                let mut request = ControlRequest::new(ControlCommand::Act {
                    atoms,
                    pace_ns: u64::try_from(arguments.pace.as_nanos()).unwrap_or(u64::MAX),
                    include_atom_outcomes: arguments.verbose,
                    postcondition: op_postcondition,
                    projection: action_projection(&arguments),
                });
                request.request_id = request_id;

                request
                    .expected_presented_state_hash
                    .clone_from(&arguments.if_state);
                let control_response = match send_request_response(&resolved.path, &request) {
                    Ok(control_response) => control_response,
                    Err(error) => {
                        let (code, applied, retry) = match &error {
                            SdkError::InvalidControlProtocol { .. } => {
                                ("invalid_payload", "none", "safe_new_request")
                            }
                            _ => (
                                "control_transport_failed",
                                "unknown",
                                "same_request_id_only",
                            ),
                        };

                        let payload = serde_json::json!({
                            "v": 1,
                            "ok": false,
                            "type": "action",
                            "code": code,
                            "message": error.to_string(),
                            "request_id": request.request_id,
                            "input": {
                                "applied": applied,
                                "atoms": serde_json::Value::Null,
                                "bytes": serde_json::Value::Null,
                                "retry": retry,
                            },
                            "postcondition": {
                                "matched": false,
                                "quality": "unmatched",
                                "state": serde_json::Value::Null,
                                "screen": serde_json::Value::Null,
                                "state_hash": serde_json::Value::Null,
                                "timeout": true,
                            },
                        });

                        if arguments.json {
                            write_json_pretty(response, &payload)?;
                        } else {
                            write_stdout(response, format!("Send action failed: {error}\n"))?;
                        }

                        return Ok(ExitCode::FAILURE);
                    }
                };

                match control_response.result {
                    ControlResult::Ok { data } => {
                        let is_direct_stream = arguments.atoms.is_empty()
                            && (arguments.file.is_some()
                                || arguments.stdin
                                || arguments.text.is_some())
                            && matches!(postcondition, ActionPostcondition::None);

                        if is_direct_stream {
                            if let ControlData::ActionApplied { input, .. } = &data {
                                write_json_line(
                                    response,
                                    &serde_json::json!({
                                        "kind": "input_sent",
                                        "request_id": input.sequence_id,
                                        "bytes_written": input.bytes_written,
                                    }),
                                )?;

                                return Ok(ExitCode::SUCCESS);
                            }

                            if let ControlData::InputSequenceApplied {
                                sequence_id,
                                bytes_written,
                                boundaries,
                                ..
                            } = &data
                            {
                                write_json_line(
                                    response,
                                    &serde_json::json!({
                                        "kind": "input_sent",
                                        "request_id": sequence_id,
                                        "bytes_written": bytes_written,
                                        "boundary": compact_boundaries(boundaries),
                                    }),
                                )?;

                                return Ok(ExitCode::SUCCESS);
                            }
                        }

                        let (output, matched) =
                            action_applied_response(control_response.request_id, &data)?;

                        if is_last {
                            if arguments.json {
                                write_json_pretty(response, &output)?;
                            } else {
                                write_stdout(response, human_action_summary(&output))?;
                            }

                            return Ok(if matched { ExitCode::SUCCESS } else { ExitCode::FAILURE });
                        }
                    }
                    ControlResult::Error {
                        code,
                        message,
                        details,
                    } => {
                        let output = action_error_response(
                            control_response.request_id,
                            code,
                            message,
                            details.as_ref(),
                            resolved.path.join("completion.json").is_file(),
                        );

                        if arguments.json {
                            write_json_pretty(response, &output)?;
                        } else {
                            write_stdout(
                                response,
                                format!(
                                    "Send action error: {}\n",
                                    output
                                        .get("message")
                                        .and_then(|m| return m.as_str())
                                        .unwrap_or("unknown error")
                                ),
                            )?;
                        }

                        return Ok(ExitCode::FAILURE);
                    }
                }
            }
        }
    }

    return Ok(ExitCode::SUCCESS);
}

pub(super) fn action_postcondition(arguments: &SendArgs) -> ActionPostcondition {
    if let Some(stable_for) = arguments.until_idle {
        return ActionPostcondition::Idle {
            stable_for_ns: u64::try_from(stable_for.as_nanos()).unwrap_or(u64::MAX),
            timeout_ns: u64::try_from(arguments.timeout.as_nanos()).unwrap_or(u64::MAX),
        };
    }

    if let Some(text) = &arguments.until_screen {
        return ActionPostcondition::Screen {
            predicate: ScreenPredicate::Contains { text: text.clone() },
            timeout_ns: u64::try_from(arguments.timeout.as_nanos()).unwrap_or(u64::MAX),
        };
    }

    if let Some(pattern) = &arguments.until_screen_regex {
        return ActionPostcondition::Screen {
            predicate: ScreenPredicate::MatchesRegex {
                pattern: pattern.clone(),
            },
            timeout_ns: u64::try_from(arguments.timeout.as_nanos()).unwrap_or(u64::MAX),
        };
    }
    return ActionPostcondition::None;
}

pub(super) fn action_projection(arguments: &SendArgs) -> ScreenProjectionRequest {
    if arguments.verbose {
        return ScreenProjectionRequest::default();
    }
    return ScreenProjectionRequest {
        detail: FrameDetail::None,
        region: None,
    };
}

#[derive(serde::Serialize)]
pub(super) struct ActionObservation<'data> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) state: Option<&'data str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) state_hash: Option<&'data str>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub(super) frame: Option<&'data ScreenFrame>,
}

#[cfg(test)]
pub(super) fn action_observation(frame: &ScreenFrame) -> Result<serde_json::Value> {
    return serde_json::to_value(ActionObservation {
        state: Some(&frame.state_hash),
        state_hash: Some(&frame.state_hash),
        frame: Some(frame),
    })
    .into_diagnostic();
}

#[derive(serde::Serialize)]
pub(super) struct ActionInputSummary<'data> {
    pub(super) applied: &'static str,
    pub(super) atoms: u32,
    pub(super) bytes: u64,
    pub(super) pace_ns: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) outcomes: Option<&'data [InputSequenceAtomOutcome]>,
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum ActionPostconditionSummary<'data> {
    None {
        matched: bool,
        outcome: &'static str,
    },
    Idle {
        matched: bool,
        outcome: &'data StableCaptureQuality,
    },
    Screen {
        matched: bool,
        outcome: &'data ScreenPostconditionQuality,
    },
}

#[derive(serde::Serialize)]
pub(super) struct ActionAppliedOutput<'data> {
    pub(super) v: u32,
    pub(super) ok: bool,
    #[serde(rename = "type")]
    pub(super) response_type: &'static str,
    pub(super) request_id: uuid::Uuid,
    pub(super) input: ActionInputSummary<'data>,
    pub(super) postcondition: ActionPostconditionSummary<'data>,
    pub(super) observation: ActionObservation<'data>,
    pub(super) ended: bool,
    pub(super) boundaries: &'data ActorBoundaries,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) code: Option<&'static str>,
}

pub(super) fn action_applied_response(
    request_id: uuid::Uuid,
    data: &ControlData,
) -> Result<(serde_json::Value, bool)> {
    let ControlData::ActionApplied {
        input,
        postcondition,
        boundaries,
    } = data
    else {
        return Err(miette::miette!(
            "recording actor returned an unexpected action payload"
        ));
    };
    if input.sequence_id != request_id {
        return Err(miette::miette!(
            "recording actor returned an action input identifier that does not match the request"
        ));
    }

    let input_summary = ActionInputSummary {
        applied: "all",
        atoms: input.atom_count,
        bytes: input.bytes_written,
        pace_ns: input.pace_ns,
        outcomes: (!input.atoms.is_empty()).then_some(&input.atoms),
    };

    let (postcondition_summary, ended, matched, state_hash, frame) = match postcondition {
        ActionPostconditionResult::None {
            matched,
            state_hash,
            frame,
        } => (
            ActionPostconditionSummary::None {
                matched: *matched,
                outcome: "matched",
            },
            false,
            *matched,
            state_hash
                .as_deref()
                .or_else(|| return frame.as_ref().map(|f| return f.state_hash.as_str())),
            frame.as_ref(),
        ),
        ActionPostconditionResult::Idle {
            matched,
            quality,
            state_hash,
            frame,
        } => (
            ActionPostconditionSummary::Idle {
                matched: *matched,
                outcome: quality,
            },
            matches!(
                quality,
                StableCaptureQuality::ChildCompleted | StableCaptureQuality::OutputClosed
            ),
            *matched,
            state_hash
                .as_deref()
                .or_else(|| return frame.as_ref().map(|f| return f.state_hash.as_str())),
            frame.as_ref(),
        ),
        ActionPostconditionResult::Screen {
            matched,
            quality,
            state_hash,
            frame,
        } => (
            ActionPostconditionSummary::Screen {
                matched: *matched,
                outcome: quality,
            },
            matches!(
                quality,
                ScreenPostconditionQuality::ChildCompleted
                    | ScreenPostconditionQuality::OutputClosed
            ),
            *matched,
            state_hash
                .as_deref()
                .or_else(|| return frame.as_ref().map(|f| return f.state_hash.as_str())),
            frame.as_ref(),
        ),
    };

    let output = ActionAppliedOutput {
        v: 1,
        ok: matched,
        response_type: "action",
        request_id,
        input: input_summary,
        postcondition: postcondition_summary,
        observation: ActionObservation {
            state: state_hash,
            state_hash,
            frame,
        },
        ended,
        boundaries,
        code: (!matched).then_some("postcondition_failed"),
    };

    return Ok((serde_json::to_value(&output).into_diagnostic()?, matched));
}

#[derive(serde::Serialize)]
pub(super) struct ActionFailedPrefix {
    pub(super) atoms: u32,
    pub(super) bytes: u64,
    pub(super) failed_atom: u32,
    pub(super) failed_operation_id: uuid::Uuid,
    pub(super) failed_atom_bytes: u64,
    pub(super) first_intent_event_seq: u64,
    pub(super) last_result_event_seq: u64,
    pub(super) resume_from_atom: u32,
}

#[derive(serde::Serialize)]
pub(super) struct ActionFailedInputSummary {
    pub(super) applied: &'static str,
    pub(super) atoms: u32,
    pub(super) bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) prefix: Option<ActionFailedPrefix>,
    pub(super) retry: &'static str,
}

#[derive(serde::Serialize)]
pub(super) struct ActionErrorPostcondition {
    pub(super) kind: &'static str,
    pub(super) matched: bool,
    pub(super) outcome: &'static str,
}

#[derive(serde::Serialize)]
pub(super) struct ActionErrorObservation<'data> {
    pub(super) state: &'data str,
    pub(super) scope: &'data str,
}

#[derive(serde::Serialize)]
pub(super) struct ActionErrorOutput<'data> {
    pub(super) v: u32,
    pub(super) ok: bool,
    #[serde(rename = "type")]
    pub(super) response_type: &'static str,
    pub(super) code: String,
    pub(super) message: String,
    pub(super) request_id: uuid::Uuid,
    pub(super) input: ActionFailedInputSummary,
    pub(super) postcondition: ActionErrorPostcondition,
    pub(super) observation: Option<ActionErrorObservation<'data>>,
    pub(super) ended: bool,
    pub(super) boundaries: Option<&'data ActorBoundaries>,
}

pub(super) fn action_error_response(
    request_id: uuid::Uuid,
    code: String,
    message: String,
    details: Option<&ControlErrorDetails>,
    ended: bool,
) -> serde_json::Value {
    let sequence = details.and_then(|details| return details.input_sequence.as_ref());
    let (applied, atoms, bytes, prefix) = match sequence {
        Some(sequence)
            if sequence.atoms_applied != 0 || sequence.failed_atom_bytes_written != 0 =>
        {
            (
                "partial",
                sequence.atoms_applied,
                sequence.bytes_written,
                Some(ActionFailedPrefix {
                    atoms: sequence.atoms_applied,
                    bytes: sequence.bytes_written,
                    failed_atom: sequence.failed_atom_index,
                    failed_operation_id: sequence.failed_operation_id,
                    failed_atom_bytes: sequence.failed_atom_bytes_written,
                    first_intent_event_seq: sequence.first_intent_event_seq,
                    last_result_event_seq: sequence.last_result_event_seq,
                    resume_from_atom: sequence.resume_from_atom_index,
                }),
            )
        }
        _ => ("none", 0, 0, None),
    };

    let input = ActionFailedInputSummary {
        applied,
        atoms,
        bytes,
        prefix,
        retry: input_retry(applied),
    };

    let observation = details.map(|details| {
        return ActionErrorObservation {
            state: &details.current_presented_state_hash,
            scope: &details.scope,
        };
    });
    let boundaries = details.map(|details| return &details.boundaries);

    let output = ActionErrorOutput {
        v: 1,
        ok: false,
        response_type: "action",
        code,
        message,
        request_id,
        input,
        postcondition: ActionErrorPostcondition {
            kind: "not_started",
            matched: false,
            outcome: "not_started",
        },
        observation,
        ended,
        boundaries,
    };

    #[expect(
        clippy::expect_used,
        reason = "ActionErrorOutput contains only primitive serializable types and cannot fail JSON serialization"
    )]
    return serde_json::to_value(&output).expect("action error serialization");
}

pub(super) fn input_retry(applied: &str) -> &'static str {
    return match applied {
        "partial" => "inspect_before_retry",
        "unknown" => "same_request_id_only",
        _ => "safe_new_request",
    };
}

#[expect(dead_code, reason = "Input failure categorization helper")]
pub(super) fn input_failure_application(details: Option<&ControlErrorDetails>) -> &'static str {
    let Some(sequence) = details.and_then(|details| return details.input_sequence.as_ref()) else {
        return "none";
    };
    if sequence.atoms_applied == 0 && sequence.failed_atom_bytes_written == 0 {
        return "none";
    }
    return "partial";
}

#[cfg(test)]
mod action_response_tests {
    use super::{
        action_applied_response, action_error_response, action_observation, action_projection,
    };
    use crate::cli::control::args::SendArgs;
    use std::{path::PathBuf, time::Duration};
    use termtrace_model::{
        protocol::control::{ControlData, ControlErrorDetails},
        terminal::observation::{ActorBoundaries, CaptureQuality},
        terminal::screen::FrameDetail,
        terminal::screen::{ScreenFrameSource, ScreenProjectionRequest},
    };
    use termtrace_terminal::{project_screen_frame, GhosttyVt};

    #[test]
    fn action_success_retains_input_observation_request_and_boundaries() {
        let request_id =
            uuid::Uuid::parse_str("018f47d2-7ad0-7000-8000-000000000001").expect("request UUID");

        let boundaries: ActorBoundaries = serde_json::from_value(serde_json::json!({
            "durable": {
                "next_event_seq": 4,
                "event_hash": "b".repeat(64),
                "output_bytes": 10
            },
            "observed": {
                "next_event_seq": 5,
                "event_hash": "c".repeat(64),
                "output_bytes": 12
            }
        }))
        .expect("actor boundaries");

        let mut engine = GhosttyVt::new(80, 24, 0).expect("semantic engine");
        engine.write(b"current screen");
        let state = engine.snapshot().expect("semantic state");
        let frame = project_screen_frame(
            &state,
            &ScreenProjectionRequest::default(),
            ScreenFrameSource::Actor {
                session_id: request_id,
                boundaries: boundaries.clone(),
                observed_monotonic_ns: 1,
            },
            true,
            CaptureQuality::Live,
        )
        .expect("rich frame");

        let data: ControlData = serde_json::from_value(serde_json::json!({
            "kind": "action_applied",
            "input": {
                "sequence_id": request_id,
                "atom_count": 2,
                "bytes_written": 4,
                "first_intent_event_seq": 1,
                "last_result_event_seq": 4,
                "pace_ns": 1_000_000,
                "atoms": [{
                    "operation_id": "018f47d2-7ad0-7000-8000-000000000002",
                    "input_kind": "text",
                    "bytes_written": 2,
                    "intent_event_seq": 1,
                    "result_event_seq": 2
                }]
            },
            "postcondition": {
                "kind": "screen",
                "matched": false,
                "quality": "deadline",
                "frame": frame
            },
            "boundaries": boundaries
        }))
        .expect("action payload");

        let (response, matched) =
            action_applied_response(request_id, &data).expect("action response");
        assert!(!matched);
        assert_eq!(response["request_id"], request_id.to_string());
        assert_eq!(response["input"]["applied"], "all");
        assert_eq!(response["input"]["atoms"], 2);
        assert_eq!(response["input"]["bytes"], 4);
        assert_eq!(
            response["input"]["outcomes"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(response["postcondition"]["outcome"], "deadline");
        assert_eq!(response["ended"], false);
        assert!(response["observation"]["data"]["rows"][0]["text"]
            .as_str()
            .is_some_and(|text| return text.starts_with("current screen")));
        assert_eq!(response["boundaries"]["durable"]["next_event_seq"], 4);
        assert_eq!(response["boundaries"]["observed"]["next_event_seq"], 5);
    }

    #[test]
    fn action_partial_failure_retains_the_exact_attempted_prefix() {
        let request_id =
            uuid::Uuid::parse_str("018f47d2-7ad0-7000-8000-000000000010").expect("request UUID");

        let details: ControlErrorDetails = serde_json::from_value(serde_json::json!({
            "current_presented_state_hash": "d".repeat(64),
            "boundaries": {
                "durable": {
                    "next_event_seq": 7,
                    "event_hash": "e".repeat(64),
                    "output_bytes": 20
                },
                "observed": {
                    "next_event_seq": 8,
                    "event_hash": "f".repeat(64),
                    "output_bytes": 21
                }
            },
            "scope": "input_sequence_attempted_prefix",
            "input_sequence": {
                "sequence_id": request_id,
                "atom_count": 3,
                "atoms_applied": 1,
                "failed_atom_index": 1,
                "failed_operation_id": "018f47d2-7ad0-7000-8000-000000000011",
                "bytes_written": 5,
                "failed_atom_bytes_written": 2,
                "first_intent_event_seq": 2,
                "last_result_event_seq": 7,
                "resume_from_atom_index": 2
            }
        }))
        .expect("failure details");

        let response = action_error_response(
            request_id,
            "input_sequence_incomplete".to_owned(),
            "input stopped".to_owned(),
            Some(&details),
            false,
        );
        assert_eq!(response["request_id"], request_id.to_string());
        assert_eq!(response["input"]["applied"], "partial");
        assert_eq!(response["input"]["atoms"], 1);
        assert_eq!(response["input"]["bytes"], 5);
        assert_eq!(response["input"]["prefix"]["atoms"], 1);
        assert_eq!(response["input"]["prefix"]["bytes"], 5);
        assert_eq!(response["input"]["prefix"]["failed_atom"], 1);
        assert_eq!(response["input"]["prefix"]["failed_atom_bytes"], 2);
        assert_eq!(response["input"]["prefix"]["resume_from_atom"], 2);
        assert_eq!(response["postcondition"]["outcome"], "not_started");
        assert_eq!(response["observation"]["state"], "d".repeat(64));
        assert_eq!(response["boundaries"]["observed"]["next_event_seq"], 8);
    }

    #[test]
    fn action_observation_serializes_state_and_flattened_frame() {
        let request_id =
            uuid::Uuid::parse_str("018f47d2-7ad0-7000-8000-000000000001").expect("request UUID");

        let boundaries: ActorBoundaries = serde_json::from_value(serde_json::json!({
            "durable": {
                "next_event_seq": 1,
                "event_hash": "a".repeat(64),
                "output_bytes": 0
            },
            "observed": {
                "next_event_seq": 2,
                "event_hash": "b".repeat(64),
                "output_bytes": 16
            }
        }))
        .expect("actor boundaries");

        let mut engine = GhosttyVt::new(80, 24, 0).expect("semantic engine");
        engine.write(b"observation test");
        let state = engine.snapshot().expect("semantic state");
        let frame = project_screen_frame(
            &state,
            &ScreenProjectionRequest::default(),
            ScreenFrameSource::Actor {
                session_id: request_id,
                boundaries,
                observed_monotonic_ns: 1,
            },
            true,
            CaptureQuality::Live,
        )
        .expect("rich frame");

        let value = action_observation(&frame).expect("observation JSON");
        assert_eq!(value["state"], frame.state_hash);
        assert_eq!(value["state_hash"], frame.state_hash);
        assert_eq!(value["version"], 1);
    }

    #[test]
    fn action_projection_selects_lightweight_detail_by_default() {
        let args = SendArgs {
            session: PathBuf::from("test-session"),
            atoms: Vec::new(),
            stdin: false,
            text: None,
            file: None,
            resize: None,
            signal: None,
            mark: None,
            until_screen: Some("ready".to_owned()),
            until_screen_regex: None,
            until_idle: None,
            timeout: Duration::from_secs(30),
            pace: Duration::ZERO,
            intent: None,
            if_state: None,
            request_id: None,
            verbose: false,
            json: false,
        };
        let projection = action_projection(&args);
        assert_eq!(projection.detail, FrameDetail::None);
    }

    #[test]
    fn action_projection_preserves_full_rich_spans_when_verbose_requested() {
        let args = SendArgs {
            session: PathBuf::from("test-session"),
            atoms: Vec::new(),
            stdin: false,
            text: None,
            file: None,
            resize: None,
            signal: None,
            mark: None,
            until_screen: Some("ready".to_owned()),
            until_screen_regex: None,
            until_idle: None,
            timeout: Duration::from_secs(30),
            pace: Duration::ZERO,
            intent: None,
            if_state: None,
            request_id: None,
            verbose: true,
            json: false,
        };
        let projection = action_projection(&args);
        assert_eq!(projection.detail, FrameDetail::Spans);
        assert!(projection.region.is_none());
    }
}
