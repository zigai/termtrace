use super::super::{
    ControlCommand, ControlData, ControlRequest, MouseButton, PointerOperation, Result, Session,
};

/// Mode-aware cell pointer and focus operations.
pub struct Pointer<'session> {
    pub(in crate::client) session: &'session Session,
}

impl Pointer<'_> {
    /// Emit a press/release click plan at a zero-based cell.
    ///
    /// # Errors
    /// Returns an error when the recorder cannot be reached or rejects the request; with `strict`,
    /// a request the application does not report is rejected instead of returning a semantic no-op.
    pub fn click(
        &self,
        column: u16,
        row: u16,
        button: MouseButton,
        strict: bool,
    ) -> Result<ControlData> {
        return self
            .session
            .control(&ControlRequest::new(ControlCommand::Pointer {
                column,
                row,
                operation: PointerOperation::Click { button },
                strict,
            }));
    }

    /// Emit signed vertical wheel ticks at a zero-based cell.
    ///
    /// # Errors
    /// Returns an error when the recorder cannot be reached or rejects the request; with `strict`,
    /// a request the application does not report is rejected instead of returning a semantic no-op.
    pub fn scroll(&self, column: u16, row: u16, lines: i16, strict: bool) -> Result<ControlData> {
        return self
            .session
            .control(&ControlRequest::new(ControlCommand::Pointer {
                column,
                row,
                operation: PointerOperation::Scroll { lines },
                strict,
            }));
    }

    /// Report focus state when the application enabled focus reporting.
    ///
    /// # Errors
    /// Returns an error when the recorder cannot be reached or rejects the request; with `strict`,
    /// a request the application does not report is rejected instead of returning a semantic no-op.
    pub fn focus(&self, focused: bool, strict: bool) -> Result<ControlData> {
        return self
            .session
            .control(&ControlRequest::new(ControlCommand::Focus {
                focused,
                strict,
            }));
    }
}
