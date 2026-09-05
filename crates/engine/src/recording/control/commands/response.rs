use super::*;

pub(in crate::recording::control) fn screen_frame_error_code(
    error: &ScreenFrameError,
) -> &'static str {
    return match error {
        ScreenFrameError::InvalidRegion { .. } => "invalid_screen_region",
        ScreenFrameError::ResourceLimit { .. } => "screen_frame_resource_limit",
        ScreenFrameError::InconsistentSemanticState { .. } | ScreenFrameError::Serialization => {
            "screen_frame_projection_failed"
        }
    };
}

pub(in crate::recording::control) fn screen_frame_error_response(
    request_id: Uuid,
    error: &ScreenFrameError,
    current_state_hash: String,
    event_log: &EventLogWriter,
    scope: &str,
) -> ControlResponse {
    return ControlResponse {
        version: CONTROL_PROTOCOL_VERSION,
        request_id,
        result: ControlResult::Error {
            code: screen_frame_error_code(error).to_owned(),
            message: error.to_string(),
            details: Some(ControlErrorDetails {
                current_presented_state_hash: current_state_hash,
                boundaries: actor_boundaries(event_log),
                scope: scope.to_owned(),
                input_sequence: None,
                screen_frame: Some(screen_frame_failure_details(error)),
            }),
        },
    };
}

pub(in crate::recording::control) fn screen_frame_failure_details(
    error: &ScreenFrameError,
) -> ScreenFrameFailureDetails {
    return match error {
        ScreenFrameError::InvalidRegion {
            region,
            cols,
            rows,
            reason,
        } => ScreenFrameFailureDetails {
            detail: None,
            region: *region,
            region_reason: Some(*reason),
            cols: Some(*cols),
            rows: Some(*rows),
            limit_kind: None,
            requested: None,
            maximum: None,
        },
        ScreenFrameError::ResourceLimit {
            limit,
            requested,
            maximum,
            detail,
            region,
        } => ScreenFrameFailureDetails {
            detail: Some(*detail),
            region: Some(*region),
            region_reason: None,
            cols: None,
            rows: None,
            limit_kind: Some(
                match limit {
                    ScreenFrameLimitKind::CellCount => "cell_count",
                    ScreenFrameLimitKind::SerializedBytes => "serialized_bytes",
                    ScreenFrameLimitKind::ResponseBytes => "response_bytes",
                }
                .to_owned(),
            ),
            requested: Some(*requested),
            maximum: Some(*maximum),
        },
        ScreenFrameError::InconsistentSemanticState { .. } | ScreenFrameError::Serialization => {
            ScreenFrameFailureDetails {
                detail: None,
                region: None,
                region_reason: None,
                cols: None,
                rows: None,
                limit_kind: None,
                requested: None,
                maximum: None,
            }
        }
    };
}
pub(super) fn control_error_code(error: &Error) -> &str {
    match error {
        Error::ControlRejected { code, .. } => return code,
        Error::InputSequenceIncomplete { .. } => return "input_sequence_incomplete",
        Error::ScreenFrame(ScreenFrameError::InvalidRegion { .. }) => {
            return "invalid_screen_region";
        }
        Error::ScreenFrame(ScreenFrameError::ResourceLimit { .. }) => {
            return "screen_frame_resource_limit";
        }
        Error::ScreenFrame(_) => return "screen_frame_projection_failed",
        Error::InputWriteFailed { .. } => return "input_write_failed",
        Error::GhosttyVt { operation, .. } if operation.contains("key") => {
            return "key_encoding_failed";
        }
        Error::GhosttyVt { operation, .. } if operation.contains("mouse") => {
            return "pointer_encoding_failed";
        }
        Error::GhosttyVt { .. } => return "terminal_encoding_failed",
        Error::EventLogPoisoned { .. } | Error::InvalidEventLog { .. } => return "event_log_failed",
        Error::Filesystem { .. } => return "io_failed",
        Error::TerminalOperation { .. } => return "terminal_operation_failed",
        _ => return "internal_error",
    }
}
