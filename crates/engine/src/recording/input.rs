//! Terminal input encoding, validation, and ordered event application.

use super::{
    model::TerminalGeometry,
    support::{control_rejected, finish_with_cleanup, terminal_error, validate_control_input},
};
use crate::{Error, Result};
use portable_pty::MasterPty;
use std::{
    borrow::Cow,
    collections::HashSet,
    io::{Error as IoError, ErrorKind, Write},
    mem::take,
    time::{Duration, Instant},
};
use termtrace_evidence::event_log::EventLogWriter;
use termtrace_model::{
    execution::event::{Event, InputDescriptor, InputKind, InputSource, OperationStatus},
    protocol::{
        control::{ActionInputResult, InputSequenceAtom, InputSequenceAtomOutcome},
        mutation::MutationEffect,
    },
    terminal::state::{KeyModifiers, ModeState, MouseButton, NamedKey},
    value::ByteString,
};
use termtrace_terminal::GhosttyVt;
use uuid::Uuid;

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

pub(super) fn semantic_key(key: NamedKey, modifiers: KeyModifiers) -> String {
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
    if steps == 0 || step >= steps {
        return end;
    }

    let start = u64::from(start);
    let end_val = u64::from(end);
    let step = u64::from(step);
    let steps = u64::from(steps);
    let interpolated = (start * (steps - step) + end_val * step + steps / 2) / steps;

    return u16::try_from(interpolated).unwrap_or(end);
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

pub(super) fn apply(
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
    let intent = Event::InputIntent {
        operation_id,
        source,
        input_kind,
        descriptor,
        data: ByteString::from(data),
        terminal_modes_hash,
        semantic_input,
        caller_intent,
    };

    return apply_intent(writer, event_log, operation_id, intent, data, false);
}

/// Answer a terminal query, leaving a successful result for the next synchronization.
pub(super) fn apply_query_response(
    writer: &mut dyn Write,
    event_log: &mut EventLogWriter,
    operation_id: Uuid,
    data: &[u8],
) -> Result<InputApplication> {
    let intent = Event::InputIntent {
        operation_id,
        source: InputSource::Host,
        input_kind: InputKind::Raw,
        descriptor: InputDescriptor::Raw,
        data: ByteString::from(data),
        terminal_modes_hash: None,
        semantic_input: Some("terminal-query-response".to_owned()),
        caller_intent: None,
    };

    return apply_intent(writer, event_log, operation_id, intent, data, true);
}

fn apply_intent(
    writer: &mut dyn Write,
    event_log: &mut EventLogWriter,
    operation_id: Uuid,
    intent: Event,
    data: &[u8],
    defer_success: bool,
) -> Result<InputApplication> {
    let intent_event_seq = event_log.append(intent)?;
    let (bytes_written, write_error) = write_input(writer, data);
    let status = if write_error.is_some() {
        OperationStatus::Failed
    } else {
        OperationStatus::Applied
    };

    let error_code = write_error.as_ref().map(io_error_code);
    let result = Event::InputResult {
        operation_id,
        status,
        bytes_written,
        error_code,
    };
    let result_write = if defer_success && write_error.is_none() {
        event_log.append_deferred(result)
    } else {
        event_log.append(result)
    };

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
    sleeping: bool,
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
            outcomes: Vec::with_capacity(if include_atom_outcomes { atoms.len() } else { 0 }),
            total_bytes_written: 0,
            first_intent_event_seq: None,
            last_result_event_seq: None,
            sleeping: false,
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

        if let InputSequenceAtom::Sleep { duration_ns, .. } = source_atom
            && !self.sleeping
        {
            self.sleeping = true;
            self.next_deadline = Instant::now() + Duration::from_nanos(*duration_ns);

            return Ok(None);
        }

        self.sleeping = false;

        let atom = match prepare_input_sequence_atom(source_atom, semantic) {
            Ok(atom) => atom,
            Err(error) if self.next_atom_index > 0 => {
                return Err(Error::InputSequenceIncomplete {
                    sequence_id: self.sequence_id,
                    failed_atom_index: u32::try_from(self.next_atom_index).unwrap_or(u32::MAX),
                    failed_operation_id: sequence_operation_id(source_atom),
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

        let applied = match apply(
            writer,
            event_log,
            sequence_operation_id(source_atom),
            InputSource::Agent,
            atom.input_kind,
            atom.descriptor,
            Some(atom.terminal_modes_hash),
            atom.semantic_input,
            match source_atom {
                InputSequenceAtom::Bytes { caller_intent, .. }
                | InputSequenceAtom::Key { caller_intent, .. }
                | InputSequenceAtom::Paste { caller_intent, .. } => caller_intent.clone(),
                InputSequenceAtom::Sleep { .. } => None,
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
                operation_id: sequence_operation_id(source_atom),
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

    pub(super) fn interrupted(
        &self,
        reason: &str,
        event_log: &EventLogWriter,
    ) -> (Error, MutationEffect) {
        let completed_atoms = u32::try_from(self.next_atom_index).unwrap_or(u32::MAX);
        let error = Error::InputSequenceIncomplete {
            sequence_id: self.sequence_id,
            failed_atom_index: completed_atoms,
            failed_operation_id: self
                .atoms
                .get(self.next_atom_index)
                .map_or(self.sequence_id, sequence_operation_id),
            atoms_applied: completed_atoms,
            bytes_written: self.total_bytes_written,
            failed_atom_bytes_written: 0,
            first_intent_event_seq: self
                .first_intent_event_seq
                .unwrap_or_else(|| return event_log.next_seq()),
            last_result_event_seq: self
                .last_result_event_seq
                .unwrap_or_else(|| return event_log.next_seq()),
            reason: reason.to_owned(),
        };

        return (
            error,
            if completed_atoms == 0 {
                MutationEffect::NotApplied
            } else {
                MutationEffect::Partial { completed_atoms }
            },
        );
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
        if !operation_ids.insert(sequence_operation_id(atom)) {
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
            InputSequenceAtom::Key { .. } | InputSequenceAtom::Sleep { .. } => {}
        }
    }

    return Ok(());
}

fn prepare_input_sequence_atom<'data>(
    atom: &'data InputSequenceAtom,
    semantic: &mut GhosttyVt,
) -> Result<PreparedInput<'data>> {
    return match atom {
        InputSequenceAtom::Sleep { duration_ns, .. } => Ok(PreparedInput {
            input_kind: InputKind::Sleep,
            descriptor: InputDescriptor::Sleep {
                duration_ns: *duration_ns,
            },
            semantic_input: Some(format!("sleep:{duration_ns}ns")),
            terminal_modes_hash: mode_hash(&semantic.snapshot()?.modes, "serialize delay modes")?,
            data: Cow::Borrowed(&[]),
        }),
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
            InputKind::Sleep => InputDescriptor::Raw,
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
                InputKind::Sleep => "sleep",
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
        semantic_input: Some(semantic_key(key, modifiers)),
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

pub(super) fn sequence_operation_id(atom: &InputSequenceAtom) -> Uuid {
    return match atom {
        InputSequenceAtom::Bytes { operation_id, .. }
        | InputSequenceAtom::Key { operation_id, .. }
        | InputSequenceAtom::Paste { operation_id, .. }
        | InputSequenceAtom::Sleep { operation_id, .. } => *operation_id,
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

const WRITE_CHUNK_BYTES: usize = 4096;

fn write_input(writer: &mut dyn Write, data: &[u8]) -> (u64, Option<IoError>) {
    let mut offset = 0_usize;
    while offset < data.len() {
        let chunk_len = (data.len().saturating_sub(offset)).min(WRITE_CHUNK_BYTES);
        let Some(chunk) = data.get(offset..offset.saturating_add(chunk_len)) else {
            return (
                u64::try_from(offset).unwrap_or(u64::MAX),
                Some(IoError::other("PTY writer reported an invalid byte count")),
            );
        };

        match writer.write(chunk) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{ErrorKind, Result as IoResult};

    struct ChunkTrackingWriter {
        chunks: Vec<usize>,
        max_writes_before_timeout: Option<usize>,
    }

    impl Write for ChunkTrackingWriter {
        fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
            if self
                .max_writes_before_timeout
                .is_some_and(|max| return self.chunks.len() >= max)
            {
                return Err(IoError::new(ErrorKind::TimedOut, "mock timeout"));
            }

            self.chunks.push(buf.len());

            return Ok(buf.len());
        }

        fn flush(&mut self) -> IoResult<()> {
            return Ok(());
        }
    }

    #[test]
    fn write_input_chunks_large_input() {
        let mut writer = ChunkTrackingWriter {
            chunks: Vec::new(),
            max_writes_before_timeout: None,
        };

        let large_data = vec![b'a'; 16 * 1024];
        let (bytes_written, error) = write_input(&mut writer, &large_data);
        assert!(error.is_none());
        assert_eq!(bytes_written, 16 * 1024);
        assert_eq!(writer.chunks.len(), 4);

        for chunk_size in writer.chunks {
            assert!(chunk_size <= WRITE_CHUNK_BYTES);
        }
    }

    #[test]
    fn write_input_reports_exact_bytes_written_on_timeout() {
        let mut writer = ChunkTrackingWriter {
            chunks: Vec::new(),
            max_writes_before_timeout: Some(1),
        };

        let large_data = vec![b'b'; 16 * 1024];
        let (bytes_written, error) = write_input(&mut writer, &large_data);
        assert!(error.is_some());
        assert_eq!(error.expect("timeout error").kind(), ErrorKind::TimedOut);
        assert_eq!(
            bytes_written,
            u64::try_from(WRITE_CHUNK_BYTES).expect("chunk size fits")
        );
        assert_eq!(writer.chunks.len(), 1);
    }

    #[test]
    fn interpolate_cell_forward() {
        let values: Vec<u16> = (0..=10)
            .map(|step| return interpolate_cell(0, 10, step, 10))
            .collect();
        assert_eq!(values, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        assert_eq!(interpolate_cell(10, 20, 0, 4), 10);
        assert_eq!(interpolate_cell(10, 20, 1, 4), 13);
        assert_eq!(interpolate_cell(10, 20, 2, 4), 15);
        assert_eq!(interpolate_cell(10, 20, 3, 4), 18);
        assert_eq!(interpolate_cell(10, 20, 4, 4), 20);
        assert_eq!(interpolate_cell(10, 20, 99, 4), 20);
    }

    #[test]
    fn interpolate_cell_reverse() {
        let values: Vec<u16> = (0..=10)
            .map(|step| return interpolate_cell(10, 0, step, 10))
            .collect();
        assert_eq!(values, vec![10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0]);

        // Explicit check for TT-13: reverse drag from 10 to 0 on final step is 0, not 1
        assert_eq!(interpolate_cell(10, 0, 10, 10), 0);
        assert_eq!(interpolate_cell(10, 0, 11, 10), 0);
        assert_eq!(interpolate_cell(20, 10, 0, 4), 20);
        assert_eq!(interpolate_cell(20, 10, 1, 4), 18);
        assert_eq!(interpolate_cell(20, 10, 2, 4), 15);
        assert_eq!(interpolate_cell(20, 10, 3, 4), 13);
        assert_eq!(interpolate_cell(20, 10, 4, 4), 10);
    }

    #[test]
    fn interpolate_cell_diagonal() {
        // Drag from (0, 10) to (10, 0) in 10 steps
        for step in 0..=10 {
            let col = interpolate_cell(0, 10, step, 10);
            let row = interpolate_cell(10, 0, step, 10);
            assert_eq!(col + row, 10);
        }

        // Drag from (0, 0) to (20, 10) in 10 steps
        assert_eq!(interpolate_cell(0, 20, 0, 10), 0);
        assert_eq!(interpolate_cell(0, 10, 0, 10), 0);
        assert_eq!(interpolate_cell(0, 20, 5, 10), 10);
        assert_eq!(interpolate_cell(0, 10, 5, 10), 5);
        assert_eq!(interpolate_cell(0, 20, 10, 10), 20);
        assert_eq!(interpolate_cell(0, 10, 10, 10), 10);
    }

    #[test]
    fn interpolate_cell_single_step() {
        assert_eq!(interpolate_cell(5, 20, 0, 1), 5);
        assert_eq!(interpolate_cell(5, 20, 1, 1), 20);
        assert_eq!(interpolate_cell(5, 20, 2, 1), 20);
        assert_eq!(interpolate_cell(20, 5, 0, 1), 20);
        assert_eq!(interpolate_cell(20, 5, 1, 1), 5);
        assert_eq!(interpolate_cell(20, 5, 2, 1), 5);
    }

    #[test]
    fn interpolate_cell_identical_start_end() {
        for step in 0..=12 {
            assert_eq!(interpolate_cell(42, 42, step, 8), 42);
        }
    }

    #[test]
    fn interpolate_cell_boundary_coordinates() {
        // Zero steps returns end
        assert_eq!(interpolate_cell(10, 20, 0, 0), 20);
        assert_eq!(interpolate_cell(20, 10, 0, 0), 10);
        assert_eq!(interpolate_cell(0, u16::MAX, 5, 0), u16::MAX);

        // Boundary values with u16::MAX
        assert_eq!(interpolate_cell(0, u16::MAX, 0, 5), 0);
        assert_eq!(interpolate_cell(0, u16::MAX, 5, 5), u16::MAX);
        assert_eq!(interpolate_cell(0, u16::MAX, 6, 5), u16::MAX);
        assert_eq!(interpolate_cell(u16::MAX, 0, 0, 5), u16::MAX);
        assert_eq!(interpolate_cell(u16::MAX, 0, 5, 5), 0);
        assert_eq!(interpolate_cell(u16::MAX, 0, 10, 5), 0);

        // Step much larger than steps
        assert_eq!(interpolate_cell(10, 20, u32::MAX, 10), 20);
    }
}
