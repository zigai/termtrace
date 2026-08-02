use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use uuid::Uuid;

use crate::{
    Result,
    event::{Event, EventRecord, OperationStatus},
};

use super::{
    types::{OperationKind, PendingOperation},
    validation::invalid_log,
};

pub(super) fn validate_operation_transition(
    path: &Path,
    line: u64,
    record: &EventRecord,
    seen: &mut HashSet<Uuid>,
    pending: &mut HashMap<Uuid, PendingOperation>,
) -> Result<()> {
    if let Some((operation_id, kind, expected_bytes)) = operation_intent(&record.event) {
        if !seen.insert(operation_id) {
            return Err(invalid_log(path, line, "operation identifier was reused"));
        }
        pending.insert(
            operation_id,
            PendingOperation {
                operation_id,
                kind,
                intent_seq: record.seq,
                expected_bytes,
            },
        );
        return Ok(());
    }

    let Some(result) = operation_result(&record.event) else {
        return Ok(());
    };
    let intent = pending
        .remove(&result.operation_id)
        .ok_or_else(|| return invalid_log(path, line, "operation result has no pending intent"))?;
    if intent.kind != result.kind {
        return Err(invalid_log(
            path,
            line,
            "operation result kind does not match its intent",
        ));
    }
    match result.status {
        OperationStatus::Applied if result.error_code.is_some() => {
            return Err(invalid_log(
                path,
                line,
                "applied operation result contains an error code",
            ));
        }
        OperationStatus::Failed if result.error_code.is_none() => {
            return Err(invalid_log(
                path,
                line,
                "failed operation result is missing an error code",
            ));
        }
        OperationStatus::Applied | OperationStatus::Failed => {}
    }
    if let (Some(expected), Some(written)) = (intent.expected_bytes, result.bytes_written)
        && (written > expected
            || (result.status == OperationStatus::Applied && written != expected))
    {
        return Err(invalid_log(
            path,
            line,
            "input result byte count is inconsistent with its intent",
        ));
    }
    return Ok(());
}

fn operation_intent(event: &Event) -> Option<(Uuid, OperationKind, Option<u64>)> {
    match event {
        Event::InputIntent {
            operation_id, data, ..
        } => {
            return Some((
                *operation_id,
                OperationKind::Input,
                Some(u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX)),
            ));
        }
        Event::ResizeIntent { operation_id, .. } => {
            return Some((*operation_id, OperationKind::Resize, None));
        }
        Event::SignalIntent { operation_id, .. } => {
            return Some((*operation_id, OperationKind::Signal, None));
        }
        _ => return None,
    }
}

struct OperationResultRef<'event> {
    operation_id: Uuid,
    kind: OperationKind,
    status: OperationStatus,
    bytes_written: Option<u64>,
    error_code: &'event Option<String>,
}

fn operation_result(event: &Event) -> Option<OperationResultRef<'_>> {
    match event {
        Event::InputResult {
            operation_id,
            status,
            bytes_written,
            error_code,
        } => {
            return Some(OperationResultRef {
                operation_id: *operation_id,
                kind: OperationKind::Input,
                status: *status,
                bytes_written: Some(*bytes_written),
                error_code,
            });
        }
        Event::ResizeResult {
            operation_id,
            status,
            error_code,
        } => {
            return Some(OperationResultRef {
                operation_id: *operation_id,
                kind: OperationKind::Resize,
                status: *status,
                bytes_written: None,
                error_code,
            });
        }
        Event::SignalResult {
            operation_id,
            status,
            error_code,
        } => {
            return Some(OperationResultRef {
                operation_id: *operation_id,
                kind: OperationKind::Signal,
                status: *status,
                bytes_written: None,
                error_code,
            });
        }
        _ => return None,
    }
}
