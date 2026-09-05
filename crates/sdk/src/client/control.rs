use super::*;

pub(super) fn validate_action_options(atoms: &[InputAtom], options: &ActionOptions) -> Result<()> {
    if atoms.is_empty() || atoms.len() > 4096 {
        return Err(Error::InvalidAtom {
            atom_index: atoms.len(),
            reason: "action must contain 1 through 4096 atoms".to_owned(),
        });
    }
    if options
        .caller_intent
        .as_ref()
        .is_some_and(|intent| return intent.len() > 1024)
    {
        return Err(Error::InvalidAtom {
            atom_index: 0,
            reason: "caller intent must not exceed 1024 UTF-8 bytes".to_owned(),
        });
    }
    for (index, atom) in atoms.iter().enumerate() {
        let byte_len = match atom {
            InputAtom::Text(text) | InputAtom::Paste(text) => text.len(),
            InputAtom::Bytes(bytes) => bytes.len(),
            InputAtom::Key { .. } => continue,
        };
        if byte_len == 0 || byte_len > 64 * 1024 {
            return Err(Error::InvalidAtom {
                atom_index: index,
                reason: "text, bytes, and paste atoms must contain 1 through 65536 bytes"
                    .to_owned(),
            });
        }
    }
    let pace_ns = exact_duration_ns("pace", options.pace)?;
    if pace_ns > 60_000_000_000 {
        return Err(Error::InvalidDuration {
            field: "pace",
            reason: "pace may not exceed 60 seconds".to_owned(),
        });
    }
    let atom_count = u64::try_from(atoms.len()).unwrap_or(u64::MAX);
    if pace_ns
        .checked_mul(atom_count)
        .is_none_or(|total| return total > 60_000_000_000)
    {
        return Err(Error::InvalidDuration {
            field: "pace",
            reason: "pace multiplied by atom count may not exceed 60 seconds".to_owned(),
        });
    }
    validate_postcondition_durations(&options.postcondition)?;
    if matches!(options.projection.detail, FrameDetail::Cells)
        && options.projection.region.is_none()
    {
        return Err(Error::ScreenFrame(ScreenFrameError::InvalidRegion {
            region: None,
            cols: 0,
            rows: 0,
            reason: InvalidRegionReason::CellsRequireExplicitRegion,
        }));
    }
    return Ok(());
}

pub(super) fn validate_postcondition_durations(postcondition: &ActionCondition) -> Result<()> {
    match postcondition {
        ActionCondition::None => return Ok(()),
        ActionCondition::Idle {
            stable_for,
            timeout,
        } => {
            let stable_for_ns = exact_duration_ns("stable_for", *stable_for)?;
            let timeout_ns = exact_duration_ns("timeout", *timeout)?;
            if stable_for_ns == 0 {
                return Err(Error::InvalidDuration {
                    field: "stable_for",
                    reason: "idle stability must be greater than zero".to_owned(),
                });
            }
            if stable_for_ns > timeout_ns {
                return Err(Error::InvalidDuration {
                    field: "timeout",
                    reason: "timeout must be nonzero and at least stable_for".to_owned(),
                });
            }
            if timeout_ns > 3_600_000_000_000 {
                return Err(Error::InvalidDuration {
                    field: "timeout",
                    reason: "timeout may not exceed one hour".to_owned(),
                });
            }
        }
        ActionCondition::Screen { timeout, .. } => {
            let timeout_ns = exact_duration_ns("timeout", *timeout)?;
            if timeout_ns == 0 || timeout_ns > 3_600_000_000_000 {
                return Err(Error::InvalidDuration {
                    field: "timeout",
                    reason: "screen timeout must be nonzero and at most one hour".to_owned(),
                });
            }
        }
    }
    return Ok(());
}

pub(super) fn wire_postcondition(postcondition: &ActionCondition) -> Result<ActionPostcondition> {
    return match postcondition {
        ActionCondition::None => Ok(ActionPostcondition::None),
        ActionCondition::Idle {
            stable_for,
            timeout,
        } => Ok(ActionPostcondition::Idle {
            stable_for_ns: exact_duration_ns("stable_for", *stable_for)?,
            timeout_ns: exact_duration_ns("timeout", *timeout)?,
        }),
        ActionCondition::Screen { predicate, timeout } => Ok(ActionPostcondition::Screen {
            predicate: predicate.clone(),
            timeout_ns: exact_duration_ns("timeout", *timeout)?,
        }),
    };
}

pub(super) fn validate_stable_options(options: StableCaptureOptions) -> Result<()> {
    let stable_for_ns = exact_duration_ns("stable_for", options.stable_for)?;
    let timeout_ns = exact_duration_ns("timeout", options.timeout)?;
    if stable_for_ns == 0 {
        return Err(Error::InvalidDuration {
            field: "stable_for",
            reason: "stable_for must be greater than zero".to_owned(),
        });
    }
    if stable_for_ns > timeout_ns || timeout_ns > 3_600_000_000_000 {
        return Err(Error::InvalidDuration {
            field: "timeout",
            reason: "timeout must satisfy stable_for <= timeout <= one hour".to_owned(),
        });
    }
    if matches!(options.projection.detail, FrameDetail::Cells)
        && options.projection.region.is_none()
    {
        return Err(Error::ScreenFrame(ScreenFrameError::InvalidRegion {
            region: None,
            cols: 0,
            rows: 0,
            reason: InvalidRegionReason::CellsRequireExplicitRegion,
        }));
    }
    return Ok(());
}

pub(super) fn exact_duration_ns(field: &'static str, duration: Duration) -> Result<u64> {
    return u64::try_from(duration.as_nanos()).map_err(|_| {
        return Error::InvalidDuration {
            field,
            reason: "duration exceeds unsigned 64-bit nanoseconds".to_owned(),
        };
    });
}

pub(super) fn action_atom(
    atom: &InputAtom,
    operation_id: Uuid,
    caller_intent: Option<String>,
) -> InputSequenceAtom {
    return match atom {
        InputAtom::Text(text) => InputSequenceAtom::Bytes {
            operation_id,
            data: ByteString::new(text.as_bytes().to_vec()),
            input_kind: InputKind::Text,
            caller_intent,
        },
        InputAtom::Bytes(bytes) => InputSequenceAtom::Bytes {
            operation_id,
            data: ByteString::new(bytes.clone()),
            input_kind: InputKind::Raw,
            caller_intent,
        },
        InputAtom::Key { key, modifiers } => InputSequenceAtom::Key {
            operation_id,
            key: *key,
            modifiers: *modifiers,
            caller_intent,
        },
        InputAtom::Paste(text) => InputSequenceAtom::Paste {
            operation_id,
            data: ByteString::new(text.as_bytes().to_vec()),
            caller_intent,
        },
    };
}

pub(super) fn public_control_error(
    code: String,
    message: String,
    details: Option<ControlErrorDetails>,
    request_id: Option<Uuid>,
    expected_state_hash: Option<String>,
) -> Error {
    if code == "stale_observation"
        && let (Some(expected_state_hash), Some(details)) = (expected_state_hash, details.as_ref())
    {
        return Error::StaleState {
            expected_state_hash,
            current_state_hash: details.current_presented_state_hash.clone(),
            boundaries: Box::new(details.boundaries.clone()),
            retry_safety: RetrySafety::NewRequest,
        };
    }
    if matches!(code.as_str(), "action_busy" | "input_sequence_busy") {
        return Error::ActionBusy { message };
    }
    if code == "input_sequence_incomplete"
        && let Some(details) = details.as_ref()
        && let Some(input_sequence) = details.input_sequence.clone()
    {
        let uncertain = input_sequence.failed_atom_bytes_written != 0;
        return Error::PartialInput {
            details: input_sequence,
            retry_safety: RetrySafety::SameRequestOnly,
            uncertain,
        };
    }
    if let Some(details) = details.as_ref()
        && let Some(screen) = details.screen_frame.as_ref()
    {
        if code == "invalid_screen_region"
            && let (Some(cols), Some(rows), Some(reason)) =
                (screen.cols, screen.rows, screen.region_reason)
        {
            let source = ScreenFrameError::InvalidRegion {
                region: screen.region,
                cols,
                rows,
                reason,
            };
            return public_screen_frame_error(request_id, details, source);
        }
        if code == "screen_frame_resource_limit"
            && let (Some(detail), Some(region), Some(limit_kind), Some(requested), Some(maximum)) = (
                screen.detail,
                screen.region,
                screen.limit_kind.as_deref(),
                screen.requested,
                screen.maximum,
            )
        {
            let limit = match limit_kind {
                "cell_count" => ScreenFrameLimitKind::CellCount,
                "serialized_bytes" => ScreenFrameLimitKind::SerializedBytes,
                "response_bytes" => ScreenFrameLimitKind::ResponseBytes,
                _ => return Error::ControlRejected { code, message },
            };
            let source = ScreenFrameError::ResourceLimit {
                limit,
                requested,
                maximum,
                detail,
                region,
            };
            return public_screen_frame_error(request_id, details, source);
        }
    }
    return Error::ControlRejected { code, message };
}

pub(super) fn public_screen_frame_error(
    request_id: Option<Uuid>,
    details: &ControlErrorDetails,
    source: ScreenFrameError,
) -> Error {
    if details.scope == "action_terminating_screen_frame"
        && let Some(request_id) = request_id
    {
        return Error::ActionTerminatingFrame {
            request_id,
            boundaries: Box::new(details.boundaries.clone()),
            input_fully_applied: true,
            retry_safety: RetrySafety::SameRequestOnly,
            source,
        };
    }
    return Error::ScreenFrame(source);
}

pub(super) fn client_error(reason: impl Into<String>) -> Error {
    return Error::InvalidOperation {
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_projection_failure_retains_generated_request_id() {
        use termtrace_evidence::event_log::CanonicalBoundary;
        use termtrace_model::{
            protocol::control::ScreenFrameFailureDetails, terminal::observation::ActorBoundaries,
        };

        let request_id = Uuid::now_v7();
        let boundary = CanonicalBoundary {
            next_event_seq: 4,
            event_hash: "a".repeat(64),
            output_bytes: 7,
        };
        let boundaries = ActorBoundaries {
            durable: boundary.clone(),
            observed: boundary,
        };
        let details = ControlErrorDetails {
            current_presented_state_hash: "b".repeat(64),
            boundaries: boundaries.clone(),
            scope: "action_terminating_screen_frame".to_owned(),
            input_sequence: None,
            screen_frame: Some(ScreenFrameFailureDetails {
                detail: Some(FrameDetail::Cells),
                region: Some(ScreenRegion {
                    top: 0,
                    left: 0,
                    right: 1,
                    bottom: 1,
                }),
                region_reason: None,
                cols: None,
                rows: None,
                limit_kind: Some("response_bytes".to_owned()),
                requested: Some(2),
                maximum: Some(1),
            }),
        };

        let error = public_control_error(
            "screen_frame_resource_limit".to_owned(),
            "projection failed".to_owned(),
            Some(details),
            Some(request_id),
            None,
        );

        let Error::ActionTerminatingFrame {
            request_id: actual_request_id,
            boundaries: actual_boundaries,
            input_fully_applied,
            retry_safety,
            source,
        } = error
        else {
            panic!("expected typed action terminating-frame failure");
        };
        assert_eq!(actual_request_id, request_id);
        assert_eq!(*actual_boundaries, boundaries);
        assert!(input_fully_applied);
        assert_eq!(retry_safety, RetrySafety::SameRequestOnly);
        assert!(matches!(
            source,
            ScreenFrameError::ResourceLimit {
                limit: ScreenFrameLimitKind::ResponseBytes,
                requested: 2,
                maximum: 1,
                ..
            }
        ));
    }
}
