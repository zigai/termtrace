use super::{
    actor_boundaries, ControlErrorDetails, ControlResponse, ControlResult, Error, EventLogWriter,
    MutationEffect, ScreenFrameError, ScreenFrameFailureDetails, ScreenFrameLimitKind, Uuid,
    CONTROL_PROTOCOL_VERSION,
};

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
    effect: MutationEffect,
) -> ControlResponse {
    return ControlResponse {
        version: CONTROL_PROTOCOL_VERSION,
        request_id,
        result: ControlResult::Error {
            code: screen_frame_error_code(error).to_owned(),
            message: error.to_string(),
            effect,
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
        Error::EventLogPoisoned { .. } | Error::InvalidEventLog { .. } => {
            return "event_log_failed"
        }
        Error::Filesystem { .. } => return "io_failed",
        Error::TerminalOperation { .. } => return "terminal_operation_failed",
        _ => return "internal_error",
    }
}

/// Effect of a mutation whose execution returned `error`.
///
/// Every mutation records its canonical intent before it touches the child, so
/// a failure before the first recorded event applied nothing. Rejections apply
/// nothing, and a stopped write reports exactly how far it got.
pub(super) const fn mutation_failure_effect(
    error: &Error,
    recorded_events: bool,
) -> MutationEffect {
    return match error {
        Error::ControlRejected { .. } => MutationEffect::NotApplied,
        Error::InputWriteFailed { bytes_written, .. } => {
            MutationEffect::stopped_input_sequence(0, *bytes_written)
        }
        Error::InputSequenceIncomplete {
            atoms_applied,
            failed_atom_bytes_written,
            ..
        } => MutationEffect::stopped_input_sequence(*atoms_applied, *failed_atom_bytes_written),
        _ if recorded_events => MutationEffect::Unknown,
        _ => MutationEffect::NotApplied,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_failed(bytes_written: u64) -> Error {
        return Error::InputWriteFailed {
            operation_id: Uuid::nil(),
            bytes_written,
            intent_event_seq: 1,
            result_event_seq: 2,
            reason: "closed".to_owned(),
        };
    }

    fn sequence_stopped(atoms_applied: u32, failed_atom_bytes_written: u64) -> Error {
        return Error::InputSequenceIncomplete {
            sequence_id: Uuid::nil(),
            failed_atom_index: atoms_applied,
            failed_operation_id: Uuid::nil(),
            atoms_applied,
            bytes_written: 4,
            failed_atom_bytes_written,
            first_intent_event_seq: 1,
            last_result_event_seq: 4,
            reason: "closed".to_owned(),
        };
    }

    #[test]
    fn rejections_apply_nothing_even_after_recording_intent() {
        let rejected = Error::ControlRejected {
            code: "signal_failed".to_owned(),
            message: "no such process".to_owned(),
        };

        assert_eq!(
            mutation_failure_effect(&rejected, true),
            MutationEffect::NotApplied
        );
    }

    #[test]
    fn writes_report_whole_atoms_or_unknown_delivery() {
        assert_eq!(
            mutation_failure_effect(&write_failed(0), true),
            MutationEffect::NotApplied
        );
        assert_eq!(
            mutation_failure_effect(&write_failed(3), true),
            MutationEffect::Unknown
        );
        assert_eq!(
            mutation_failure_effect(&sequence_stopped(2, 0), true),
            MutationEffect::Partial { completed_atoms: 2 }
        );
        assert_eq!(
            mutation_failure_effect(&sequence_stopped(2, 1), true),
            MutationEffect::Unknown
        );
    }

    #[test]
    fn other_failures_are_unknown_once_the_mutation_recorded_events() {
        let failure = Error::TerminalOperation {
            operation: "resize child PTY",
            reason: "failed".to_owned(),
        };

        assert_eq!(
            mutation_failure_effect(&failure, false),
            MutationEffect::NotApplied
        );
        assert_eq!(
            mutation_failure_effect(&failure, true),
            MutationEffect::Unknown
        );
    }
}
