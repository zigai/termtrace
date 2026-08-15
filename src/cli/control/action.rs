use super::input::{input_sequence_atom_for_cli, sequence_operation_id};
use super::*;

pub(in crate::cli) fn run_act(
    response: &mut ResponseWriter,
    arguments: ActArgs,
) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session.clone())?;
    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(response, &resolved, "act", ExitCode::FAILURE);
    }
    let request_id = arguments.request_id.unwrap_or_else(uuid::Uuid::now_v7);
    let mut atoms: Vec<InputSequenceAtom> = Vec::new();
    for atom in &arguments.atoms {
        if arguments.pace.is_zero() || !atom.starts_with("text:") {
            let operation_id = sequence_operation_id(request_id, atoms.len());
            atoms.push(input_sequence_atom_for_cli(
                atom,
                operation_id,
                arguments.intent.clone(),
            )?);
            continue;
        }
        let text = atom.strip_prefix("text:").unwrap_or_default();
        for scalar in text.chars() {
            atoms.push(InputSequenceAtom::Bytes {
                operation_id: sequence_operation_id(request_id, atoms.len()),
                data: ByteString::new(scalar.to_string().into_bytes()),
                input_kind: InputKind::Text,
                caller_intent: arguments.intent.clone(),
            });
        }
    }
    if atoms.is_empty() || atoms.len() > 4096 {
        return Err(miette::miette!(
            "input sequence must contain 1 to 4096 atoms"
        ));
    }
    let postcondition = action_postcondition(&arguments);
    let mut request = ControlRequest::new(ControlCommand::Act {
        atoms,
        pace_ns: u64::try_from(arguments.pace.as_nanos()).unwrap_or(u64::MAX),
        include_atom_outcomes: arguments.verbose,
        postcondition,
        projection: ScreenProjectionRequest::default(),
    });
    request.request_id = request_id;
    request.expected_presented_state_hash = arguments.expected_presented_state_hash;
    let control_response = match send_request_response(&resolved.path, &request) {
        Ok(control_response) => control_response,
        Err(error) => {
            write_json_line(
                response,
                &serde_json::json!({
                    "v": 1,
                    "ok": false,
                    "type": "action",
                    "code": "control_transport_failed",
                    "message": error.to_string(),
                    "request_id": request.request_id,
                    "input": {
                        "applied": "unknown",
                        "atoms": serde_json::Value::Null,
                        "bytes": serde_json::Value::Null,
                        "retry": "same_request_id_only",
                    },
                    "postcondition": {
                        "kind": "not_started",
                        "matched": false,
                        "outcome": "not_started",
                    },
                    "observation": serde_json::Value::Null,
                    "ended": resolved.path.join("completion.json").is_file(),
                    "boundaries": serde_json::Value::Null,
                }),
            )?;
            return Ok(ExitCode::FAILURE);
        }
    };
    match control_response.result {
        ControlResult::Ok { data } => {
            let (output, matched) = action_applied_response(control_response.request_id, &data)?;
            write_json_line(response, &output)?;
            return Ok(if matched {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
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
            write_json_line(response, &output)?;
            return Ok(ExitCode::FAILURE);
        }
    }
}

pub(super) fn action_postcondition(arguments: &ActArgs) -> ActionPostcondition {
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

pub(super) fn action_observation(frame: &ScreenFrame) -> Result<serde_json::Value> {
    let mut observation = serde_json::to_value(frame).into_diagnostic()?;
    observation
        .as_object_mut()
        .ok_or_else(|| miette::miette!("action frame response is not an object"))?
        .insert(
            "state".to_owned(),
            serde_json::Value::String(frame.state_hash.clone()),
        );
    return Ok(observation);
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
    let mut input_summary = serde_json::json!({
        "applied": "all",
        "atoms": input.atom_count,
        "bytes": input.bytes_written,
        "pace_ns": input.pace_ns,
    });
    if !input.atoms.is_empty() {
        input_summary
            .as_object_mut()
            .ok_or_else(|| miette::miette!("action input response is not an object"))?
            .insert(
                "outcomes".to_owned(),
                serde_json::to_value(&input.atoms).into_diagnostic()?,
            );
    }
    let (postcondition, observation, ended, matched) = match postcondition {
        ActionPostconditionResult::None { matched, frame } => (
            serde_json::json!({
                "kind": "none",
                "matched": matched,
                "outcome": "matched",
            }),
            action_observation(frame)?,
            false,
            *matched,
        ),
        ActionPostconditionResult::Idle {
            matched,
            quality,
            frame,
        } => (
            serde_json::json!({
                "kind": "idle",
                "matched": matched,
                "outcome": quality,
            }),
            action_observation(frame)?,
            matches!(
                quality,
                StableCaptureQuality::ChildCompleted | StableCaptureQuality::OutputClosed
            ),
            *matched,
        ),
        ActionPostconditionResult::Screen {
            matched,
            quality,
            frame,
        } => (
            serde_json::json!({
                "kind": "screen",
                "matched": matched,
                "outcome": quality,
            }),
            action_observation(frame)?,
            matches!(
                quality,
                ScreenPostconditionQuality::ChildCompleted
                    | ScreenPostconditionQuality::OutputClosed
            ),
            *matched,
        ),
    };
    let mut output = serde_json::json!({
        "v": 1,
        "ok": matched,
        "type": "action",
        "request_id": request_id,
        "input": input_summary,
        "postcondition": postcondition,
        "observation": observation,
        "ended": ended,
        "boundaries": boundaries,
    });
    if !matched {
        output
            .as_object_mut()
            .ok_or_else(|| miette::miette!("action response is not an object"))?
            .insert(
                "code".to_owned(),
                serde_json::Value::String("postcondition_failed".to_owned()),
            );
    }
    return Ok((output, matched));
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
                serde_json::json!(sequence.atoms_applied),
                serde_json::json!(sequence.bytes_written),
                serde_json::json!({
                    "atoms": sequence.atoms_applied,
                    "bytes": sequence.bytes_written,
                    "failed_atom": sequence.failed_atom_index,
                    "failed_operation_id": sequence.failed_operation_id,
                    "failed_atom_bytes": sequence.failed_atom_bytes_written,
                    "first_intent_event_seq": sequence.first_intent_event_seq,
                    "last_result_event_seq": sequence.last_result_event_seq,
                    "resume_from_atom": sequence.resume_from_atom_index,
                }),
            )
        }
        Some(_) => (
            "none",
            serde_json::json!(0),
            serde_json::json!(0),
            serde_json::Value::Null,
        ),
        None => (
            "none",
            serde_json::json!(0),
            serde_json::json!(0),
            serde_json::Value::Null,
        ),
    };
    let input = if prefix.is_null() {
        serde_json::json!({
            "applied": applied,
            "atoms": atoms,
            "bytes": bytes,
            "retry": input_retry(applied),
        })
    } else {
        serde_json::json!({
            "applied": applied,
            "atoms": atoms,
            "bytes": bytes,
            "prefix": prefix,
            "retry": input_retry(applied),
        })
    };
    let observation = details.map_or(serde_json::Value::Null, |details| {
        return serde_json::json!({
            "state": details.current_presented_state_hash,
            "scope": details.scope,
        });
    });
    let boundaries = details.map_or(serde_json::Value::Null, |details| {
        return serde_json::json!(details.boundaries);
    });
    return serde_json::json!({
        "v": 1,
        "ok": false,
        "type": "action",
        "code": code,
        "message": message,
        "request_id": request_id,
        "input": input,
        "postcondition": {
            "kind": "not_started",
            "matched": false,
            "outcome": "not_started",
        },
        "observation": observation,
        "ended": ended,
        "boundaries": boundaries,
    });
}

pub(super) fn input_retry(applied: &str) -> &'static str {
    return match applied {
        "partial" => "inspect_before_retry",
        "unknown" => "same_request_id_only",
        _ => "safe_new_request",
    };
}

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
    use termtrace_model::{
        protocol::control::{ControlData, ControlErrorDetails},
        terminal::observation::{ActorBoundaries, CaptureQuality},
        terminal::screen::{ScreenFrameSource, ScreenProjectionRequest},
    };
    use termtrace_terminal::{GhosttyVt, project_screen_frame};

    use super::{action_applied_response, action_error_response};

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
        assert!(
            response["observation"]["data"]["rows"][0]["text"]
                .as_str()
                .is_some_and(|text| return text.starts_with("current screen"))
        );
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
}
