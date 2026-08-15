use super::*;

pub(in crate::cli) fn run_send(
    response: &mut ResponseWriter,
    arguments: SendArgs,
) -> Result<ExitCode> {
    let request_id = arguments.request_id.unwrap_or_else(uuid::Uuid::now_v7);
    let mut atoms: Vec<InputSequenceAtom> = Vec::new();
    if arguments.stdin {
        let mut data: Vec<u8> = Vec::new();
        io::stdin().read_to_end(&mut data).into_diagnostic()?;
        atoms.push(InputSequenceAtom::Bytes {
            operation_id: sequence_operation_id(request_id, 0),
            data: ByteString::new(data),
            input_kind: InputKind::Raw,
            caller_intent: arguments.intent.clone(),
        });
    } else {
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
    }
    if atoms.is_empty() || atoms.len() > 4096 {
        return Err(miette::miette!(
            "input sequence must contain 1 to 4096 atoms"
        ));
    }
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(response, &resolved, "send", ExitCode::FAILURE);
    }
    let mut request = ControlRequest::new(ControlCommand::InputSequence {
        atoms,
        pace_ns: u64::try_from(arguments.pace.as_nanos()).unwrap_or(u64::MAX),
        include_atom_outcomes: arguments.verbose,
    });
    request.request_id = request_id;
    request.expected_presented_state_hash = arguments.expected_presented_state_hash;
    let control_response = match send_request_response(&resolved.path, &request) {
        Ok(control_response) => control_response,
        Err(error) => {
            write_json_line(
                response,
                &serde_json::json!({
                    "kind": "input_sequence_failed",
                    "sequence_id": request.request_id,
                    "code": "transport_lost",
                    "message": error.to_string(),
                    "applied": "unknown",
                    "retry": "same_request_id_only",
                    "session_state": "unknown",
                }),
            )?;
            return Ok(ExitCode::FAILURE);
        }
    };
    let data = match control_response.result {
        ControlResult::Ok { data } => data,
        ControlResult::Error {
            code,
            message,
            details,
        } => {
            let applied = input_failure_application(details.as_ref());
            write_json_line(
                response,
                &serde_json::json!({
                    "kind": "input_sequence_failed",
                    "sequence_id": request.request_id,
                    "code": code,
                    "message": message,
                    "details": details,
                    "applied": applied,
                    "retry": input_retry(applied),
                    "session_state": "live",
                }),
            )?;
            return Ok(ExitCode::FAILURE);
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
    return write_json_line(response, &output);
}

pub(super) fn input_sequence_atom_for_cli(
    atom: &str,
    operation_id: uuid::Uuid,
    caller_intent: Option<String>,
) -> Result<InputSequenceAtom> {
    if let Some((key, modifiers)) = parse_named_key(atom)? {
        return Ok(InputSequenceAtom::Key {
            operation_id,
            key,
            modifiers,
            caller_intent,
        });
    }
    if let Some(paste) = atom.strip_prefix("paste:") {
        return Ok(InputSequenceAtom::Paste {
            operation_id,
            data: ByteString::new(paste.as_bytes().to_vec()),
            caller_intent,
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
        caller_intent,
    });
}

pub(super) fn sequence_operation_id(sequence_id: uuid::Uuid, index: usize) -> uuid::Uuid {
    let atom_number = u128::try_from(index).unwrap_or(u128::MAX).saturating_add(1);
    return uuid::Uuid::from_u128(sequence_id.as_u128() ^ atom_number);
}

pub(super) fn parse_named_key(atom: &str) -> Result<Option<(NamedKey, KeyModifiers)>> {
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

pub(in crate::cli) fn run_keys(response: &mut ResponseWriter) -> Result<ExitCode> {
    return write_json_line(
        response,
        &serde_json::json!({
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
        }),
    );
}

pub(in crate::cli) fn run_resize(
    response: &mut ResponseWriter,
    arguments: ResizeArgs,
) -> Result<ExitCode> {
    let pixel_width = arguments
        .pixel_width
        .or_else(|| return arguments.cols.checked_mul(12))
        .ok_or_else(|| miette::miette!("pixel width overflow; pass --pixel-width explicitly"))?;
    let pixel_height = arguments
        .pixel_height
        .or_else(|| return arguments.rows.checked_mul(24))
        .ok_or_else(|| miette::miette!("pixel height overflow; pass --pixel-height explicitly"))?;
    return run_control_guarded(
        response,
        arguments.session,
        ControlCommand::Resize {
            cols: arguments.cols,
            rows: arguments.rows,
            pixel_width,
            pixel_height,
        },
        arguments.expected_presented_state_hash,
        arguments.request_id,
    );
}

pub(in crate::cli) fn run_pointer(
    response: &mut ResponseWriter,
    arguments: PointerArgs,
) -> Result<ExitCode> {
    let (column, row, operation, strict, expected_presented_state_hash, request_id, postcondition) =
        match arguments.operation {
            PointerCommand::Click(click) => (
                click.cell.0,
                click.cell.1,
                PointerOperation::Click {
                    button: click.button.into(),
                },
                click.strict,
                click.expected_presented_state_hash,
                click.request_id,
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
                click.request_id,
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
                drag.request_id,
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
                scroll.request_id,
                scroll.postcondition,
            ),
        };
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(response, &resolved, "pointer", ExitCode::FAILURE);
    }
    let mut request = ControlRequest::new(ControlCommand::Pointer {
        column,
        row,
        operation,
        strict,
    });
    request.expected_presented_state_hash = expected_presented_state_hash;
    if let Some(request_id) = request_id {
        request.request_id = request_id;
    }
    let input = match send_cli_control(response, &resolved.path, &request, "pointer")? {
        CliControlOutcome::Applied(data) => *data,
        CliControlOutcome::Rejected(exit_code) => return Ok(exit_code),
    };
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
        return write_json_line(response, &input);
    };
    let matched = wait_for_screen(
        &resolved.path,
        predicate,
        postcondition.stable_for,
        Duration::from_millis(postcondition.timeout_ms),
    )
    .into_diagnostic()?;
    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "pointer_with_postcondition",
            "input": input,
            "postcondition": matched,
        }),
    );
}
