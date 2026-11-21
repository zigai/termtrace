use super::args::{SendArgs, SendScreen};
use super::input::parse_atom;
use super::*;
use std::{fmt::Write as _, mem::take};
use termtrace_model::{
    protocol::control::{PointerOperation, ProcessSignal},
    protocol::mutation::{MutationEffect, Retry},
    terminal::diff::ScreenChanges,
    terminal::screen::{FrameDetail, ScreenFrameContent},
    terminal::state::MouseButton,
};
use termtrace_sdk::session::{
    CommandPostcondition, SendFailure, SendLastStep, SendOutcome, SendRequest, SendStep,
    WaitAtomFailure,
};

fn parse_click_str(s: &str) -> Result<(u16, u16)> {
    let (x_str, y_str) = s
        .split_once(',')
        .ok_or_else(|| miette::miette!("invalid click coordinates '{s}'; expected X,Y"))?;
    let col = x_str.trim().parse::<u16>().into_diagnostic()?;
    let row = y_str.trim().parse::<u16>().into_diagnostic()?;

    return Ok((col, row));
}

/// Motion samples a `drag:` atom interpolates between its two cells.
const DRAG_STEPS: u8 = 1;

fn parse_scroll_str(s: &str) -> Result<(u16, u16, i16)> {
    let mut parts = s.split(',');
    let column = parts
        .next()
        .ok_or_else(|| {
            return miette::miette!("invalid scroll format '{s}'; expected COL,ROW,LINES");
        })?
        .trim()
        .parse::<u16>()
        .into_diagnostic()?;
    let row = parts
        .next()
        .ok_or_else(|| {
            return miette::miette!("invalid scroll format '{s}'; expected COL,ROW,LINES");
        })?
        .trim()
        .parse::<u16>()
        .into_diagnostic()?;
    let lines = parts
        .next()
        .ok_or_else(|| {
            return miette::miette!("invalid scroll format '{s}'; expected COL,ROW,LINES");
        })?
        .trim()
        .parse::<i16>()
        .into_diagnostic()?;

    if parts.next().is_some() {
        return Err(miette::miette!(
            "invalid scroll format '{s}'; expected COL,ROW,LINES"
        ));
    }

    if lines == 0 {
        return Err(miette::miette!(
            "invalid scroll lines 0; negative scrolls up and positive scrolls down"
        ));
    }
    return Ok((column, row, lines));
}

fn parse_drag_str(s: &str) -> Result<((u16, u16), (u16, u16))> {
    let coordinates: Vec<&str> = s.split(',').collect();
    if coordinates.len() != 4 {
        return Err(miette::miette!(
            "invalid drag format '{s}'; expected COL,ROW,COL,ROW"
        ));
    }

    let mut cells = coordinates.into_iter().map(|value| {
        return value.trim().parse::<u16>().into_diagnostic();
    });
    let from_column = cells.next().transpose()?.unwrap_or_default();
    let from_row = cells.next().transpose()?.unwrap_or_default();
    let to_column = cells.next().transpose()?.unwrap_or_default();
    let to_row = cells.next().transpose()?.unwrap_or_default();

    return Ok(((from_column, from_row), (to_column, to_row)));
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

struct PlannedStep {
    step: SendStep,
    label: &'static str,
    wait: Option<PlannedWait>,
}

struct PlannedWait {
    atom_index: usize,
    atom: String,
}

enum AtomAction {
    Command(ControlCommand, &'static str),
    Wait(ScreenPredicate),
    Input,
}

/// Ordered `send` steps, each with the label reported for it and the wait atom behind it.
fn send_steps(arguments: &SendArgs) -> Result<Vec<PlannedStep>> {
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
        steps.push(planned(
            SendStep::Command(parse_resize_str(resize)?),
            "resize",
        ));
    }

    if let Some(signal) = &arguments.signal {
        let signal = parse_signal_str(signal)?;
        steps.push(planned(
            SendStep::Command(ControlCommand::Signal { signal }),
            "signal",
        ));
    }

    if let Some(label) = &arguments.mark {
        let label = label.clone();
        steps.push(planned(
            SendStep::Command(ControlCommand::Mark { label }),
            "mark",
        ));
    }

    for (index, atom) in arguments.atoms.iter().enumerate() {
        let action = parse_action_atom(atom)?;

        match action {
            AtomAction::Command(command, label) => {
                flush_input(&mut steps, &mut atoms);
                steps.push(planned(SendStep::Command(command), label));
            }
            AtomAction::Wait(predicate) => {
                flush_input(&mut steps, &mut atoms);
                steps.push(PlannedStep {
                    step: SendStep::Wait(predicate),
                    label: "wait",
                    wait: Some(PlannedWait {
                        atom_index: index,
                        atom: atom.clone(),
                    }),
                });
            }
            AtomAction::Input => {
                if atom == "-" {
                    atoms.push(InputAtom::Bytes(stdin_bytes.clone()));
                } else {
                    atoms.push(parse_atom(atom)?);
                }
            }
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

    flush_input(&mut steps, &mut atoms);
    return Ok(steps);
}

fn planned(step: SendStep, label: &'static str) -> PlannedStep {
    return PlannedStep {
        step,
        label,
        wait: None,
    };
}

fn flush_input(steps: &mut Vec<PlannedStep>, atoms: &mut Vec<InputAtom>) {
    if !atoms.is_empty() {
        steps.push(planned(SendStep::Input(take(atoms)), "action"));
    }
}

fn parse_action_atom(atom: &str) -> Result<AtomAction> {
    if let Some(coordinates) = atom.strip_prefix("click:") {
        let (column, row) = parse_click_str(coordinates)?;
        let operation = PointerOperation::Click {
            button: MouseButton::Left,
        };

        return Ok(AtomAction::Command(
            pointer(column, row, operation),
            "pointer",
        ));
    }

    if let Some(coordinates) = atom.strip_prefix("hover:") {
        let (column, row) = parse_click_str(coordinates)?;

        return Ok(AtomAction::Command(
            pointer(column, row, PointerOperation::Move),
            "pointer",
        ));
    }

    if let Some(coordinates) = atom.strip_prefix("scroll:") {
        let (column, row, lines) = parse_scroll_str(coordinates)?;

        return Ok(AtomAction::Command(
            pointer(column, row, PointerOperation::Scroll { lines }),
            "pointer",
        ));
    }

    if let Some(cells) = atom.strip_prefix("drag:") {
        let ((column, row), (to_column, to_row)) = parse_drag_str(cells)?;
        let operation = PointerOperation::Drag {
            button: MouseButton::Left,
            to_column,
            to_row,
            steps: DRAG_STEPS,
        };

        return Ok(AtomAction::Command(
            pointer(column, row, operation),
            "pointer",
        ));
    }

    if let Some(text) = atom.strip_prefix("activate:") {
        return Ok(AtomAction::Command(
            ControlCommand::Activate {
                text: text.to_owned(),
                unique: false,
                strict: false,
            },
            "activate",
        ));
    }

    if let Some(geometry) = atom.strip_prefix("resize:") {
        return Ok(AtomAction::Command(parse_resize_str(geometry)?, "resize"));
    }

    if let Some(signal) = atom.strip_prefix("signal:") {
        return Ok(AtomAction::Command(
            ControlCommand::Signal {
                signal: parse_signal_str(signal)?,
            },
            "signal",
        ));
    }

    if let Some(label) = atom.strip_prefix("mark:") {
        return Ok(AtomAction::Command(
            ControlCommand::Mark {
                label: label.to_owned(),
            },
            "mark",
        ));
    }

    if let Some(text) = atom.strip_prefix("wait:") {
        return Ok(AtomAction::Wait(ScreenPredicate::Contains {
            text: text.to_owned(),
        }));
    }
    return Ok(AtomAction::Input);
}

fn pointer(column: u16, row: u16, operation: PointerOperation) -> ControlCommand {
    return ControlCommand::Pointer {
        column,
        row,
        operation,
        strict: false,
    };
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
    let planned = send_steps(&arguments)?;
    if planned.is_empty() {
        return Err(miette::miette!(
            "send requires at least one input atom or flag"
        ));
    }

    let request = SendRequest {
        steps: planned
            .iter()
            .map(|plan| return plan.step.clone())
            .collect(),
        guard: MutationGuard {
            request_id: arguments.request_id,
            if_state: arguments.if_state.clone(),
        },
        intent: arguments.intent.clone(),
        pace: arguments.pace,
        verbose_atom_outcomes: arguments.verbose,
        postcondition: condition(&arguments),
        projection: projection(&arguments),
        report_changes: arguments.screen == SendScreen::Diff,
        wait_timeout: arguments.timeout.unwrap_or(defaults::WAIT_TIMEOUT),
    };
    let outcome = match session.send(request) {
        Ok(outcome) => outcome,
        Err(failure) => {
            return write_send_failure(response, &arguments, &session, &planned, failure)
        }
    };

    let SendOutcome {
        last,
        changes,
        changing_rows,
    } = outcome;

    let output_wait = output_postcondition(&arguments, &resolved);
    let report_changes = arguments.screen == SendScreen::Diff;

    let outcome = match last {
        SendLastStep::Command {
            request_id,
            postcondition,
            ..
        } => {
            return write_trailing_postcondition(
                response,
                &arguments,
                &session,
                TrailingReport {
                    label: planned.last().map_or("send", |plan| return plan.label),
                    request_id,
                    postcondition,
                    changes: changes.as_ref(),
                    frame: None,
                    changing_rows: changing_rows.as_deref(),
                    output_wait: output_wait.as_ref(),
                },
            );
        }
        SendLastStep::Wait {
            request_id,
            frame,
            postcondition,
        } => {
            return write_trailing_postcondition(
                response,
                &arguments,
                &session,
                TrailingReport {
                    label: "wait",
                    request_id,
                    postcondition,
                    changes: changes.as_ref(),
                    frame: Some(&frame),
                    changing_rows: changing_rows.as_deref(),
                    output_wait: output_wait.as_ref(),
                },
            );
        }
        SendLastStep::Action(outcome) => outcome,
    };

    let matched =
        outcome.is_matched() && output_wait.as_ref().is_none_or(|wait| return wait.matched);

    let mut output = applied_response(&outcome, arguments.verbose)?;

    if let Some(wait) = &output_wait {
        apply_output_postcondition(&mut output, wait);
    }

    if arguments.screen == SendScreen::Text || !matched {
        insert_screen(&mut output, outcome.frame());
    }

    if let Some(rows) = changing_rows.as_deref() {
        insert_changing_rows(&mut output, rows);
    }

    if arguments.json {
        if report_changes {
            insert_changes(&mut output, changes.as_ref())?;
        }

        write_json_pretty(response, &output)?;
    } else {
        let mut summary = human_action_summary(&output);

        if arguments.screen == SendScreen::State
            && let Some(state) = outcome.state_hash()
        {
            writeln!(summary, "State: {state}").into_diagnostic()?;
        }

        if let Some(rows) = changing_rows.as_deref() {
            writeln!(summary, "{}", changing_rows_notice(rows)).into_diagnostic()?;
        }

        summary.push_str(&human_send_screen(
            arguments.screen,
            changes.as_ref(),
            outcome.frame(),
        )?);
        write_stdout(response, summary)?;
    }

    return Ok(if matched { ExitCode::SUCCESS } else { ExitCode::FAILURE });
}

struct OutputWait {
    matched: bool,
    outcome: &'static str,
}

fn output_postcondition(arguments: &SendArgs, resolved: &ResolvedSessionRef) -> Option<OutputWait> {
    let text = arguments.until_output.as_ref()?;
    let timeout = arguments.timeout.unwrap_or(defaults::WAIT_TIMEOUT);

    if wait_for_output(&resolved.path, text.as_bytes(), timeout).is_ok() {
        return Some(OutputWait {
            matched: true,
            outcome: "matched",
        });
    }

    let ended = resolve_session(resolved.path.clone())
        .is_ok_and(|session| return session.phase != SessionPhase::Live);

    return Some(OutputWait {
        matched: false,
        outcome: if ended { "child_completed" } else { "deadline" },
    });
}

fn apply_output_postcondition(output: &mut serde_json::Value, wait: &OutputWait) {
    if let Some(object) = output.as_object_mut() {
        object.insert(
            "postcondition".to_owned(),
            serde_json::json!({
                "kind": "output",
                "matched": wait.matched,
                "outcome": wait.outcome,
            }),
        );
        object.insert("ok".to_owned(), serde_json::json!(wait.matched));

        if wait.matched {
            object.remove("code");
        } else {
            object.insert("code".to_owned(), serde_json::json!("postcondition_failed"));
        }
    }
}

fn insert_changing_rows(output: &mut serde_json::Value, rows: &[u16]) {
    if let Some(object) = output.as_object_mut() {
        object.insert("changing_rows".to_owned(), serde_json::json!(rows));
    }
}

fn human_frame_summary(frame: &ScreenFrame) -> String {
    return human_screen_summary(&serde_json::json!({
        "state": frame.state_hash,
        "size": [frame.geometry.cols, frame.geometry.rows],
        "quality": frame.capture_quality,
        "screen": frame_text(frame),
    }));
}

pub(super) fn frame_text(frame: &ScreenFrame) -> String {
    let rows: Vec<&str> = match &frame.data {
        ScreenFrameContent::None => Vec::new(),
        ScreenFrameContent::Text { rows } => {
            rows.iter().map(|row| return row.text.as_str()).collect()
        }
        ScreenFrameContent::Spans { rows } => {
            rows.iter().map(|row| return row.text.as_str()).collect()
        }
        ScreenFrameContent::Cells { rows } => {
            rows.iter().map(|row| return row.text.as_str()).collect()
        }
    };

    let viewport = rows
        .into_iter()
        .map(|row| return row.trim_end_matches(' '))
        .collect::<Vec<_>>()
        .join("\n");
    return viewport.trim_end_matches('\n').to_owned();
}

fn insert_screen(output: &mut serde_json::Value, frame: Option<&ScreenFrame>) {
    if let (Some(object), Some(frame)) = (output.as_object_mut(), frame) {
        object.insert("screen".to_owned(), serde_json::json!(frame_text(frame)));
        object.insert(
            "size".to_owned(),
            serde_json::json!([frame.geometry.cols, frame.geometry.rows]),
        );
    }
}

fn human_send_screen(
    screen: SendScreen,
    changes: Option<&ScreenChanges>,
    frame: Option<&ScreenFrame>,
) -> Result<String> {
    return Ok(match (screen, changes, frame) {
        (SendScreen::State, _, _) => String::new(),
        (SendScreen::Diff, Some(changes), Some(frame))
            if !changes_need_full_screen(changes, frame) =>
        {
            human_screen_changes(changes, Some(frame))?
        }
        (SendScreen::Diff, Some(changes), None) => human_screen_changes(changes, None)?,
        (_, _, Some(frame)) => human_frame_summary(frame),
        (_, _, None) => String::new(),
    });
}

fn changes_need_full_screen(changes: &ScreenChanges, frame: &ScreenFrame) -> bool {
    let switched_screen = changes
        .modes
        .as_ref()
        .is_some_and(|modes| return modes.before.alternate_screen != modes.after.alternate_screen);

    let reported_lines = changes.rows.len().saturating_add(changes.moves.len());

    return changes.geometry.is_some()
        || switched_screen
        || reported_lines.saturating_mul(5) > usize::from(frame.geometry.rows).saturating_mul(2);
}

fn insert_changes(output: &mut serde_json::Value, changes: Option<&ScreenChanges>) -> Result<()> {
    if let (Some(object), Some(changes)) = (output.as_object_mut(), changes) {
        object.insert(
            "changes".to_owned(),
            serde_json::to_value(changes).into_diagnostic()?,
        );
    }

    return Ok(());
}

struct TrailingReport<'data> {
    label: &'static str,
    request_id: uuid::Uuid,
    postcondition: Option<Box<CommandPostcondition>>,
    changes: Option<&'data ScreenChanges>,
    frame: Option<&'data ScreenFrame>,
    changing_rows: Option<&'data [u16]>,
    output_wait: Option<&'data OutputWait>,
}

/// Report the postcondition checked after a trailing control command or wait.
fn write_trailing_postcondition(
    response: &mut ResponseWriter,
    arguments: &SendArgs,
    session: &Session,
    report: TrailingReport<'_>,
) -> Result<ExitCode> {
    let TrailingReport {
        label,
        request_id,
        postcondition,
        changes,
        frame,
        changing_rows,
        output_wait,
    } = report;
    let frame = match postcondition.as_deref() {
        Some(CommandPostcondition::Idle(Ok(capture))) => Some(capture.frame.clone()),
        Some(CommandPostcondition::Screen(Ok(wait))) => Some(wait.frame.clone()),
        _ => frame
            .cloned()
            .or_else(|| return session.frame(projection(arguments)).ok()),
    };

    let (postcondition_matched, kind, outcome) =
        match postcondition.map(|postcondition| return *postcondition) {
            None => (true, "none", "matched".to_owned()),
            Some(CommandPostcondition::Idle(Ok(capture))) => (
                capture.quality.is_settled(),
                "idle",
                capture.quality.as_str().to_owned(),
            ),
            Some(CommandPostcondition::Idle(Err(_))) => (false, "idle", "error".to_owned()),
            Some(CommandPostcondition::Screen(Ok(wait))) => (
                wait.is_satisfied(),
                "screen",
                stable_enum_label(&wait.quality)?,
            ),
            Some(CommandPostcondition::Screen(Err(_))) => (false, "screen", "error".to_owned()),
        };
    let matched = output_wait.map_or(postcondition_matched, |wait| return wait.matched);
    let report_changes = arguments.screen == SendScreen::Diff;

    if arguments.json {
        let mut output = serde_json::json!({
            "v": 1,
            "ok": matched,
            "type": label,
            "request_id": request_id,
            "postcondition": {
                "kind": kind,
                "matched": matched,
                "outcome": outcome,
            }
        });

        if let Some(wait) = output_wait {
            apply_output_postcondition(&mut output, wait);
        } else if !matched && let Some(object) = output.as_object_mut() {
            object.insert("code".to_owned(), serde_json::json!("postcondition_failed"));
        }

        if report_changes {
            insert_changes(&mut output, changes)?;
        }

        if arguments.screen == SendScreen::Text || !matched {
            insert_screen(&mut output, frame.as_ref());
        }

        if let Some(rows) = changing_rows {
            insert_changing_rows(&mut output, rows);
        }

        if let Some(frame) = &frame
            && let Some(object) = output.as_object_mut()
        {
            object.insert(
                "observation".to_owned(),
                serde_json::json!({"state": frame.state_hash}),
            );
        }

        if matched && let Some(frame) = &frame {
            let predicate = arguments
                .until_screen
                .as_ref()
                .map(|text| return ScreenPredicate::Contains { text: text.clone() })
                .or_else(|| {
                    return arguments.until_screen_regex.as_ref().map(|pattern| {
                        return ScreenPredicate::MatchesRegex {
                            pattern: pattern.clone(),
                        };
                    });
                });

            if let Some(predicate) = predicate
                && let Some((row, text)) = frame_match(frame, &predicate)?
            {
                let postcondition = output
                    .get_mut("postcondition")
                    .and_then(serde_json::Value::as_object_mut)
                    .ok_or_else(|| {
                        return miette::miette!("send response has no postcondition object");
                    })?;
                postcondition.insert("matched_row".to_owned(), serde_json::json!(row));
                postcondition.insert("matched_text".to_owned(), serde_json::json!(text));
            }
        }

        write_json_pretty(response, &output)?;
    } else {
        let status = if matched { "completed" } else { "failed" };
        let mut summary = format!(
            "Send command {status} ({label}); postcondition {kind} matched={matched}, outcome={outcome}\n"
        );

        if arguments.screen == SendScreen::State
            && let Some(frame) = &frame
        {
            writeln!(summary, "State: {}", frame.state_hash).into_diagnostic()?;
        }

        if let Some(rows) = changing_rows {
            writeln!(summary, "{}", changing_rows_notice(rows)).into_diagnostic()?;
        }

        summary.push_str(&human_send_screen(
            arguments.screen,
            changes,
            frame.as_ref(),
        )?);
        write_stdout(response, summary)?;
    }

    return Ok(if matched { ExitCode::SUCCESS } else { ExitCode::FAILURE });
}

fn frame_match(frame: &ScreenFrame, predicate: &ScreenPredicate) -> Result<Option<(u16, String)>> {
    let rows: Vec<(&str, u16, bool)> = match &frame.data {
        ScreenFrameContent::Text { rows } => rows
            .iter()
            .map(|row| return (row.text.as_str(), row.row, row.soft_wrapped))
            .collect(),
        ScreenFrameContent::Spans { rows } => rows
            .iter()
            .map(|row| return (row.text.as_str(), row.row, row.soft_wrapped))
            .collect(),
        ScreenFrameContent::Cells { rows } => rows
            .iter()
            .map(|row| return (row.text.as_str(), row.row, row.soft_wrapped))
            .collect(),
        ScreenFrameContent::None => Vec::new(),
    };

    let mut text = String::new();
    let mut starts = Vec::new();

    for (line, row, wrapped) in rows {
        starts.push((text.len(), row));
        text.push_str(if wrapped { line } else { line.trim_end_matches(' ') });

        if !wrapped {
            text.push('\n');
        }
    }

    let compiled = predicate.compile().into_diagnostic()?;
    let matched = compiled
        .text_matches(&text)
        .and_then(|matches| return matches.last().cloned());

    return Ok(matched.and_then(|matched| {
        let row = starts
            .iter()
            .rev()
            .find(|(offset, _)| return *offset <= matched.start)?
            .1;

        return Some((row, text.get(matched)?.to_owned()));
    }));
}

fn write_send_failure(
    response: &mut ResponseWriter,
    arguments: &SendArgs,
    session: &Session,
    planned: &[PlannedStep],
    failure: SendFailure,
) -> Result<ExitCode> {
    let SendFailure {
        step,
        request_id,
        error,
        wait_atom,
    } = failure;
    let label = planned.get(step).map_or("send", |plan| return plan.label);

    if let Some(wait_atom) = wait_atom {
        let planned_wait = planned.get(step).and_then(|plan| return plan.wait.as_ref());

        return write_wait_atom_failure(
            response,
            arguments,
            request_id,
            planned_wait,
            wait_atom.as_ref(),
        );
    }

    let ended = session
        .completion()
        .is_ok_and(|completion| return completion.is_some());

    let mut output = error_response(request_id, &error, ended, arguments.verbose);
    let frame = session
        .frame(ScreenProjectionRequest {
            detail: FrameDetail::Text,
            region: None,
        })
        .ok();
    insert_screen(&mut output, frame.as_ref());

    if arguments.json {
        write_json_pretty(response, &output)?;
    } else {
        write_stdout(response, format!("Send {label} failed: {error}\n"))?;
    }

    return Ok(ExitCode::FAILURE);
}

fn write_wait_atom_failure(
    response: &mut ResponseWriter,
    arguments: &SendArgs,
    request_id: uuid::Uuid,
    planned: Option<&PlannedWait>,
    failure: &WaitAtomFailure,
) -> Result<ExitCode> {
    let atom_index = planned.map_or(0, |wait| return wait.atom_index);
    let atom = planned.map_or("wait:", |wait| return wait.atom.as_str());
    let frame = &failure.frame;

    if arguments.json {
        let output = serde_json::json!({
            "v": 1,
            "ok": false,
            "type": "action",
            "code": "wait_atom_failed",
            "request_id": request_id,
            "atom": atom,
            "atom_index": atom_index,
            "outcome": failure.outcome.as_str(),
            "state": frame.state_hash,
            "size": [frame.geometry.cols, frame.geometry.rows],
            "screen": frame_text(frame),
        });

        write_json_pretty(response, &output)?;
    } else {
        write_stdout(
            response,
            format!(
                "Send stopped at {atom} (atom {atom_index}): the screen did not contain the text; outcome {}.\n{}\n",
                failure.outcome.as_str(),
                frame_text(frame)
            ),
        )?;
    }

    return Ok(ExitCode::FAILURE);
}

pub(super) fn condition(arguments: &SendArgs) -> ActionCondition {
    let timeout = arguments.timeout.unwrap_or(defaults::WAIT_TIMEOUT);

    if let Some(stable_for) = arguments.until_idle {
        return ActionCondition::Idle {
            stable_for,
            timeout,
            reaction: Reaction {
                changed_from: None,
                timeout: arguments
                    .reaction_timeout
                    .unwrap_or(defaults::REACTION_TIMEOUT),
            },
        };
    }

    if let Some(text) = &arguments.until_screen {
        return ActionCondition::Screen {
            predicate: ScreenPredicate::Contains { text: text.clone() },
            timeout,
            settle: arguments.settle.unwrap_or_default(),
        };
    }

    if let Some(text) = &arguments.until_screen_absent {
        return ActionCondition::Screen {
            predicate: ScreenPredicate::Absent { text: text.clone() },
            timeout,
            settle: arguments.settle.unwrap_or_default(),
        };
    }

    if let Some(pattern) = &arguments.until_screen_regex {
        return ActionCondition::Screen {
            predicate: ScreenPredicate::MatchesRegex {
                pattern: pattern.clone(),
            },
            timeout,
            settle: arguments.settle.unwrap_or_default(),
        };
    }
    return ActionCondition::None;
}

pub(super) fn projection(arguments: &SendArgs) -> ScreenProjectionRequest {
    if arguments.verbose {
        return ScreenProjectionRequest::default();
    }

    if arguments.screen == SendScreen::State
        && arguments.until_screen.is_none()
        && arguments.until_screen_absent.is_none()
        && arguments.until_screen_regex.is_none()
        && arguments.until_idle.is_none()
        && arguments.until_output.is_none()
    {
        return hash_only_projection();
    }
    return ScreenProjectionRequest {
        detail: FrameDetail::Text,
        region: None,
    };
}

#[derive(serde::Serialize)]
pub(super) struct ActionObservation<'data> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) state: Option<&'data str>,
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
        #[serde(skip_serializing_if = "Option::is_none")]
        matched_row: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        matched_text: Option<&'data str>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) boundaries: Option<&'data ActorBoundaries>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) code: Option<&'static str>,
}

pub(super) fn applied_response(
    outcome: &ActOutcome,
    include_details: bool,
) -> Result<serde_json::Value> {
    let input = &outcome.input;
    let matched = outcome.is_matched();
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
            matched,
            quality,
            matched_row,
            matched_text,
            ..
        } => (
            ActionPostconditionSummary::Screen {
                matched: *matched,
                outcome: quality,
                matched_row: *matched_row,
                matched_text: matched_text.as_deref(),
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
            frame: outcome.frame().filter(|_| return include_details),
        },
        ended,
        boundaries: include_details.then_some(&outcome.boundaries),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) boundaries: Option<&'data ActorBoundaries>,
}

pub(super) fn error_response(
    request_id: uuid::Uuid,
    error: &SessionError,
    ended: bool,
    verbose: bool,
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
        boundaries: details
            .filter(|_| return verbose)
            .map(|details| return &details.boundaries),
    };
    #[expect(
        clippy::expect_used,
        reason = "ActionErrorOutput contains only primitive serializable types and cannot fail JSON serialization"
    )]
    return serde_json::to_value(&output).expect("action error serialization");
}

#[cfg(test)]
mod action_response_tests {
    use super::{applied_response, error_response, projection};
    use crate::cli::control::args::{SendArgs, SendScreen};
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
                state_id: 0,
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
        let response = applied_response(&outcome, true).expect("action response");

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

        let error = Error::Control(termtrace_service::ControlError::Rejected {
            code: "input_sequence_incomplete".to_owned(),
            message: "input stopped".to_owned(),
            effect: MutationEffect::Unknown,
            details: Some(Box::new(details)),
        });

        let response = error_response(request_id, &error, false, true);

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

    fn send_args(verbose: bool, json: bool) -> SendArgs {
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
            until_screen_absent: None,
            until_screen_regex: None,
            until_output: None,
            until_idle: None,
            reaction_timeout: None,
            timeout: None,
            settle: None,
            pace: Duration::ZERO,
            intent: None,
            if_state: None,
            request_id: None,
            verbose,
            screen: SendScreen::Text,
            json,
        };
    }

    #[test]
    fn action_projection_selects_screen_text_for_human_output() {
        assert_eq!(
            projection(&send_args(false, false)).detail,
            FrameDetail::Text
        );
    }

    #[test]
    fn action_projection_preserves_full_rich_spans_when_verbose_requested() {
        let projection = projection(&send_args(true, false));
        assert_eq!(projection.detail, FrameDetail::Spans);
        assert!(projection.region.is_none());
    }
}
