//! Terminal input encoding, validation, and ordered event application.

use std::{
    borrow::Cow,
    collections::HashSet,
    io::{Error as IoError, ErrorKind, Write},
    mem::take,
    time::{Duration, Instant},
};

use portable_pty::MasterPty;
use uuid::Uuid;

use termtrace_evidence::event_log::EventLogWriter;
use termtrace_model::{
    execution::event::{Event, InputDescriptor, InputKind, InputSource, OperationStatus},
    protocol::control::{ActionInputResult, InputSequenceAtom, InputSequenceAtomOutcome},
    terminal::state::{KeyModifiers, ModeState, MouseButton, NamedKey},
    value::ByteString,
};
use termtrace_terminal::GhosttyVt;

use super::{
    model::TerminalGeometry,
    support::{control_rejected, finish_with_cleanup, terminal_error, validate_control_input},
};
use crate::{Error, Result};

pub(super) struct InputApplication {
    pub(super) bytes_written: u64,
    pub(super) intent_event_seq: u64,
    pub(super) result_event_seq: u64,
}

pub(super) struct PreparedInput<'data> {
    pub(super) input_kind: InputKind,
    pub(super) descriptor: InputDescriptor,
    pub(super) semantic_input: Option<String>,
    pub(super) terminal_modes_hash: String,
    pub(super) data: Cow<'data, [u8]>,
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

pub(super) fn semantic_key_input(key: NamedKey, modifiers: KeyModifiers) -> String {
    let mut parts: Vec<&str> = Vec::with_capacity(5);
    if modifiers.control {
        parts.push("ctrl");
    }
    if modifiers.shift {
        parts.push("shift");
    }
    if modifiers.alt {
        parts.push("alt");
    }
    if modifiers.super_key {
        parts.push("super");
    }
    let key = semantic_key_name(key);
    parts.push(&key);
    return parts.join("+");
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
    terminal_modes_hash: Option<String>,
    semantic_input: Option<String>,
    caller_intent: Option<String>,
    data: &[u8],
) -> Result<InputApplication> {
    let intent_event_seq = event_log.append(Event::InputIntent {
        operation_id,
        source,
        input_kind,
        descriptor,
        data: ByteString::from(data),
        terminal_modes_hash,
        semantic_input,
        caller_intent,
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
                Err(Error::from(cleanup)),
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

pub(super) struct InputSequenceExecution {
    sequence_id: Uuid,
    atoms: Vec<InputSequenceAtom>,
    pace_ns: u64,
    include_atom_outcomes: bool,
    next_atom_index: usize,
    next_deadline: Instant,
    outcomes: Vec<InputSequenceAtomOutcome>,
    total_bytes_written: u64,
    first_intent_event_seq: Option<u64>,
    last_result_event_seq: Option<u64>,
}

impl InputSequenceExecution {
    pub(super) fn new(
        sequence_id: Uuid,
        atoms: &[InputSequenceAtom],
        pace_ns: u64,
        include_atom_outcomes: bool,
    ) -> Result<Self> {
        validate_input_sequence(atoms, pace_ns)?;
        return Ok(Self {
            sequence_id,
            atoms: atoms.to_vec(),
            pace_ns,
            include_atom_outcomes,
            next_atom_index: 0,
            next_deadline: Instant::now(),
            outcomes: Vec::with_capacity(if include_atom_outcomes {
                atoms.len()
            } else {
                0
            }),
            total_bytes_written: 0,
            first_intent_event_seq: None,
            last_result_event_seq: None,
        });
    }

    pub(super) fn next_deadline(&self) -> Instant {
        return self.next_deadline;
    }

    pub(super) fn advance(
        &mut self,
        writer: &mut dyn Write,
        event_log: &mut EventLogWriter,
        semantic: &mut GhosttyVt,
    ) -> Result<Option<ActionInputResult>> {
        let source_atom = self.atoms.get(self.next_atom_index).ok_or_else(|| {
            return terminal_error(
                "advance input sequence",
                "next atom index is outside the validated sequence",
            );
        })?;
        let atom = match prepare_input_sequence_atom(source_atom, semantic) {
            Ok(atom) => atom,
            Err(error) if self.next_atom_index > 0 => {
                return Err(Error::InputSequenceIncomplete {
                    sequence_id: self.sequence_id,
                    failed_atom_index: u32::try_from(self.next_atom_index).unwrap_or(u32::MAX),
                    failed_operation_id: input_sequence_operation_id(source_atom),
                    atoms_applied: u32::try_from(self.next_atom_index).unwrap_or(u32::MAX),
                    bytes_written: self.total_bytes_written,
                    failed_atom_bytes_written: 0,
                    first_intent_event_seq: self
                        .first_intent_event_seq
                        .unwrap_or_else(|| return event_log.next_seq()),
                    last_result_event_seq: self
                        .last_result_event_seq
                        .unwrap_or_else(|| return event_log.next_seq()),
                    reason: format!("atom encoding failed before PTY delivery: {error}"),
                });
            }
            Err(error) => return Err(error),
        };
        let applied = match apply_input(
            writer,
            event_log,
            input_sequence_operation_id(source_atom),
            InputSource::Agent,
            atom.input_kind,
            atom.descriptor,
            Some(atom.terminal_modes_hash),
            atom.semantic_input,
            match source_atom {
                InputSequenceAtom::Bytes { caller_intent, .. }
                | InputSequenceAtom::Key { caller_intent, .. }
                | InputSequenceAtom::Paste { caller_intent, .. } => caller_intent.clone(),
            },
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
                return Err(Error::InputSequenceIncomplete {
                    sequence_id: self.sequence_id,
                    failed_atom_index: u32::try_from(self.next_atom_index).unwrap_or(u32::MAX),
                    failed_operation_id: operation_id,
                    atoms_applied: u32::try_from(self.next_atom_index).unwrap_or(u32::MAX),
                    bytes_written: self.total_bytes_written.saturating_add(bytes_written),
                    failed_atom_bytes_written: bytes_written,
                    first_intent_event_seq: self.first_intent_event_seq.unwrap_or(intent_event_seq),
                    last_result_event_seq: result_event_seq,
                    reason,
                });
            }
            Err(error) => return Err(error),
        };
        self.first_intent_event_seq
            .get_or_insert(applied.intent_event_seq);
        self.last_result_event_seq = Some(applied.result_event_seq);
        self.total_bytes_written = self
            .total_bytes_written
            .saturating_add(applied.bytes_written);
        if self.include_atom_outcomes {
            self.outcomes.push(InputSequenceAtomOutcome {
                operation_id: input_sequence_operation_id(source_atom),
                input_kind: atom.input_kind,
                bytes_written: applied.bytes_written,
                intent_event_seq: applied.intent_event_seq,
                result_event_seq: applied.result_event_seq,
            });
        }
        self.next_atom_index = self.next_atom_index.saturating_add(1);
        if self.next_atom_index < self.atoms.len() {
            self.next_deadline = Instant::now() + Duration::from_nanos(self.pace_ns);
            return Ok(None);
        }
        return Ok(Some(ActionInputResult {
            sequence_id: self.sequence_id,
            atom_count: u32::try_from(self.atoms.len()).unwrap_or(u32::MAX),
            bytes_written: self.total_bytes_written,
            first_intent_event_seq: self
                .first_intent_event_seq
                .unwrap_or_else(|| return event_log.next_seq()),
            last_result_event_seq: self
                .last_result_event_seq
                .unwrap_or_else(|| return event_log.next_seq()),
            pace_ns: self.pace_ns,
            atoms: take(&mut self.outcomes),
        }));
    }
}

fn validate_input_sequence(atoms: &[InputSequenceAtom], pace_ns: u64) -> Result<()> {
    if atoms.is_empty() || atoms.len() > 4096 {
        return Err(control_rejected(
            "invalid_input_sequence",
            "input sequence must contain 1 through 4096 atoms",
        ));
    }
    let atom_count = u64::try_from(atoms.len()).unwrap_or(u64::MAX);
    let total_pace_ns = pace_ns.checked_mul(atom_count).ok_or_else(|| {
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
    let mut operation_ids = HashSet::with_capacity(atoms.len());
    for atom in atoms {
        if !operation_ids.insert(input_sequence_operation_id(atom)) {
            return Err(control_rejected(
                "invalid_input_sequence",
                "input sequence operation identifiers must be unique",
            ));
        }
        match atom {
            InputSequenceAtom::Bytes {
                data,
                caller_intent,
                ..
            } => validate_control_input(data.as_bytes(), caller_intent.as_deref())?,
            InputSequenceAtom::Paste {
                data,
                caller_intent,
                ..
            } => {
                validate_control_input(data.as_bytes(), caller_intent.as_deref())?;
                encode_paste(data.as_bytes(), false)?;
            }
            InputSequenceAtom::Key { .. } => {}
        }
    }
    return Ok(());
}

fn prepare_input_sequence_atom<'data>(
    atom: &'data InputSequenceAtom,
    semantic: &mut GhosttyVt,
) -> Result<PreparedInput<'data>> {
    return match atom {
        InputSequenceAtom::Bytes {
            data, input_kind, ..
        } => prepare_bytes(data.as_bytes(), *input_kind, semantic),
        InputSequenceAtom::Key { key, modifiers, .. } => {
            prepare_key(*key, *modifiers, semantic, |error| {
                return control_rejected("key_encoding_failed", &error.to_string());
            })
        }
        InputSequenceAtom::Paste { data, .. } => prepare_paste(data.as_bytes(), semantic),
    };
}

pub(super) fn prepare_bytes<'data>(
    data: &'data [u8],
    input_kind: InputKind,
    semantic: &mut GhosttyVt,
) -> Result<PreparedInput<'data>> {
    let modes = semantic.snapshot()?.modes;
    return Ok(PreparedInput {
        input_kind,
        descriptor: match input_kind {
            InputKind::Text => InputDescriptor::Text,
            InputKind::Raw
            | InputKind::Keys
            | InputKind::Paste
            | InputKind::Focus
            | InputKind::Pointer => InputDescriptor::Raw,
        },
        terminal_modes_hash: mode_hash(&modes, "serialize input modes")?,
        semantic_input: Some(
            match input_kind {
                InputKind::Text => "text",
                InputKind::Raw => "raw",
                InputKind::Keys => "keys",
                InputKind::Paste => "paste",
                InputKind::Focus => "focus",
                InputKind::Pointer => "pointer",
            }
            .to_owned(),
        ),
        data: Cow::Borrowed(data),
    });
}

pub(super) fn prepare_key(
    key: NamedKey,
    modifiers: KeyModifiers,
    semantic: &mut GhosttyVt,
    encoding_error: fn(termtrace_terminal::Error) -> Error,
) -> Result<PreparedInput<'static>> {
    let modes = semantic.snapshot()?.modes;
    let terminal_modes_hash = mode_hash(&modes, "serialize keyboard modes")?;
    let encoded = semantic
        .encode_key(key, modifiers)
        .map_err(encoding_error)?;
    if encoded.data.as_bytes().is_empty() {
        return Err(control_rejected(
            "key_has_no_output",
            "the requested key press produced no terminal bytes",
        ));
    }
    return Ok(PreparedInput {
        input_kind: InputKind::Keys,
        descriptor: InputDescriptor::Key {
            key: semantic_key_name(key),
            shift: modifiers.shift,
            control: modifiers.control,
            alt: modifiers.alt,
            super_key: modifiers.super_key,
            encoder: encoded.encoder,
            terminal_modes_hash: terminal_modes_hash.clone(),
        },
        terminal_modes_hash,
        semantic_input: Some(semantic_key_input(key, modifiers)),
        data: Cow::Owned(encoded.data.into_bytes()),
    });
}

pub(super) fn prepare_paste(
    data: &[u8],
    semantic: &mut GhosttyVt,
) -> Result<PreparedInput<'static>> {
    let modes = semantic.snapshot()?.modes;
    let terminal_modes_hash = mode_hash(&modes, "serialize paste modes")?;
    return Ok(PreparedInput {
        input_kind: InputKind::Paste,
        descriptor: InputDescriptor::Paste {
            bracketed: modes.bracketed_paste,
            terminal_modes_hash: terminal_modes_hash.clone(),
        },
        terminal_modes_hash,
        semantic_input: Some("paste".to_owned()),
        data: Cow::Owned(encode_paste(data, modes.bracketed_paste)?),
    });
}

pub(super) fn mode_hash(modes: &ModeState, operation: &'static str) -> Result<String> {
    let mode_bytes = serde_json::to_vec(modes).map_err(|error| {
        return terminal_error(operation, error.to_string());
    })?;
    return Ok(blake3::hash(&mode_bytes).to_hex().to_string());
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
    let result_write: Result<()> = event_log
        .append(Event::ResizeResult {
            operation_id,
            status,
            error_code: resize_result
                .as_ref()
                .err()
                .map(|_| return "pty_resize_failed".to_owned()),
        })
        .map(|_| ())
        .map_err(Error::from);
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
