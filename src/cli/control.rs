use super::*;

pub(super) fn run_wait(arguments: WaitArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    let timeout = Duration::from_millis(arguments.timeout_ms);
    if let Some(output) = arguments.output {
        if !arguments.stable_for.is_zero() {
            return Err(miette::miette!(
                "--stable-for applies only to screen predicates"
            ));
        }
        return write_json_line(&wait_for_output(session, output.as_bytes(), timeout)?);
    }
    let predicate = match (
        arguments.screen,
        arguments.screen_absent,
        arguments.screen_regex,
        arguments.row_contains,
        arguments.region_contains,
        arguments.cursor_at,
        arguments.cursor_visible,
        arguments.cursor_shape,
        arguments.mode,
        arguments.mode_disabled,
        arguments.state_changed,
        arguments.state_unchanged,
    ) {
        (Some(text), None, None, None, None, None, None, None, None, None, None, None) => {
            ScreenPredicate::Contains { text }
        }
        (None, Some(text), None, None, None, None, None, None, None, None, None, None) => {
            ScreenPredicate::Absent { text }
        }
        (None, None, Some(pattern), None, None, None, None, None, None, None, None, None) => {
            ScreenPredicate::MatchesRegex { pattern }
        }
        (None, None, None, Some(value), None, None, None, None, None, None, None, None) => {
            let (row, text) = parse_row_predicate(&value)?;
            ScreenPredicate::RowContains { row, text }
        }
        (None, None, None, None, Some(value), None, None, None, None, None, None, None) => {
            let (region, text) = parse_region_predicate(&value)?;
            ScreenPredicate::RegionContains { region, text }
        }
        (None, None, None, None, None, Some(value), None, None, None, None, None, None) => {
            let (column, row) = parse_cursor_position(&value)?;
            ScreenPredicate::Cursor {
                column: Some(column),
                row: Some(row),
                visible: None,
                shape: None,
            }
        }
        (None, None, None, None, None, None, Some(visible), None, None, None, None, None) => {
            ScreenPredicate::Cursor {
                column: None,
                row: None,
                visible: Some(visible),
                shape: None,
            }
        }
        (None, None, None, None, None, None, None, Some(shape), None, None, None, None) => {
            ScreenPredicate::Cursor {
                column: None,
                row: None,
                visible: None,
                shape: Some(shape.into()),
            }
        }
        (None, None, None, None, None, None, None, None, Some(mode), None, None, None) => {
            ScreenPredicate::Mode {
                mode: mode.into(),
                enabled: true,
            }
        }
        (None, None, None, None, None, None, None, None, None, Some(mode), None, None) => {
            ScreenPredicate::Mode {
                mode: mode.into(),
                enabled: false,
            }
        }
        (None, None, None, None, None, None, None, None, None, None, Some(hash), None) => {
            ScreenPredicate::StateHash {
                hash,
                comparison: StateHashComparison::Changed,
            }
        }
        (None, None, None, None, None, None, None, None, None, None, None, Some(hash)) => {
            ScreenPredicate::StateHash {
                hash,
                comparison: StateHashComparison::Unchanged,
            }
        }
        _ => return Err(miette::miette!("exactly one wait predicate is required")),
    };
    return write_json_line(&wait_for_screen(
        session,
        predicate,
        arguments.stable_for,
        timeout,
    )?);
}

pub(super) fn run_wait_idle(arguments: WaitIdleArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    let data = send_request(
        session,
        &ControlRequest::new(ControlCommand::CaptureStable {
            stable_for_ns: u64::try_from(arguments.stable_for.as_nanos()).unwrap_or(u64::MAX),
            timeout_ns: arguments.timeout_ms.saturating_mul(1_000_000),
        }),
    )?;
    let ControlData::StableSnapshot {
        semantic_state_zstd: _,
        viewport_text,
        state_hash,
        cols,
        rows,
        cursor_column,
        cursor_row,
        cursor_visible,
        first_observed_monotonic_ns,
        observed_monotonic_ns,
        stable_for_ns,
        quality,
        modes,
        boundaries,
    } = &data
    else {
        return Err(miette::miette!(
            "recording actor returned an unexpected stable-capture payload"
        ));
    };
    if !arguments.allow_incomplete && *quality != StableCaptureQuality::Stable {
        return Err(miette::miette!(
            "stable capture ended with {}; pass --allow-incomplete to receive the typed snapshot",
            quality.as_str()
        ));
    }
    if arguments.full_state {
        return write_json_line(&data);
    }
    return write_json_line(&serde_json::json!({
        "kind": "stable_snapshot",
        "quality": quality,
        "state_hash": state_hash,
        "viewport_text": viewport_text,
        "geometry": {
            "cols": cols,
            "rows": rows,
        },
        "cursor": {
            "column": cursor_column,
            "row": cursor_row,
            "visible": cursor_visible,
        },
        "first_observed_monotonic_ns": first_observed_monotonic_ns,
        "observed_monotonic_ns": observed_monotonic_ns,
        "stable_for_ns": stable_for_ns,
        "modes": modes,
        "boundary": compact_boundaries(boundaries),
    }));
}

fn compact_boundaries(boundaries: &ActorBoundaries) -> serde_json::Value {
    if boundaries.durable == boundaries.observed {
        return serde_json::json!({
            "durable": true,
            "next_event_seq": boundaries.observed.next_event_seq,
            "event_hash": boundaries.observed.event_hash,
            "output_bytes": boundaries.observed.output_bytes,
        });
    }
    return serde_json::json!({
        "durable": false,
        "durable_boundary": boundaries.durable,
        "observed_boundary": boundaries.observed,
    });
}

fn compact_control_data(data: &ControlData) -> serde_json::Value {
    return match data {
        ControlData::Acknowledged { boundaries } => serde_json::json!({
            "kind": "acknowledged",
            "boundary": compact_boundaries(boundaries),
        }),
        ControlData::InputApplied {
            operation_id,
            input_kind,
            bytes_written,
            intent_event_seq,
            result_event_seq,
            boundaries,
            ..
        } => serde_json::json!({
            "kind": "input_applied",
            "operation_id": operation_id,
            "input_kind": input_kind,
            "bytes_written": bytes_written,
            "intent_event_seq": intent_event_seq,
            "result_event_seq": result_event_seq,
            "boundary": compact_boundaries(boundaries),
        }),
        ControlData::InputSequenceApplied {
            sequence_id,
            atom_count,
            bytes_written,
            first_intent_event_seq,
            last_result_event_seq,
            pace_ns,
            boundaries,
            ..
        } => serde_json::json!({
            "kind": "input_sequence_applied",
            "sequence_id": sequence_id,
            "atom_count": atom_count,
            "bytes_written": bytes_written,
            "first_intent_event_seq": first_intent_event_seq,
            "last_result_event_seq": last_result_event_seq,
            "pace_ns": pace_ns,
            "boundary": compact_boundaries(boundaries),
        }),
        ControlData::InputNoop {
            input_kind,
            reason,
            terminal_modes_hash,
            boundaries,
        } => serde_json::json!({
            "kind": "input_noop",
            "input_kind": input_kind,
            "reason": reason,
            "terminal_modes_hash": terminal_modes_hash,
            "boundary": compact_boundaries(boundaries),
        }),
        ControlData::Activated {
            operation_id,
            text,
            start_column,
            end_column,
            row,
            target_column,
            presented_state_hash,
            bytes_written,
            intent_event_seq,
            result_event_seq,
            boundaries,
            ..
        } => serde_json::json!({
            "kind": "activated",
            "operation_id": operation_id,
            "text": text,
            "region": {
                "start_column": start_column,
                "end_column": end_column,
                "row": row,
                "target_column": target_column,
            },
            "presented_state_hash": presented_state_hash,
            "bytes_written": bytes_written,
            "intent_event_seq": intent_event_seq,
            "result_event_seq": result_event_seq,
            "boundary": compact_boundaries(boundaries),
        }),
        ControlData::Status { pid, boundaries } => serde_json::json!({
            "kind": "status",
            "pid": pid,
            "boundary": compact_boundaries(boundaries),
        }),
        ControlData::Snapshot {
            viewport_text,
            state_hash,
            presented,
            cols,
            rows,
            cursor_column,
            cursor_row,
            cursor_visible,
            observed_monotonic_ns,
            modes,
            boundaries,
            ..
        } => serde_json::json!({
            "kind": "snapshot",
            "state_hash": state_hash,
            "viewport_text": viewport_text,
            "presented": presented,
            "geometry": { "cols": cols, "rows": rows },
            "cursor": { "column": cursor_column, "row": cursor_row, "visible": cursor_visible },
            "observed_monotonic_ns": observed_monotonic_ns,
            "modes": modes,
            "boundary": compact_boundaries(boundaries),
        }),
        ControlData::StableSnapshot { .. } | ControlData::ScreenWait { .. } => {
            serde_json::to_value(data).unwrap_or_else(|_| {
                return serde_json::json!({ "kind": "control_result_encoding_failed" });
            })
        }
        ControlData::Marked {
            label,
            event_seq,
            boundaries,
        } => serde_json::json!({
            "kind": "marked",
            "label": label,
            "event_seq": event_seq,
            "boundary": compact_boundaries(boundaries),
        }),
    };
}

pub(super) fn run_control(session: PathBuf, command: ControlCommand) -> Result<ExitCode> {
    return run_control_guarded(session, command, None);
}

pub(super) fn run_control_guarded(
    session: PathBuf,
    command: ControlCommand,
    expected_presented_state_hash: Option<String>,
) -> Result<ExitCode> {
    let session = resolve_session_ref(session)?;
    let mut control_request = ControlRequest::new(command);
    control_request.expected_presented_state_hash = expected_presented_state_hash;
    let data = send_request(session, &control_request)?;
    return write_json_line(&compact_control_data(&data));
}

pub(super) fn run_status(arguments: StatusArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    let completion_path = session.join("completion.json");
    if arguments.wait_complete || completion_path.is_file() {
        let completion =
            wait_for_completion(&session, Duration::from_millis(arguments.timeout_ms))?;
        return write_json_line(&serde_json::json!({
            "kind": "completed",
            "completion": completion,
        }));
    }
    let data = request(session, ControlCommand::Status)?;
    return write_json_line(&compact_control_data(&data));
}

pub(super) fn run_stop(arguments: StopArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    if session.join("completion.json").is_file() {
        let completion = wait_for_completion(&session, Duration::ZERO)?;
        return write_json_line(&serde_json::json!({
            "kind": "completed",
            "completion": completion,
        }));
    }
    let mut control_request = ControlRequest::new(ControlCommand::Stop);
    control_request.expected_presented_state_hash = arguments.expected_presented_state_hash;
    let acknowledged = send_request(&session, &control_request)?;
    if !arguments.wait {
        return write_json_line(&compact_control_data(&acknowledged));
    }
    let completion = wait_for_completion(&session, Duration::from_millis(arguments.timeout_ms))?;
    return write_json_line(&serde_json::json!({
        "kind": "completed",
        "completion": completion,
    }));
}

pub(super) fn run_send(arguments: SendArgs) -> Result<ExitCode> {
    let mut atoms = Vec::new();
    if arguments.stdin {
        let mut data = Vec::new();
        io::stdin().read_to_end(&mut data).into_diagnostic()?;
        atoms.push(InputSequenceAtom::Bytes {
            operation_id: uuid::Uuid::now_v7(),
            data: ByteString::new(data),
            input_kind: InputKind::Raw,
            intent: arguments.intent.clone(),
        });
    } else {
        for atom in &arguments.atoms {
            if arguments.pace.is_zero() || !atom.starts_with("text:") {
                atoms.push(input_sequence_atom_for_cli(atom, arguments.intent.clone())?);
                continue;
            }
            let text = atom.strip_prefix("text:").unwrap_or_default();
            for scalar in text.chars() {
                atoms.push(InputSequenceAtom::Bytes {
                    operation_id: uuid::Uuid::now_v7(),
                    data: ByteString::new(scalar.to_string().into_bytes()),
                    input_kind: InputKind::Text,
                    intent: arguments.intent.clone(),
                });
            }
        }
    }
    if atoms.is_empty() || atoms.len() > 4096 {
        return Err(miette::miette!(
            "input sequence must contain 1 to 4096 atoms"
        ));
    }
    let session = resolve_session_ref(arguments.session)?;
    let mut request = ControlRequest::new(ControlCommand::InputSequence {
        atoms,
        pace_ns: u64::try_from(arguments.pace.as_nanos()).unwrap_or(u64::MAX),
        include_atom_outcomes: arguments.verbose,
    });
    request.expected_presented_state_hash = arguments.expected_presented_state_hash;
    let response = send_request_response(&session, &request)?;
    let data = match response.result {
        ControlResult::Ok { data } => data,
        ControlResult::Error {
            code,
            message,
            details,
        } => {
            write_json_line(&serde_json::json!({
                "kind": "input_sequence_failed",
                "sequence_id": request.request_id,
                "code": code,
                "message": message,
                "details": details,
            }))?;
            return Err(miette::miette!("input sequence failed: {code}: {message}"));
        }
    };
    let ControlData::InputSequenceApplied {
        sequence_id,
        atom_count,
        bytes_written,
        first_intent_event_seq,
        last_result_event_seq,
        pace_ns,
        atoms,
        boundaries,
    } = data
    else {
        return Err(miette::miette!(
            "recording actor returned an unexpected input-sequence payload"
        ));
    };
    let mut output = serde_json::json!({
        "kind": "input_sequence_applied",
        "sequence_id": sequence_id,
        "atom_count": atom_count,
        "bytes_written": bytes_written,
        "first_intent_event_seq": first_intent_event_seq,
        "last_result_event_seq": last_result_event_seq,
        "pace_ns": pace_ns,
        "boundary": compact_boundaries(&boundaries),
    });
    if arguments.verbose {
        output
            .as_object_mut()
            .ok_or_else(|| return miette::miette!("input-sequence output is not an object"))?
            .insert(
                "atoms".to_owned(),
                serde_json::to_value(atoms).into_diagnostic()?,
            );
    }
    return write_json_line(&output);
}

fn input_sequence_atom_for_cli(atom: &str, intent: Option<String>) -> Result<InputSequenceAtom> {
    let operation_id = uuid::Uuid::now_v7();
    if let Some((key, modifiers)) = parse_named_key(atom)? {
        return Ok(InputSequenceAtom::Key {
            operation_id,
            key,
            modifiers,
        });
    }
    if let Some(paste) = atom.strip_prefix("paste:") {
        return Ok(InputSequenceAtom::Paste {
            operation_id,
            data: ByteString::new(paste.as_bytes().to_vec()),
        });
    }
    let (data, input_kind) = if let Some(text) = atom.strip_prefix("text:") {
        (text.as_bytes().to_vec(), InputKind::Text)
    } else if let Some(raw) = atom.strip_prefix("raw:") {
        (raw.as_bytes().to_vec(), InputKind::Raw)
    } else if let Some(path) = atom.strip_prefix("raw-file:") {
        (fs_err::read(path).into_diagnostic()?, InputKind::Raw)
    } else if atom.chars().count() == 1
        && atom.chars().all(|character| return !character.is_control())
    {
        (atom.as_bytes().to_vec(), InputKind::Text)
    } else {
        return Err(miette::miette!(
            "unknown input atom; run `termtrace control keys` or use text:VALUE, paste:VALUE, raw:VALUE, or raw-file:PATH"
        ));
    };
    return Ok(InputSequenceAtom::Bytes {
        operation_id,
        data: ByteString::new(data),
        input_kind,
        intent,
    });
}

fn parse_named_key(atom: &str) -> Result<Option<(NamedKey, KeyModifiers)>> {
    let normalized = atom.to_ascii_lowercase().replace('+', "-");
    let mut remaining = normalized.as_str();
    let mut modifiers = KeyModifiers::default();
    let mut modifier_count = 0_u8;
    loop {
        let (next, selected) = if let Some(next) = remaining.strip_prefix("ctrl-") {
            if modifiers.control {
                return Err(miette::miette!(
                    "control modifier was supplied more than once"
                ));
            }
            modifiers.control = true;
            (next, true)
        } else if let Some(next) = remaining.strip_prefix("control-") {
            if modifiers.control {
                return Err(miette::miette!(
                    "control modifier was supplied more than once"
                ));
            }
            modifiers.control = true;
            (next, true)
        } else if let Some(next) = remaining.strip_prefix("shift-") {
            if modifiers.shift {
                return Err(miette::miette!(
                    "shift modifier was supplied more than once"
                ));
            }
            modifiers.shift = true;
            (next, true)
        } else if let Some(next) = remaining.strip_prefix("alt-") {
            if modifiers.alt {
                return Err(miette::miette!("alt modifier was supplied more than once"));
            }
            modifiers.alt = true;
            (next, true)
        } else if let Some(next) = remaining.strip_prefix("super-") {
            if modifiers.super_key {
                return Err(miette::miette!(
                    "super modifier was supplied more than once"
                ));
            }
            modifiers.super_key = true;
            (next, true)
        } else {
            (remaining, false)
        };
        if !selected {
            break;
        }
        modifier_count = modifier_count.saturating_add(1);
        remaining = next;
    }
    let key = match remaining {
        "enter" => NamedKey::Enter,
        "escape" | "esc" => NamedKey::Escape,
        "tab" => NamedKey::Tab,
        "backspace" => NamedKey::Backspace,
        "delete" => NamedKey::Delete,
        "insert" => NamedKey::Insert,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "page-up" => NamedKey::PageUp,
        "page-down" => NamedKey::PageDown,
        "up" => NamedKey::ArrowUp,
        "down" => NamedKey::ArrowDown,
        "left" => NamedKey::ArrowLeft,
        "right" => NamedKey::ArrowRight,
        value if value.len() >= 2 && value.starts_with('f') => {
            let suffix = value
                .strip_prefix('f')
                .ok_or_else(|| miette::miette!("function key must be f1 through f24"))?;
            let number = suffix
                .parse::<u8>()
                .map_err(|_| miette::miette!("function key must be f1 through f24"))?;
            if !(1..=24).contains(&number) {
                return Err(miette::miette!("function key must be f1 through f24"));
            }
            NamedKey::Function(number)
        }
        value if modifier_count > 0 && value.len() == 1 => {
            let letter = value
                .chars()
                .next()
                .ok_or_else(|| miette::miette!("modified letter is missing"))?;
            if !letter.is_ascii_alphabetic() {
                return Err(miette::miette!(
                    "modified printable keys currently support ASCII letters; run `termtrace control keys`"
                ));
            }
            NamedKey::Letter(letter)
        }
        _ => return Ok(None),
    };
    return Ok(Some((key, modifiers)));
}

pub(super) fn run_keys() -> Result<ExitCode> {
    return write_json_line(&serde_json::json!({
        "format": "termtrace-key-contract",
        "version": 1,
        "named_keys": [
            "enter", "escape", "esc", "tab", "backspace", "delete", "insert",
            "home", "end", "page-up", "page-down", "up", "down", "left", "right",
            "f1..f24"
        ],
        "modifiers": ["ctrl", "shift", "alt", "super"],
        "modifier_separators": ["-", "+"],
        "modified_examples": ["ctrl-f", "ctrl+f", "ctrl-down", "shift-tab", "alt-left"],
        "text": {
            "single_printable_character": "q",
            "explicit": "text:any UTF-8 text"
        },
        "paste": "paste:any UTF-8 text",
        "raw": ["raw:exact bytes from this argument", "raw-file:PATH", "send --stdin SESSION"]
    }));
}

pub(super) fn run_resize(arguments: ResizeArgs) -> Result<ExitCode> {
    let pixel_width = arguments
        .pixel_width
        .or_else(|| return arguments.cols.checked_mul(12))
        .ok_or_else(|| miette::miette!("pixel width overflow; pass --pixel-width explicitly"))?;
    let pixel_height = arguments
        .pixel_height
        .or_else(|| return arguments.rows.checked_mul(24))
        .ok_or_else(|| miette::miette!("pixel height overflow; pass --pixel-height explicitly"))?;
    return run_control_guarded(
        arguments.session,
        ControlCommand::Resize {
            cols: arguments.cols,
            rows: arguments.rows,
            pixel_width,
            pixel_height,
        },
        arguments.expected_presented_state_hash,
    );
}

pub(super) fn run_pointer(arguments: PointerArgs) -> Result<ExitCode> {
    let (column, row, operation, strict, expected_presented_state_hash, postcondition) =
        match arguments.operation {
            PointerCommand::Click(click) => (
                click.cell.0,
                click.cell.1,
                PointerOperation::Click {
                    button: click.button.into(),
                },
                click.strict,
                click.expected_presented_state_hash,
                click.postcondition,
            ),
            PointerCommand::MultiClick(click) => (
                click.cell.0,
                click.cell.1,
                PointerOperation::MultiClick {
                    button: click.button.into(),
                    count: click.count,
                },
                click.strict,
                click.expected_presented_state_hash,
                click.postcondition,
            ),
            PointerCommand::Drag(drag) => (
                drag.from.0,
                drag.from.1,
                PointerOperation::Drag {
                    button: drag.button.into(),
                    to_column: drag.to.0,
                    to_row: drag.to.1,
                    steps: drag.steps,
                },
                drag.strict,
                drag.expected_presented_state_hash,
                drag.postcondition,
            ),
            PointerCommand::Scroll(scroll) => (
                scroll.cell.0,
                scroll.cell.1,
                PointerOperation::Scroll {
                    lines: scroll.lines,
                },
                scroll.strict,
                scroll.expected_presented_state_hash,
                scroll.postcondition,
            ),
        };
    let session = resolve_session_ref(arguments.session)?;
    let mut request = ControlRequest::new(ControlCommand::Pointer {
        column,
        row,
        operation,
        strict,
    });
    request.expected_presented_state_hash = expected_presented_state_hash;
    let input = send_request(&session, &request)?;
    let predicate = match (postcondition.wait_screen, postcondition.wait_screen_absent) {
        (Some(text), None) => Some(ScreenPredicate::Contains { text }),
        (None, Some(text)) => Some(ScreenPredicate::Absent { text }),
        (None, None) => None,
        (Some(_), Some(_)) => {
            return Err(miette::miette!(
                "pointer postcondition requires exactly one screen predicate"
            ));
        }
    };
    let Some(predicate) = predicate else {
        return write_json_line(&input);
    };
    let matched = wait_for_screen(
        &session,
        predicate,
        postcondition.stable_for,
        Duration::from_millis(postcondition.timeout_ms),
    )?;
    return write_json_line(&serde_json::json!({
        "kind": "pointer_with_postcondition",
        "input": input,
        "postcondition": matched,
    }));
}
