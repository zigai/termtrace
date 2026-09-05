use super::*;

impl SessionActor<'_> {
    pub(super) fn execute_control_command(
        &mut self,
        request_id: Uuid,
        command: &ControlCommand,
    ) -> Result<ControlData> {
        if let Some(outcome) = self.execute_observation_command(request_id, command) {
            return outcome;
        }
        match command {
            ControlCommand::Input {
                data,
                input_kind,
                caller_intent,
            } => {
                validate_control_input(data.as_bytes(), caller_intent.as_deref())?;
                let modes = self.live.semantic.snapshot()?.modes;
                let mode_bytes = serde_json::to_vec(&modes).map_err(|error| {
                    return terminal_error("serialize input modes", error.to_string());
                })?;
                let terminal_modes_hash = blake3::hash(&mode_bytes).to_hex().to_string();
                let semantic_input = match input_kind {
                    InputKind::Text => "text",
                    InputKind::Raw => "raw",
                    InputKind::Keys => "keys",
                    InputKind::Paste => "paste",
                    InputKind::Focus => "focus",
                    InputKind::Pointer => "pointer",
                };
                let applied = apply_input(
                    self.spawned.writer.as_mut(),
                    &mut self.event_log,
                    request_id,
                    InputSource::Agent,
                    *input_kind,
                    match input_kind {
                        InputKind::Text => InputDescriptor::Text,
                        InputKind::Raw
                        | InputKind::Keys
                        | InputKind::Paste
                        | InputKind::Focus
                        | InputKind::Pointer => InputDescriptor::Raw,
                    },
                    Some(terminal_modes_hash),
                    Some(semantic_input.to_owned()),
                    caller_intent.clone(),
                    data.as_bytes(),
                )?;
                return Ok(ControlData::InputApplied {
                    operation_id: request_id,
                    input_kind: *input_kind,
                    data: data.clone(),
                    bytes_written: applied.bytes_written,
                    intent_event_seq: applied.intent_event_seq,
                    result_event_seq: applied.result_event_seq,
                    boundaries: actor_boundaries(&self.event_log),
                });
            }
            ControlCommand::InputSequence { .. } | ControlCommand::Act { .. } => {
                return Err(terminal_error(
                    "dispatch input sequence",
                    "paced input sequence was not scheduled by the actor",
                ));
            }
            ControlCommand::Key { key, modifiers } => {
                let modes = self.live.semantic.snapshot()?.modes;
                let mode_bytes = serde_json::to_vec(&modes).map_err(|error| {
                    return terminal_error("serialize keyboard modes", error.to_string());
                })?;
                let terminal_modes_hash = blake3::hash(&mode_bytes).to_hex().to_string();
                let encoded = self.live.semantic.encode_key(*key, *modifiers)?;
                if encoded.data.as_bytes().is_empty() {
                    return Err(control_rejected(
                        "key_has_no_output",
                        "the requested key press produced no terminal bytes",
                    ));
                }
                let applied = apply_input(
                    self.spawned.writer.as_mut(),
                    &mut self.event_log,
                    request_id,
                    InputSource::Agent,
                    InputKind::Keys,
                    InputDescriptor::Key {
                        key: semantic_key_name(*key),
                        shift: modifiers.shift,
                        control: modifiers.control,
                        alt: modifiers.alt,
                        super_key: modifiers.super_key,
                        encoder: encoded.encoder,
                        terminal_modes_hash: terminal_modes_hash.clone(),
                    },
                    Some(terminal_modes_hash),
                    Some(semantic_key_input(*key, *modifiers)),
                    None,
                    encoded.data.as_bytes(),
                )?;
                return Ok(ControlData::InputApplied {
                    operation_id: request_id,
                    input_kind: InputKind::Keys,
                    data: encoded.data,
                    bytes_written: applied.bytes_written,
                    intent_event_seq: applied.intent_event_seq,
                    result_event_seq: applied.result_event_seq,
                    boundaries: actor_boundaries(&self.event_log),
                });
            }
            ControlCommand::Paste { data } => {
                validate_control_input(data.as_bytes(), Some("paste"))?;
                let modes = self.live.semantic.snapshot()?.modes;
                let mode_bytes = serde_json::to_vec(&modes).map_err(|error| {
                    return terminal_error("serialize paste modes", error.to_string());
                })?;
                let terminal_modes_hash = blake3::hash(&mode_bytes).to_hex().to_string();
                let encoded = encode_paste(data.as_bytes(), modes.bracketed_paste)?;
                let applied = apply_input(
                    self.spawned.writer.as_mut(),
                    &mut self.event_log,
                    request_id,
                    InputSource::Agent,
                    InputKind::Paste,
                    InputDescriptor::Paste {
                        bracketed: modes.bracketed_paste,
                        terminal_modes_hash: terminal_modes_hash.clone(),
                    },
                    Some(terminal_modes_hash),
                    Some("paste".to_owned()),
                    None,
                    &encoded,
                )?;
                return Ok(ControlData::InputApplied {
                    operation_id: request_id,
                    input_kind: InputKind::Paste,
                    data: ByteString::new(encoded),
                    bytes_written: applied.bytes_written,
                    intent_event_seq: applied.intent_event_seq,
                    result_event_seq: applied.result_event_seq,
                    boundaries: actor_boundaries(&self.event_log),
                });
            }
            ControlCommand::Focus { focused, strict } => {
                let modes = self.live.semantic.snapshot()?.modes;
                let mode_bytes = serde_json::to_vec(&modes).map_err(|error| {
                    return terminal_error("serialize focus modes", error.to_string());
                })?;
                let terminal_modes_hash = blake3::hash(&mode_bytes).to_hex().to_string();
                let reporting_enabled = modes
                    .values
                    .iter()
                    .any(|mode| return !mode.ansi && mode.number == 1004 && mode.enabled);
                if !reporting_enabled {
                    if *strict {
                        return Err(control_rejected(
                            "focus_reporting_disabled",
                            "the application has not enabled DEC focus reporting mode 1004",
                        ));
                    }
                    return Ok(ControlData::InputNoop {
                        input_kind: InputKind::Focus,
                        reason: "focus_reporting_disabled".to_owned(),
                        terminal_modes_hash,
                        boundaries: actor_boundaries(&self.event_log),
                    });
                }
                let encoded = encode_focus(if *focused {
                    FocusEvent::In
                } else {
                    FocusEvent::Out
                })?;
                let applied = apply_input(
                    self.spawned.writer.as_mut(),
                    &mut self.event_log,
                    request_id,
                    InputSource::Agent,
                    InputKind::Focus,
                    InputDescriptor::Focus {
                        focused: *focused,
                        encoder: "libghostty-focus-encoder-v1".to_owned(),
                        terminal_modes_hash: terminal_modes_hash.clone(),
                    },
                    Some(terminal_modes_hash),
                    Some(if *focused { "focus-in" } else { "focus-out" }.to_owned()),
                    None,
                    encoded.as_bytes(),
                )?;
                return Ok(ControlData::InputApplied {
                    operation_id: request_id,
                    input_kind: InputKind::Focus,
                    data: encoded,
                    bytes_written: applied.bytes_written,
                    intent_event_seq: applied.intent_event_seq,
                    result_event_seq: applied.result_event_seq,
                    boundaries: actor_boundaries(&self.event_log),
                });
            }
            ControlCommand::Pointer {
                column,
                row,
                operation,
                strict,
            } => {
                if *column >= self.live.geometry.cols || *row >= self.live.geometry.rows {
                    return Err(control_rejected(
                        "pointer_cell_out_of_bounds",
                        &format!(
                            "pointer cell {column},{row} is outside {}x{} geometry",
                            self.live.geometry.cols, self.live.geometry.rows
                        ),
                    ));
                }
                let modes = self.live.semantic.snapshot()?.modes;
                let mode_bytes = serde_json::to_vec(&modes).map_err(|error| {
                    return terminal_error("serialize pointer modes", error.to_string());
                })?;
                let terminal_modes_hash = blake3::hash(&mode_bytes).to_hex().to_string();
                let reporting_enabled = modes.values.iter().any(|mode| {
                    return !mode.ansi
                        && matches!(mode.number, 9 | 1000 | 1002 | 1003)
                        && mode.enabled;
                });
                if !reporting_enabled {
                    if *strict {
                        return Err(control_rejected(
                            "pointer_reporting_disabled",
                            "the application has not enabled a DEC mouse tracking mode",
                        ));
                    }
                    return Ok(ControlData::InputNoop {
                        input_kind: InputKind::Pointer,
                        reason: "pointer_reporting_disabled".to_owned(),
                        terminal_modes_hash,
                        boundaries: actor_boundaries(&self.event_log),
                    });
                }
                let mut encoded = Vec::new();
                let (operation_name, button_name, lines, end_cell, click_count) = match operation {
                    PointerOperation::Click { button } => {
                        if matches!(button, MouseButton::WheelUp | MouseButton::WheelDown) {
                            return Err(control_rejected(
                                "invalid_pointer_button",
                                "wheel buttons are reserved for scroll operations",
                            ));
                        }
                        for action in [MouseAction::Press, MouseAction::Release] {
                            encoded.extend_from_slice(
                                self.live
                                    .semantic
                                    .encode_mouse(
                                        action,
                                        *button,
                                        *column,
                                        *row,
                                        self.live.geometry.cols,
                                        self.live.geometry.rows,
                                        self.live.geometry.pixel_width,
                                        self.live.geometry.pixel_height,
                                    )?
                                    .as_bytes(),
                            );
                        }
                        ("click", mouse_button_name(*button), 0, None, 1)
                    }
                    PointerOperation::MultiClick { button, count } => {
                        if matches!(button, MouseButton::WheelUp | MouseButton::WheelDown)
                            || !(2..=4).contains(count)
                        {
                            return Err(control_rejected(
                                "invalid_multi_click",
                                "multi-click requires a non-wheel button and a count from 2 through 4",
                            ));
                        }
                        for _ in 0..*count {
                            for action in [MouseAction::Press, MouseAction::Release] {
                                encoded.extend_from_slice(
                                    self.live
                                        .semantic
                                        .encode_mouse(
                                            action,
                                            *button,
                                            *column,
                                            *row,
                                            self.live.geometry.cols,
                                            self.live.geometry.rows,
                                            self.live.geometry.pixel_width,
                                            self.live.geometry.pixel_height,
                                        )?
                                        .as_bytes(),
                                );
                            }
                        }
                        ("multi_click", mouse_button_name(*button), 0, None, *count)
                    }
                    PointerOperation::Drag {
                        button,
                        to_column,
                        to_row,
                        steps,
                    } => {
                        if matches!(button, MouseButton::WheelUp | MouseButton::WheelDown)
                            || *steps == 0
                            || *steps > 100
                            || *to_column >= self.live.geometry.cols
                            || *to_row >= self.live.geometry.rows
                        {
                            return Err(control_rejected(
                                "invalid_pointer_drag",
                                "drag requires a non-wheel button, an in-bounds destination, and 1 through 100 motion steps",
                            ));
                        }
                        encoded.extend_from_slice(
                            self.live
                                .semantic
                                .encode_mouse(
                                    MouseAction::Press,
                                    *button,
                                    *column,
                                    *row,
                                    self.live.geometry.cols,
                                    self.live.geometry.rows,
                                    self.live.geometry.pixel_width,
                                    self.live.geometry.pixel_height,
                                )?
                                .as_bytes(),
                        );
                        let step_count = u32::from(*steps);
                        for step in 1..=step_count {
                            let drag_column =
                                interpolate_cell(*column, *to_column, step, step_count);
                            let drag_row = interpolate_cell(*row, *to_row, step, step_count);
                            encoded.extend_from_slice(
                                self.live
                                    .semantic
                                    .encode_mouse(
                                        MouseAction::Motion,
                                        *button,
                                        drag_column,
                                        drag_row,
                                        self.live.geometry.cols,
                                        self.live.geometry.rows,
                                        self.live.geometry.pixel_width,
                                        self.live.geometry.pixel_height,
                                    )?
                                    .as_bytes(),
                            );
                        }
                        encoded.extend_from_slice(
                            self.live
                                .semantic
                                .encode_mouse(
                                    MouseAction::Release,
                                    *button,
                                    *to_column,
                                    *to_row,
                                    self.live.geometry.cols,
                                    self.live.geometry.rows,
                                    self.live.geometry.pixel_width,
                                    self.live.geometry.pixel_height,
                                )?
                                .as_bytes(),
                        );
                        (
                            "drag",
                            mouse_button_name(*button),
                            0,
                            Some((*to_column, *to_row)),
                            0,
                        )
                    }
                    PointerOperation::Scroll { lines } => {
                        if *lines == 0 || lines.unsigned_abs() > 100 {
                            return Err(control_rejected(
                                "invalid_scroll_lines",
                                "scroll lines must be between -100 and 100 and nonzero",
                            ));
                        }
                        let button = if *lines < 0 {
                            MouseButton::WheelUp
                        } else {
                            MouseButton::WheelDown
                        };
                        for _ in 0..lines.unsigned_abs() {
                            encoded.extend_from_slice(
                                self.live
                                    .semantic
                                    .encode_mouse(
                                        MouseAction::Press,
                                        button,
                                        *column,
                                        *row,
                                        self.live.geometry.cols,
                                        self.live.geometry.rows,
                                        self.live.geometry.pixel_width,
                                        self.live.geometry.pixel_height,
                                    )?
                                    .as_bytes(),
                            );
                        }
                        ("scroll", mouse_button_name(button), *lines, None, 0)
                    }
                };
                if encoded.is_empty() {
                    if *strict {
                        return Err(control_rejected(
                            "pointer_has_no_output",
                            "the current tracking mode suppressed this pointer operation",
                        ));
                    }
                    return Ok(ControlData::InputNoop {
                        input_kind: InputKind::Pointer,
                        reason: "pointer_has_no_output".to_owned(),
                        terminal_modes_hash,
                        boundaries: actor_boundaries(&self.event_log),
                    });
                }
                let applied = apply_input(
                    self.spawned.writer.as_mut(),
                    &mut self.event_log,
                    request_id,
                    InputSource::Agent,
                    InputKind::Pointer,
                    InputDescriptor::Pointer {
                        operation: operation_name.to_owned(),
                        button: button_name.to_owned(),
                        lines,
                        column: *column,
                        row: *row,
                        end_column: end_cell.map(|cell| return cell.0),
                        end_row: end_cell.map(|cell| return cell.1),
                        click_count,
                        cols: self.live.geometry.cols,
                        rows: self.live.geometry.rows,
                        pixel_width: self.live.geometry.pixel_width,
                        pixel_height: self.live.geometry.pixel_height,
                        encoder: "libghostty-mouse-encoder-v1".to_owned(),
                        terminal_modes_hash: terminal_modes_hash.clone(),
                    },
                    Some(terminal_modes_hash),
                    Some(format!("pointer-{operation_name}")),
                    None,
                    &encoded,
                )?;
                return Ok(ControlData::InputApplied {
                    operation_id: request_id,
                    input_kind: InputKind::Pointer,
                    data: ByteString::new(encoded),
                    bytes_written: applied.bytes_written,
                    intent_event_seq: applied.intent_event_seq,
                    result_event_seq: applied.result_event_seq,
                    boundaries: actor_boundaries(&self.event_log),
                });
            }
            ControlCommand::Activate {
                text,
                unique,
                strict,
            } => {
                if text.is_empty() || text.len() > 4096 || text.contains('\n') {
                    return Err(control_rejected(
                        "invalid_activation_text",
                        "activation text must contain 1 to 4096 bytes on one row",
                    ));
                }
                let semantic_state = self.live.semantic.snapshot()?;
                if semantic_state.modes.synchronized_output {
                    return Err(control_rejected(
                        "presentation_transaction_active",
                        "text activation is unavailable during synchronized output",
                    ));
                }
                let state_hash = presented_state_hash(&self.live)?;
                let matches = find_text_regions(&self.live.presented, text);
                let visible = matches
                    .iter()
                    .filter(|region| return !region.concealed)
                    .collect::<Vec<_>>();
                if visible.is_empty() {
                    let code = if matches.is_empty() {
                        "activation_text_not_found"
                    } else {
                        "activation_text_concealed"
                    };
                    return Err(control_rejected(
                        code,
                        "activation requires a visible row-contained text match",
                    ));
                }
                if *unique && visible.len() != 1 {
                    let candidates = visible
                        .iter()
                        .map(|region| {
                            return format!(
                                "{}:{}-{}",
                                region.row, region.start_column, region.end_column
                            );
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    return Err(control_rejected(
                        "activation_text_ambiguous",
                        &format!("activation text matched multiple regions: {candidates}"),
                    ));
                }
                let region = visible.first().ok_or_else(|| {
                    return control_rejected("activation_text_not_found", "no match");
                })?;
                let target_column = region
                    .start_column
                    .saturating_add(region.end_column.saturating_sub(region.start_column + 1) / 2);
                let mode_bytes = serde_json::to_vec(&semantic_state.modes).map_err(|error| {
                    return terminal_error("serialize activation modes", error.to_string());
                })?;
                let terminal_modes_hash = blake3::hash(&mode_bytes).to_hex().to_string();
                let reporting_enabled = semantic_state.modes.values.iter().any(|mode| {
                    return !mode.ansi
                        && matches!(mode.number, 9 | 1000 | 1002 | 1003)
                        && mode.enabled;
                });
                if !reporting_enabled {
                    if *strict {
                        return Err(control_rejected(
                            "pointer_reporting_disabled",
                            "the application has not enabled a DEC mouse tracking mode",
                        ));
                    }
                    return Ok(ControlData::InputNoop {
                        input_kind: InputKind::Pointer,
                        reason: "pointer_reporting_disabled".to_owned(),
                        terminal_modes_hash,
                        boundaries: actor_boundaries(&self.event_log),
                    });
                }
                let mut encoded = Vec::new();
                for action in [MouseAction::Press, MouseAction::Release] {
                    encoded.extend_from_slice(
                        self.live
                            .semantic
                            .encode_mouse(
                                action,
                                MouseButton::Left,
                                target_column,
                                region.row,
                                self.live.geometry.cols,
                                self.live.geometry.rows,
                                self.live.geometry.pixel_width,
                                self.live.geometry.pixel_height,
                            )?
                            .as_bytes(),
                    );
                }
                if encoded.is_empty() {
                    if *strict {
                        return Err(control_rejected(
                            "pointer_has_no_output",
                            "the current tracking mode suppressed the activation click",
                        ));
                    }
                    return Ok(ControlData::InputNoop {
                        input_kind: InputKind::Pointer,
                        reason: "pointer_has_no_output".to_owned(),
                        terminal_modes_hash,
                        boundaries: actor_boundaries(&self.event_log),
                    });
                }
                let applied = apply_input(
                    self.spawned.writer.as_mut(),
                    &mut self.event_log,
                    request_id,
                    InputSource::Agent,
                    InputKind::Pointer,
                    InputDescriptor::Activate {
                        text: text.clone(),
                        start_column: region.start_column,
                        end_column: region.end_column,
                        row: region.row,
                        target_column,
                        presented_state_hash: state_hash.clone(),
                        cols: self.live.geometry.cols,
                        rows: self.live.geometry.rows,
                        encoder: "libghostty-mouse-encoder-v1".to_owned(),
                        terminal_modes_hash: terminal_modes_hash.clone(),
                    },
                    Some(terminal_modes_hash),
                    Some("activate-visible-text".to_owned()),
                    None,
                    &encoded,
                )?;
                return Ok(ControlData::Activated {
                    operation_id: request_id,
                    text: text.clone(),
                    start_column: region.start_column,
                    end_column: region.end_column,
                    row: region.row,
                    target_column,
                    presented_state_hash: state_hash,
                    data: ByteString::new(encoded),
                    bytes_written: applied.bytes_written,
                    intent_event_seq: applied.intent_event_seq,
                    result_event_seq: applied.result_event_seq,
                    boundaries: actor_boundaries(&self.event_log),
                });
            }
            ControlCommand::Resize {
                cols,
                rows,
                pixel_width,
                pixel_height,
            } => {
                let geometry = TerminalGeometry {
                    cols: *cols,
                    rows: *rows,
                    pixel_width: *pixel_width,
                    pixel_height: *pixel_height,
                };
                validate_control_geometry(geometry)?;
                apply_resize(
                    self.spawned.master.as_mut(),
                    &mut self.event_log,
                    request_id,
                    geometry,
                )?;
                resize_semantic(&mut self.live.semantic, geometry)?;
                self.live.geometry = geometry;
                let latest = self.live.semantic.snapshot()?;
                if !latest.modes.synchronized_output {
                    self.live.presented = latest;
                }
                return Ok(ControlData::Acknowledged {
                    boundaries: actor_boundaries(&self.event_log),
                });
            }
            ControlCommand::Signal { signal } => {
                apply_signal(
                    self.spawned.pid,
                    &mut self.event_log,
                    request_id,
                    *signal,
                    false,
                )?;
                return Ok(ControlData::Acknowledged {
                    boundaries: actor_boundaries(&self.event_log),
                });
            }
            ControlCommand::Mark { label } => {
                let label = label.trim();
                if label.is_empty() || label.len() > 256 {
                    return Err(control_rejected(
                        "invalid_marker",
                        "marker must contain 1 to 256 UTF-8 bytes",
                    ));
                }
                let event_seq = self.event_log.append(Event::Marker {
                    operation_id: request_id,
                    label: label.to_owned(),
                })?;
                return Ok(ControlData::Marked {
                    label: label.to_owned(),
                    event_seq,
                    boundaries: actor_boundaries(&self.event_log),
                });
            }
            ControlCommand::Stop => {
                apply_signal(
                    self.spawned.pid,
                    &mut self.event_log,
                    request_id,
                    ProcessSignal::Terminate,
                    false,
                )?;
                if self.host_signal_kill_deadline.is_none() {
                    self.host_signal_kill_deadline =
                        Some(Instant::now() + HOST_SIGNAL_GRACE_PERIOD);
                }
                return Ok(ControlData::Acknowledged {
                    boundaries: actor_boundaries(&self.event_log),
                });
            }
            ControlCommand::Ping
            | ControlCommand::Status
            | ControlCommand::Snapshot
            | ControlCommand::ScreenFrame { .. }
            | ControlCommand::WaitScreen { .. }
            | ControlCommand::CaptureStable { .. } => {
                return Err(control_rejected(
                    "invalid_control_dispatch",
                    "observation command reached the mutation dispatcher",
                ));
            }
        }
    }
}

fn validate_control_geometry(geometry: TerminalGeometry) -> Result<()> {
    if supported_geometry_cell_count(geometry.cols, geometry.rows).is_none()
        || geometry.pixel_width == 0
        || geometry.pixel_height == 0
    {
        return Err(control_rejected(
            "invalid_geometry",
            "terminal geometry exceeds the supported row, column, or cell limit, or has zero pixels",
        ));
    }
    return Ok(());
}

pub(in crate::recording) fn apply_signal(
    pid: Option<u32>,
    event_log: &mut EventLogWriter,
    operation_id: Uuid,
    signal: ProcessSignal,
    missing_is_ok: bool,
) -> Result<()> {
    let (signal, name) = nix_signal(signal);
    event_log.append(Event::SignalIntent {
        operation_id,
        number: signal as i32,
        name: name.to_owned(),
    })?;
    let signal_result = pid
        .and_then(|value| return i32::try_from(value).ok())
        .ok_or(Errno::ESRCH)
        .and_then(|value| return killpg(Pid::from_raw(value), signal));
    let status = if signal_result.is_ok() {
        OperationStatus::Applied
    } else {
        OperationStatus::Failed
    };
    let result_write: Result<()> = event_log
        .append(Event::SignalResult {
            operation_id,
            status,
            error_code: signal_result
                .as_ref()
                .err()
                .map(|error| format!("os_errno_{}", *error as i32)),
        })
        .map(|_| ())
        .map_err(Error::from);
    let Some(error) = signal_result.err() else {
        return result_write;
    };
    if missing_is_ok && error == Errno::ESRCH {
        return result_write;
    }
    return finish_with_cleanup(
        Err(control_rejected(
            "signal_failed",
            &format!("signal child process group failed: {error}"),
        )),
        result_write,
    );
}
fn nix_signal(signal: ProcessSignal) -> (Signal, &'static str) {
    match signal {
        ProcessSignal::Interrupt => return (Signal::SIGINT, "SIGINT"),
        ProcessSignal::Terminate => return (Signal::SIGTERM, "SIGTERM"),
        ProcessSignal::Hangup => return (Signal::SIGHUP, "SIGHUP"),
        ProcessSignal::User1 => return (Signal::SIGUSR1, "SIGUSR1"),
        ProcessSignal::User2 => return (Signal::SIGUSR2, "SIGUSR2"),
    }
}
