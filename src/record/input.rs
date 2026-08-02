//! Terminal input encoding, validation, and ordered event application.

use std::{
    collections::HashSet,
    io::{Error as IoError, ErrorKind, Write},
    thread,
    time::Duration,
};

use portable_pty::MasterPty;
use uuid::Uuid;

use crate::{
    Error, Result,
    control::{ControlData, InputSequenceAtom, InputSequenceAtomOutcome},
    event::{ByteString, Event, InputDescriptor, InputKind, InputSource, OperationStatus},
    event_log::EventLogWriter,
    ghostty_vt::{GhosttyVt, MouseButton, NamedKey},
};

use super::{
    model::TerminalGeometry,
    support::{
        actor_boundaries, control_rejected, finish_with_cleanup, terminal_error,
        validate_control_input,
    },
};

pub(super) struct InputApplication {
    pub(super) bytes_written: u64,
    pub(super) intent_event_seq: u64,
    pub(super) result_event_seq: u64,
}

struct PreparedSequenceAtom {
    operation_id: Uuid,
    input_kind: InputKind,
    descriptor: InputDescriptor,
    intent: Option<String>,
    data: Vec<u8>,
}
pub(super) fn semantic_key_name(key: NamedKey) -> String {
    return match key {
        NamedKey::Enter => "enter".to_owned(),
        NamedKey::Escape => "escape".to_owned(),
        NamedKey::Tab => "tab".to_owned(),
        NamedKey::Backspace => "backspace".to_owned(),
        NamedKey::Delete => "delete".to_owned(),
        NamedKey::Insert => "insert".to_owned(),
        NamedKey::Home => "home".to_owned(),
        NamedKey::End => "end".to_owned(),
        NamedKey::PageUp => "page-up".to_owned(),
        NamedKey::PageDown => "page-down".to_owned(),
        NamedKey::ArrowUp => "up".to_owned(),
        NamedKey::ArrowDown => "down".to_owned(),
        NamedKey::ArrowLeft => "left".to_owned(),
        NamedKey::ArrowRight => "right".to_owned(),
        NamedKey::Function(number) => format!("f{number}"),
        NamedKey::Letter(letter) => letter.to_ascii_lowercase().to_string(),
    };
}

pub(super) fn mouse_button_name(button: MouseButton) -> &'static str {
    return match button {
        MouseButton::Left => "left",
        MouseButton::Middle => "middle",
        MouseButton::Right => "right",
        MouseButton::WheelUp => "wheel-up",
        MouseButton::WheelDown => "wheel-down",
    };
}

pub(super) fn interpolate_cell(start: u16, end: u16, step: u32, steps: u32) -> u16 {
    let start = i64::from(start);
    let delta = i64::from(end) - start;
    let rounded = start + (delta * i64::from(step) + i64::from(steps) / 2) / i64::from(steps);
    return u16::try_from(rounded).unwrap_or(end);
}

pub(super) fn encode_paste(data: &[u8], bracketed: bool) -> Result<Vec<u8>> {
    if data.windows(6).any(|window| return window == b"\x1b[201~")
        || data.iter().any(|byte| {
            return (*byte < 0x20 && !matches!(*byte, b'\t' | b'\n' | b'\r'))
                || *byte == 0x7f
                || *byte == 0x1b;
        })
    {
        return Err(control_rejected(
            "unsafe_paste_content",
            "paste contains an escape, delimiter, or unsupported control byte",
        ));
    }
    if bracketed {
        let mut encoded = Vec::with_capacity(data.len().saturating_add(12));
        encoded.extend_from_slice(b"\x1b[200~");
        encoded.extend_from_slice(data);
        encoded.extend_from_slice(b"\x1b[201~");
        return Ok(encoded);
    }
    return Ok(data
        .iter()
        .map(|byte| return if *byte == b'\n' { b'\r' } else { *byte })
        .collect());
}

pub(super) fn apply_input(
    writer: &mut dyn Write,
    event_log: &mut EventLogWriter,
    operation_id: Uuid,
    source: InputSource,
    input_kind: InputKind,
    descriptor: InputDescriptor,
    intent: Option<String>,
    data: &[u8],
) -> Result<InputApplication> {
    let intent_event_seq = event_log.append(Event::InputIntent {
        operation_id,
        source,
        input_kind,
        descriptor,
        data: ByteString::from(data),
        intent,
    })?;
    let (bytes_written, write_error) = write_input(writer, data);
    let status = if write_error.is_some() {
        OperationStatus::Failed
    } else {
        OperationStatus::Applied
    };
    let error_code = write_error.as_ref().map(io_error_code);
    let result_write = event_log.append(Event::InputResult {
        operation_id,
        status,
        bytes_written,
        error_code,
    });
    let Some(error) = write_error else {
        let result_event_seq = result_write?;
        return Ok(InputApplication {
            bytes_written,
            intent_event_seq,
            result_event_seq,
        });
    };
    let result_event_seq = match result_write {
        Ok(result_event_seq) => result_event_seq,
        Err(cleanup) => {
            return finish_with_cleanup(
                Err(terminal_error("write PTY input", error.to_string())),
                Err(cleanup),
            );
        }
    };
    return Err(Error::InputWriteFailed {
        operation_id,
        bytes_written,
        intent_event_seq,
        result_event_seq,
        reason: error.to_string(),
    });
}

pub(super) fn apply_input_sequence(
    writer: &mut dyn Write,
    event_log: &mut EventLogWriter,
    sequence_id: Uuid,
    atoms: &[InputSequenceAtom],
    pace_ns: u64,
    include_atom_outcomes: bool,
    semantic: &mut GhosttyVt,
) -> Result<ControlData> {
    if atoms.is_empty() || atoms.len() > 4096 {
        return Err(control_rejected(
            "invalid_input_sequence",
            "input sequence must contain 1 through 4096 atoms",
        ));
    }
    let gap_count = u64::try_from(atoms.len().saturating_sub(1)).unwrap_or(u64::MAX);
    let total_pace_ns = pace_ns.checked_mul(gap_count).ok_or_else(|| {
        return control_rejected(
            "invalid_input_sequence",
            "input sequence pacing duration overflowed",
        );
    })?;
    if total_pace_ns > 60_000_000_000 {
        return Err(control_rejected(
            "invalid_input_sequence",
            "input sequence pacing may not exceed 60 seconds in total",
        ));
    }
    let prepared = prepare_input_sequence(atoms, semantic)?;
    let mut outcomes = Vec::with_capacity(if include_atom_outcomes {
        prepared.len()
    } else {
        0
    });
    let mut total_bytes_written = 0_u64;
    let mut first_intent_event_seq = None;
    let mut last_result_event_seq = None;
    for (index, atom) in prepared.into_iter().enumerate() {
        if index != 0 && pace_ns != 0 {
            thread::sleep(Duration::from_nanos(pace_ns));
        }
        let applied = match apply_input(
            writer,
            event_log,
            atom.operation_id,
            InputSource::Agent,
            atom.input_kind,
            atom.descriptor,
            atom.intent,
            &atom.data,
        ) {
            Ok(applied) => applied,
            Err(Error::InputWriteFailed {
                operation_id,
                bytes_written,
                intent_event_seq,
                result_event_seq,
                reason,
            }) => {
                let first_intent_event_seq = first_intent_event_seq.unwrap_or(intent_event_seq);
                return Err(Error::InputSequenceIncomplete {
                    sequence_id,
                    failed_atom_index: u32::try_from(index).unwrap_or(u32::MAX),
                    failed_operation_id: operation_id,
                    atoms_applied: u32::try_from(index).unwrap_or(u32::MAX),
                    bytes_written: total_bytes_written.saturating_add(bytes_written),
                    failed_atom_bytes_written: bytes_written,
                    first_intent_event_seq,
                    last_result_event_seq: result_event_seq,
                    reason,
                });
            }
            Err(error) => return Err(error),
        };
        first_intent_event_seq.get_or_insert(applied.intent_event_seq);
        last_result_event_seq = Some(applied.result_event_seq);
        total_bytes_written = total_bytes_written.saturating_add(applied.bytes_written);
        if include_atom_outcomes {
            outcomes.push(InputSequenceAtomOutcome {
                operation_id: atom.operation_id,
                input_kind: atom.input_kind,
                bytes_written: applied.bytes_written,
                intent_event_seq: applied.intent_event_seq,
                result_event_seq: applied.result_event_seq,
            });
        }
    }
    return Ok(ControlData::InputSequenceApplied {
        sequence_id,
        atom_count: u32::try_from(atoms.len()).unwrap_or(u32::MAX),
        bytes_written: total_bytes_written,
        first_intent_event_seq: first_intent_event_seq
            .unwrap_or_else(|| return event_log.next_seq()),
        last_result_event_seq: last_result_event_seq.unwrap_or_else(|| return event_log.next_seq()),
        pace_ns,
        atoms: outcomes,
        boundaries: actor_boundaries(event_log),
    });
}

fn prepare_input_sequence(
    atoms: &[InputSequenceAtom],
    semantic: &mut GhosttyVt,
) -> Result<Vec<PreparedSequenceAtom>> {
    let mut operation_ids = HashSet::with_capacity(atoms.len());
    let mut prepared = Vec::with_capacity(atoms.len());
    for atom in atoms {
        let operation_id = input_sequence_operation_id(atom);
        if !operation_ids.insert(operation_id) {
            return Err(control_rejected(
                "invalid_input_sequence",
                "input sequence operation identifiers must be unique",
            ));
        }
        let value = match atom {
            InputSequenceAtom::Bytes {
                operation_id,
                data,
                input_kind,
                intent,
            } => {
                validate_control_input(data.as_bytes(), intent.as_deref())?;
                PreparedSequenceAtom {
                    operation_id: *operation_id,
                    input_kind: *input_kind,
                    descriptor: match input_kind {
                        InputKind::Text => InputDescriptor::Text,
                        InputKind::Raw
                        | InputKind::Keys
                        | InputKind::Paste
                        | InputKind::Focus
                        | InputKind::Pointer => InputDescriptor::Raw,
                    },
                    intent: intent.clone(),
                    data: data.as_bytes().to_vec(),
                }
            }
            InputSequenceAtom::Key {
                operation_id,
                key,
                modifiers,
            } => {
                let modes = semantic.snapshot()?.modes;
                let mode_bytes = serde_json::to_vec(&modes).map_err(|error| {
                    return terminal_error("serialize keyboard modes", error.to_string());
                })?;
                let encoded = semantic.encode_key(*key, *modifiers)?;
                if encoded.data.as_bytes().is_empty() {
                    return Err(control_rejected(
                        "key_has_no_output",
                        "the requested key press produced no terminal bytes",
                    ));
                }
                PreparedSequenceAtom {
                    operation_id: *operation_id,
                    input_kind: InputKind::Keys,
                    descriptor: InputDescriptor::Key {
                        key: semantic_key_name(*key),
                        shift: modifiers.shift,
                        control: modifiers.control,
                        alt: modifiers.alt,
                        super_key: modifiers.super_key,
                        encoder: encoded.encoder,
                        terminal_modes_hash: blake3::hash(&mode_bytes).to_hex().to_string(),
                    },
                    intent: Some(semantic_key_name(*key)),
                    data: encoded.data.as_bytes().to_vec(),
                }
            }
            InputSequenceAtom::Paste { operation_id, data } => {
                validate_control_input(data.as_bytes(), Some("paste"))?;
                let modes = semantic.snapshot()?.modes;
                let mode_bytes = serde_json::to_vec(&modes).map_err(|error| {
                    return terminal_error("serialize paste modes", error.to_string());
                })?;
                PreparedSequenceAtom {
                    operation_id: *operation_id,
                    input_kind: InputKind::Paste,
                    descriptor: InputDescriptor::Paste {
                        bracketed: modes.bracketed_paste,
                        terminal_modes_hash: blake3::hash(&mode_bytes).to_hex().to_string(),
                    },
                    intent: Some("paste".to_owned()),
                    data: encode_paste(data.as_bytes(), modes.bracketed_paste)?,
                }
            }
        };
        prepared.push(value);
    }
    return Ok(prepared);
}

pub(super) fn input_sequence_operation_id(atom: &InputSequenceAtom) -> Uuid {
    return match atom {
        InputSequenceAtom::Bytes { operation_id, .. }
        | InputSequenceAtom::Key { operation_id, .. }
        | InputSequenceAtom::Paste { operation_id, .. } => *operation_id,
    };
}

pub(super) fn apply_resize(
    master: &mut dyn MasterPty,
    event_log: &mut EventLogWriter,
    operation_id: Uuid,
    geometry: TerminalGeometry,
) -> Result<()> {
    event_log.append(Event::ResizeIntent {
        operation_id,
        cols: geometry.cols,
        rows: geometry.rows,
        pixel_width: geometry.pixel_width,
        pixel_height: geometry.pixel_height,
    })?;
    let resize_result = master.resize(geometry.into());
    let status = if resize_result.is_ok() {
        OperationStatus::Applied
    } else {
        OperationStatus::Failed
    };
    let result_write = event_log
        .append(Event::ResizeResult {
            operation_id,
            status,
            error_code: resize_result
                .as_ref()
                .err()
                .map(|_| return "pty_resize_failed".to_owned()),
        })
        .map(|_| ());
    let Some(error) = resize_result.err() else {
        return result_write;
    };
    return finish_with_cleanup(
        Err(terminal_error("resize child PTY", error.to_string())),
        result_write,
    );
}

fn write_input(writer: &mut dyn Write, data: &[u8]) -> (u64, Option<IoError>) {
    let mut offset = 0_usize;
    while offset < data.len() {
        let Some(remaining) = data.get(offset..) else {
            return (
                u64::try_from(offset).unwrap_or(u64::MAX),
                Some(IoError::other("PTY writer reported an invalid byte count")),
            );
        };
        match writer.write(remaining) {
            Ok(0) => {
                return (
                    u64::try_from(offset).unwrap_or(u64::MAX),
                    Some(IoError::new(
                        ErrorKind::WriteZero,
                        "PTY accepted zero input bytes",
                    )),
                );
            }
            Ok(written) => offset = offset.saturating_add(written),
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return (u64::try_from(offset).unwrap_or(u64::MAX), Some(error)),
        }
    }
    return (u64::try_from(offset).unwrap_or(u64::MAX), None);
}

fn io_error_code(error: &IoError) -> String {
    return error.raw_os_error().map_or_else(
        || return format!("io_{:?}", error.kind()).to_ascii_lowercase(),
        |errno| format!("os_errno_{errno}"),
    );
}
