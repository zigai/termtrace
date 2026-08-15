use super::super::*;

/// Typed keyboard and byte input operations.
pub struct Keyboard<'session> {
    pub(in crate::client) session: &'session Session,
}

impl Keyboard<'_> {
    /// Apply exact UTF-8 text with a caller-retainable request.
    pub fn text(&self, text: impl Into<String>) -> Result<ControlData> {
        let request = ControlRequest::new(ControlCommand::Input {
            data: ByteString::new(text.into().into_bytes()),
            input_kind: InputKind::Text,
            caller_intent: None,
        });
        return self.session.send(&request);
    }

    /// Encode one symbolic key from live modes.
    pub fn press(&self, key: NamedKey, modifiers: KeyModifiers) -> Result<ControlData> {
        return self
            .session
            .send(&ControlRequest::new(ControlCommand::Key { key, modifiers }));
    }

    /// Encode caller-provided semantic paste content.
    pub fn paste(&self, data: impl Into<Vec<u8>>) -> Result<ControlData> {
        return self
            .session
            .send(&ControlRequest::new(ControlCommand::Paste {
                data: ByteString::new(data.into()),
            }));
    }
}

/// Mode-aware cell pointer and focus operations.
pub struct Pointer<'session> {
    pub(in crate::client) session: &'session Session,
}

impl Pointer<'_> {
    /// Emit a press/release click plan at a zero-based cell.
    pub fn click(
        &self,
        column: u16,
        row: u16,
        button: MouseButton,
        strict: bool,
    ) -> Result<ControlData> {
        return self
            .session
            .send(&ControlRequest::new(ControlCommand::Pointer {
                column,
                row,
                operation: PointerOperation::Click { button },
                strict,
            }));
    }

    /// Emit signed vertical wheel ticks at a zero-based cell.
    pub fn scroll(&self, column: u16, row: u16, lines: i16, strict: bool) -> Result<ControlData> {
        return self
            .session
            .send(&ControlRequest::new(ControlCommand::Pointer {
                column,
                row,
                operation: PointerOperation::Scroll { lines },
                strict,
            }));
    }

    /// Report focus state when the application enabled focus reporting.
    pub fn focus(&self, focused: bool, strict: bool) -> Result<ControlData> {
        return self
            .session
            .send(&ControlRequest::new(ControlCommand::Focus {
                focused,
                strict,
            }));
    }
}
