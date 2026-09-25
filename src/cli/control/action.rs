use super::args::SendArgs;
use super::input::input_atom_for_cli;
use super::*;
use std::mem::take;
use termtrace_model::{
    protocol::control::{PointerOperation, ProcessSignal},
    protocol::mutation::{MutationEffect, Retry},
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

fn parse_resize_str(s: &str) -> Result<ControlCommand> {
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

    return Ok(ControlCommand::Resize {
        cols,
        rows,
        pixel_width: cols.saturating_mul(defaults::CELL_WIDTH),
        pixel_height: rows.saturating_mul(defaults::CELL_HEIGHT),
    });
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

/// One ordered piece of a `send`: a single control command, or a run of input atoms applied
/// as one action.
enum SendStep {
    Control(ControlCommand, &'static str),
    Act(Vec<InputAtom>),
}

fn send_steps(arguments: &SendArgs) -> Result<Vec<SendStep>> {
    let stdin_requested = arguments.stdin || arguments.atoms.iter().any(|atom| return atom == "-");
    let mut stdin_bytes = Vec::new();
    if stdin_requested {
        io::stdin()
            .read_to_end(&mut stdin_bytes)
            .into_diagnostic()?;
    }

    let mut steps = Vec::new();
    let mut atoms = Vec::new();

    if let Some(resize) = &arguments.resize {
        steps.push(SendStep::Control(parse_resize_str(resize)?, "resize"));
    }

    if let Some(signal) = &arguments.signal {
        let signal = parse_signal_str(signal)?;
        steps.push(SendStep::Control(
            ControlCommand::Signal { signal },
            "signal",
        ));
    }

    if let Some(label) = &arguments.mark {
        let label = label.clone();
        steps.push(SendStep::Control(ControlCommand::Mark { label }, "mark"));
    }

    for atom in &arguments.atoms {
        let control = if let Some(coordinates) = atom.strip_prefix("click:") {
            let (column, row) = parse_click_str(coordinates)?;
            let operation = PointerOperation::Click {
                button: MouseButton::Left,
            };

            Some((
                ControlCommand::Pointer {
                    column,
                    row,
                    operation,
                    strict: false,
                },
                "pointer",
            ))
        } else if let Some(text) = atom.strip_prefix("activate:") {
            Some((
                ControlCommand::Activate {
                    text: text.to_owned(),
                    unique: false,
                    strict: false,
                },
                "activate",
            ))
        } else if let Some(geometry) = atom.strip_prefix("resize:") {
            Some((parse_resize_str(geometry)?, "resize"))
        } else if let Some(signal) = atom.strip_prefix("signal:") {
            let signal = parse_signal_str(signal)?;
            Some((ControlCommand::Signal { signal }, "signal"))
        } else if let Some(label) = atom.strip_prefix("mark:") {
            let label = label.to_owned();
            Some((ControlCommand::Mark { label }, "mark"))
        } else {
            None
        };

        match control {
            Some((command, label)) => {
                if !atoms.is_empty() {
                    steps.push(SendStep::Act(take(&mut atoms)));
                }

                steps.push(SendStep::Control(command, label));
            }
            None if atom == "-" => atoms.push(InputAtom::Bytes(stdin_bytes.clone())),
            None => atoms.push(input_atom_for_cli(atom)?),
        }
    }

    if let Some(text) = &arguments.text {
        atoms.push(InputAtom::Text(text.clone()));
    }

    if let Some(path) = &arguments.file {
        atoms.push(InputAtom::Bytes(fs_err::read(path).into_diagnostic()?));
    }

    if arguments.stdin && !arguments.atoms.iter().any(|atom| return atom == "-") {
        atoms.push(InputAtom::Bytes(stdin_bytes));
    }

    if !atoms.is_empty() {
        steps.push(SendStep::Act(atoms));
    }
    return Ok(steps);
}

pub(in crate::cli) fn run_send(
    response: &mut ResponseWriter,
    arguments: SendArgs,
) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session.clone())?;
    if resolved.phase != SessionPhase::Live {
        return write_session_lifecycle(response, &resolved, "send", ExitCode::FAILURE);
    }

    let session = attach_session(&resolved.path)?;
    let request_id = arguments.request_id.unwrap_or_else(uuid::Uuid::now_v7);
    let guard = MutationGuard {
        request_id: Some(request_id),
        if_state: arguments.if_state.clone(),
    };
    let steps = send_steps(&arguments)?;

    if steps.is_empty() {
        return Err(miette::miette!(
            "send requires at least one input atom or flag"
        ));
    }

    let postcondition = action_condition(&arguments);
    let total_steps = steps.len();

    for (index, step) in steps.into_iter().enumerate() {
        let is_last = index + 1 == total_steps;
        match step {
            SendStep::Control(command, label) => {
                let mut request = ControlRequest::new(command);
                request.request_id = request_id;
                request
                    .expected_presented_state_hash
                    .clone_from(&arguments.if_state);

                if let Err(error) = session.control(&request) {
                    return write_send_failure(
                        response, &arguments, &session, label, request_id, &error,
                    );
                }

                if is_last {
                    return write_trailing_postcondition(
                        response,
                        &session,
                        &arguments,
                        label,
                        request_id,
                        &postcondition,
                    );
                }
            }
            SendStep::Act(atoms) => {
                let action = ActRequest {
                    atoms,
                    guard: guard.clone(),
                    intent: arguments.intent.clone(),
                    pace: arguments.pace,
                    verbose_atom_outcomes: arguments.verbose,
                    postcondition: if is_last {
                        postcondition.clone()
                    } else {
                        ActionCondition::None
                    },
                    projection: action_projection(&arguments),
                    allow_incomplete: true,
                };

                let outcome = match session.act(action) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        return write_send_failure(
                            response, &arguments, &session, "action", request_id, &error,
                        );
                    }
                };

                if !is_last {
                    continue;
                }

                let direct_stream = arguments.atoms.is_empty()
                    && (arguments.file.is_some() || arguments.stdin || arguments.text.is_some())
                    && postcondition == ActionCondition::None;

                if direct_stream {
                    return write_json_line(
                        response,
                        &serde_json::json!({
                            "kind": "input_sent",
                            "request_id": outcome.input.sequence_id,
                            "bytes_written": outcome.input.bytes_written,
                        }),
                    );
                }

                let output = action_applied_response(&outcome)?;
                if arguments.json {
                    write_json_pretty(response, &output)?;
                } else {
                    write_stdout(response, human_action_summary(&output))?;
                }

                return Ok(if outcome.matched() { ExitCode::SUCCESS } else { ExitCode::FAILURE });
            }
        }
    }

    return Ok(ExitCode::SUCCESS);
}

/// Wait for the requested postcondition after a trailing control command.
fn write_trailing_postcondition(
    response: &mut ResponseWriter,
    session: &Session,
    arguments: &SendArgs,
    label: &'static str,
    request_id: uuid::Uuid,
    postcondition: &ActionCondition,
) -> Result<ExitCode> {
    let (matched, kind, outcome) = match postcondition {
        ActionCondition::None => (true, "none", "matched".to_owned()),
        ActionCondition::Idle {
            stable_for,
            timeout,
        } => match session.capture_stable(StableCaptureOptions {
            stable_for: *stable_for,
            timeout: *timeout,
            projection: action_projection(arguments),
            allow_incomplete: true,
        }) {
            Ok(capture) => (
                capture.quality.is_settled(),
                "idle",
                capture.quality.as_str().to_owned(),
            ),
            Err(_) => (false, "idle", "error".to_owned()),
        },
        ActionCondition::Screen { predicate, timeout } => {
            match session.wait_screen(predicate.clone(), Duration::ZERO, *timeout) {
                Ok(wait) => (
                    wait.satisfied(),
                    "screen",
                    stable_enum_label(&wait.quality)?,
                ),
                Err(_) => (false, "screen", "error".to_owned()),
            }
        }
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
                    "kind": kind,
                    "matched": matched,
                    "outcome": outcome,
                }
            }),
        )?;
    } else {
        let status = if matched { "completed" } else { "failed" };
        write_stdout(
            response,
            format!("Send command {status} ({label}); postcondition {kind} matched={matched}, outcome={outcome}\n"),
        )?;
    }

    return Ok(if matched { ExitCode::SUCCESS } else { ExitCode::FAILURE });
}

fn write_send_failure(
    response: &mut ResponseWriter,
    arguments: &SendArgs,
    session: &Session,
    label: &'static str,
    request_id: uuid::Uuid,
    error: &SessionError,
) -> Result<ExitCode> {
    let output = action_error_response(request_id, error, is_completed(session.path()));

    if arguments.json {
        write_json_pretty(response, &output)?;
    } else {
        write_stdout(response, format!("Send {label} failed: {error}\n"))?;
    }

    return Ok(ExitCode::FAILURE);
}

pub(super) fn action_condition(arguments: &SendArgs) -> ActionCondition {
    let timeout = arguments.timeout.unwrap_or(defaults::WAIT_TIMEOUT);

    if let Some(stable_for) = arguments.until_idle {
        return ActionCondition::Idle {
            stable_for,
            timeout,
        };
    }

    if let Some(text) = &arguments.until_screen {
        return ActionCondition::Screen {
            predicate: ScreenPredicate::Contains { text: text.clone() },
            timeout,
        };
    }

    if let Some(pattern) = &arguments.until_screen_regex {
        return ActionCondition::Screen {
            predicate: ScreenPredicate::MatchesRegex {
                pattern: pattern.clone(),
            },
            timeout,
        };
    }
    return ActionCondition::None;
}

pub(super) fn action_projection(arguments: &SendArgs) -> ScreenProjectionRequest {
    if arguments.verbose {
        return ScreenProjectionRequest::default();
    }
    return hash_only_projection();
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

#[derive(serde::Serialize)]
pub(super) struct ActionInputSummary<'data> {
    pub(super) effect: MutationEffect,
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

pub(super) fn action_applied_response(outcome: &ActOutcome) -> Result<serde_json::Value> {
    let input = &outcome.input;
    let matched = outcome.matched();
    let (postcondition, ended) = match &outcome.postcondition {
        ActionPostconditionResult::None { matched, .. } => (
            ActionPostconditionSummary::None {
                matched: *matched,
                outcome: "matched",
            },
            false,
        ),
        ActionPostconditionResult::Idle {
            matched, quality, ..
        } => (
            ActionPostconditionSummary::Idle {
                matched: *matched,
                outcome: quality,
            },
            matches!(
                quality,
                StableCaptureQuality::ChildCompleted | StableCaptureQuality::OutputClosed
            ),
        ),
        ActionPostconditionResult::Screen {
            matched, quality, ..
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
        ),
    };

    let state_hash = outcome.state_hash();
    let output = ActionAppliedOutput {
        v: 1,
        ok: matched,
        response_type: "action",
        request_id: outcome.request_id,
        input: ActionInputSummary {
            effect: MutationEffect::Applied,
            atoms: input.atom_count,
            bytes: input.bytes_written,
            pace_ns: input.pace_ns,
            outcomes: (!input.atoms.is_empty()).then_some(&input.atoms),
        },
        postcondition,
        observation: ActionObservation {
            state: state_hash,
            state_hash,
            frame: outcome.frame(),
        },
        ended,
        boundaries: &outcome.boundaries,
        code: (!matched).then_some("postcondition_failed"),
    };

    return serde_json::to_value(&output).into_diagnostic();
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
    pub(super) effect: MutationEffect,
    pub(super) atoms: u32,
    pub(super) bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) prefix: Option<ActionFailedPrefix>,
    pub(super) retry: Retry,
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
    pub(super) code: &'data str,
    pub(super) message: String,
    pub(super) request_id: uuid::Uuid,
    pub(super) input: ActionFailedInputSummary,
    pub(super) observation: Option<ActionErrorObservation<'data>>,
    pub(super) ended: bool,
    pub(super) boundaries: Option<&'data ActorBoundaries>,
}

pub(super) fn action_error_response(
    request_id: uuid::Uuid,
    error: &SessionError,
    ended: bool,
) -> serde_json::Value {
    let effect = error.effect();
    let details = error.details();
    let prefix = details
        .and_then(|details| return details.input_sequence.as_ref())
        .filter(|_| return effect != MutationEffect::NotApplied)
        .map(|sequence| {
            return ActionFailedPrefix {
                atoms: sequence.atoms_applied,
                bytes: sequence.bytes_written,
                failed_atom: sequence.failed_atom_index,
                failed_operation_id: sequence.failed_operation_id,
                failed_atom_bytes: sequence.failed_atom_bytes_written,
                first_intent_event_seq: sequence.first_intent_event_seq,
                last_result_event_seq: sequence.last_result_event_seq,
                resume_from_atom: sequence.resume_from_atom_index,
            };
        });

    let output = ActionErrorOutput {
        v: 1,
        ok: false,
        response_type: "action",
        code: error.code(),
        message: error.to_string(),
        request_id,
        input: ActionFailedInputSummary {
            effect,
            atoms: prefix.as_ref().map_or(0, |prefix| return prefix.atoms),
            bytes: prefix.as_ref().map_or(0, |prefix| return prefix.bytes),
            prefix,
            retry: effect.retry(),
        },
        observation: details.map(|details| {
            return ActionErrorObservation {
                state: &details.current_presented_state_hash,
                scope: &details.scope,
            };
        }),
        ended,
        boundaries: details.map(|details| return &details.boundaries),
    };

    #[expect(
        clippy::expect_used,
        reason = "ActionErrorOutput contains only primitive serializable types and cannot fail JSON serialization"
    )]
    return serde_json::to_value(&output).expect("action error serialization");
}

#[cfg(test)]
mod action_response_tests {
    use super::{action_applied_response, action_error_response, action_projection};
    use crate::cli::control::args::SendArgs;
    use std::{path::PathBuf, time::Duration};
    use termtrace_model::{
        protocol::control::{ActionInputResult, ActionPostconditionResult, ControlErrorDetails},
        protocol::mutation::MutationEffect,
        terminal::observation::{ActorBoundaries, CaptureQuality},
        terminal::screen::FrameDetail,
        terminal::screen::{ScreenFrameSource, ScreenProjectionRequest},
    };
    use termtrace_sdk::{ActOutcome, Error};
    use termtrace_terminal::{project_screen_frame, GhosttyVt};

    fn boundaries() -> ActorBoundaries {
        return serde_json::from_value(serde_json::json!({
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
    }

    #[test]
    fn action_success_retains_input_observation_request_and_boundaries() {
        let request_id =
            uuid::Uuid::parse_str("018f47d2-7ad0-7000-8000-000000000001").expect("request UUID");

        let mut engine = GhosttyVt::new(80, 24, 0).expect("semantic engine");
        engine.write(b"current screen");
        let state = engine.snapshot().expect("semantic state");
        let frame = project_screen_frame(
            &state,
            &ScreenProjectionRequest::default(),
            ScreenFrameSource::Actor {
                session_id: request_id,
                boundaries: boundaries(),
                observed_monotonic_ns: 1,
            },
            true,
            CaptureQuality::Live,
        )
        .expect("rich frame");

        let input: ActionInputResult = serde_json::from_value(serde_json::json!({
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
        }))
        .expect("action input");
        let postcondition: ActionPostconditionResult = serde_json::from_value(serde_json::json!({
            "kind": "screen",
            "matched": false,
            "quality": "deadline",
            "frame": frame
        }))
        .expect("postcondition");
        let outcome = ActOutcome {
            request_id,
            input,
            postcondition,
            boundaries: boundaries(),
        };

        let response = action_applied_response(&outcome).expect("action response");
        assert_eq!(response["ok"], false);
        assert_eq!(response["code"], "postcondition_failed");
        assert_eq!(response["request_id"], request_id.to_string());
        assert_eq!(response["input"]["effect"]["kind"], "applied");
        assert_eq!(response["input"]["atoms"], 2);
        assert_eq!(response["input"]["bytes"], 4);
        assert_eq!(
            response["input"]["outcomes"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(response["postcondition"]["outcome"], "deadline");
        assert_eq!(response["ended"], false);
        assert_eq!(response["observation"]["state"], frame.state_hash);
        assert!(response["observation"]["data"]["rows"][0]["text"]
            .as_str()
            .is_some_and(|text| return text.starts_with("current screen")));
        assert_eq!(response["boundaries"]["observed"]["next_event_seq"], 5);
    }

    #[test]
    fn action_partial_failure_retains_the_exact_attempted_prefix() {
        let request_id =
            uuid::Uuid::parse_str("018f47d2-7ad0-7000-8000-000000000010").expect("request UUID");

        let details: ControlErrorDetails = serde_json::from_value(serde_json::json!({
            "current_presented_state_hash": "d".repeat(64),
            "boundaries": boundaries(),
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
        let error = Error::Control(termtrace_control::Error::Rejected {
            code: "input_sequence_incomplete".to_owned(),
            message: "input stopped".to_owned(),
            effect: MutationEffect::Unknown,
            details: Some(Box::new(details)),
        });

        let response = action_error_response(request_id, &error, false);
        assert_eq!(response["code"], "input_sequence_incomplete");
        assert_eq!(response["request_id"], request_id.to_string());
        assert_eq!(response["input"]["effect"]["kind"], "unknown");
        assert_eq!(response["input"]["retry"], "same_request_id");
        assert_eq!(response["input"]["atoms"], 1);
        assert_eq!(response["input"]["bytes"], 5);
        assert_eq!(response["input"]["prefix"]["failed_atom"], 1);
        assert_eq!(response["input"]["prefix"]["failed_atom_bytes"], 2);
        assert_eq!(response["input"]["prefix"]["resume_from_atom"], 2);
        assert_eq!(response["observation"]["state"], "d".repeat(64));
        assert_eq!(response["boundaries"]["observed"]["next_event_seq"], 5);
    }

    fn send_args(verbose: bool) -> SendArgs {
        return SendArgs {
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
            timeout: None,
            pace: Duration::ZERO,
            intent: None,
            if_state: None,
            request_id: None,
            verbose,
            json: false,
        };
    }

    #[test]
    fn action_projection_selects_lightweight_detail_by_default() {
        assert_eq!(
            action_projection(&send_args(false)).detail,
            FrameDetail::None
        );
    }

    #[test]
    fn action_projection_preserves_full_rich_spans_when_verbose_requested() {
        let projection = action_projection(&send_args(true));
        assert_eq!(projection.detail, FrameDetail::Spans);
        assert!(projection.region.is_none());
    }
}
