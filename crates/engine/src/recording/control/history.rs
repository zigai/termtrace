//! Durable mutation replay and bounded in-memory retry caching.

use std::collections::{HashMap, VecDeque};

use uuid::Uuid;

use termtrace_evidence::request_journal::{MutationJournal, guarded_command_hash};
use termtrace_model::protocol::control::{ControlCommand, ControlResponse};

use super::super::{
    input::input_sequence_operation_id,
    support::{control_error_response, control_rejected},
};
use crate::Result;

struct CachedMutation {
    command_hash: String,
    response: ControlResponse,
}

pub(in crate::recording) struct MutationHistory {
    journal: MutationJournal,
    cached: HashMap<Uuid, CachedMutation>,
    order: VecDeque<Uuid>,
    capacity: usize,
}

impl MutationHistory {
    pub(in crate::recording) fn new(journal: MutationJournal, capacity: usize) -> Self {
        return Self {
            journal,
            cached: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        };
    }

    pub(in crate::recording) fn lookup(
        &mut self,
        request_id: Uuid,
        command: &ControlCommand,
        expected_presented_state_hash: Option<&str>,
    ) -> Result<(String, Option<ControlResponse>)> {
        let requested_hash = guarded_command_hash(command, expected_presented_state_hash)?;
        if let Some(cached) = self.cached.get(&request_id) {
            let response = mutation_retry_response(request_id, &requested_hash, cached);
            return Ok((requested_hash, Some(response)));
        }
        let Some(record) = self.journal.lookup(request_id)? else {
            return Ok((requested_hash, None));
        };
        let cached = CachedMutation {
            command_hash: record.command_hash,
            response: record.response,
        };
        let response = mutation_retry_response(request_id, &requested_hash, &cached);
        self.cache(request_id, cached);
        return Ok((requested_hash, Some(response)));
    }

    pub(super) fn is_full(&self) -> bool {
        return !self.journal.has_record_capacity();
    }

    pub(in crate::recording) fn reserve_operation(&mut self, operation_id: Uuid) -> Result<()> {
        if !self.journal.reserve_operations(&[operation_id])? {
            return Err(control_rejected(
                "operation_id_collision",
                "operation identifier was already reserved by another mutation",
            ));
        }
        return Ok(());
    }

    pub(super) fn reserve_command(
        &mut self,
        request_id: Uuid,
        command: &ControlCommand,
    ) -> Result<()> {
        if let ControlCommand::InputSequence { atoms, .. } | ControlCommand::Act { atoms, .. } =
            command
        {
            let identifiers: Vec<_> = atoms.iter().map(input_sequence_operation_id).collect();
            if !self.journal.reserve_operations(&identifiers)? {
                return Err(control_rejected(
                    "operation_id_collision",
                    "input sequence operation identifier was already reserved by another mutation",
                ));
            }
            return Ok(());
        }
        return self.reserve_operation(request_id);
    }

    pub(in crate::recording) fn remember(
        &mut self,
        request_id: Uuid,
        command: &ControlCommand,
        command_hash: String,
        event_start_seq: u64,
        event_end_seq: u64,
        event_hash: String,
        response: ControlResponse,
    ) -> Result<()> {
        let operation_ids = if event_end_seq == event_start_seq {
            Vec::new()
        } else if let ControlCommand::InputSequence { atoms, .. }
        | ControlCommand::Act { atoms, .. } = command
        {
            let attempted = usize::try_from(event_end_seq.saturating_sub(event_start_seq) / 2)
                .unwrap_or(usize::MAX);
            atoms
                .iter()
                .take(attempted)
                .map(input_sequence_operation_id)
                .collect()
        } else {
            vec![request_id]
        };
        let record = self.journal.append(
            request_id,
            command_hash,
            operation_ids,
            event_start_seq,
            event_end_seq,
            event_hash,
            response,
        )?;
        self.cache(
            record.request_id,
            CachedMutation {
                command_hash: record.command_hash,
                response: record.response,
            },
        );
        return Ok(());
    }

    fn cache(&mut self, request_id: Uuid, value: CachedMutation) {
        if self.cached.insert(request_id, value).is_none() {
            self.order.push_back(request_id);
        }
        while self.cached.len() > self.capacity {
            let Some(evicted) = self.order.pop_front() else {
                break;
            };
            self.cached.remove(&evicted);
        }
    }
}

fn mutation_retry_response(
    request_id: Uuid,
    requested_hash: &str,
    cached: &CachedMutation,
) -> ControlResponse {
    if cached.command_hash == requested_hash {
        return cached.response.clone();
    }
    return control_error_response(
        request_id,
        "request_id_collision",
        "request identifier was already used for a different mutation",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use termtrace_model::{
        execution::event::InputKind, protocol::control::InputSequenceAtom, value::ByteString,
    };

    #[test]
    fn duplicate_operation_rejects_whole_batch_without_reserving_new_ids() {
        let temporary = tempfile::TempDir::new().expect("temporary directory");
        let journal = MutationJournal::create(temporary.path()).expect("journal");
        let mut history = MutationHistory::new(journal, 1);
        history
            .reserve_operation(Uuid::nil())
            .expect("initial reservation");
        let first = Uuid::from_u128(1);
        let second = Uuid::nil();
        let command = ControlCommand::InputSequence {
            atoms: [first, second]
                .into_iter()
                .map(|operation_id| {
                    return InputSequenceAtom::Bytes {
                        operation_id,
                        data: ByteString::new(vec![b'x']),
                        input_kind: InputKind::Text,
                        caller_intent: None,
                    };
                })
                .collect(),
            pace_ns: 0,
            include_atom_outcomes: false,
        };
        assert!(matches!(
            history.reserve_command(Uuid::now_v7(), &command),
            Err(crate::Error::ControlRejected { code, .. }) if code == "operation_id_collision"
        ));
        history
            .reserve_operation(first)
            .expect("rejected batch did not reserve its first identifier");
        assert!(matches!(
            history.reserve_operation(second),
            Err(crate::Error::ControlRejected { code, .. }) if code == "operation_id_collision"
        ));
        assert!(matches!(
            history.reserve_operation(Uuid::nil()),
            Err(crate::Error::ControlRejected { code, .. }) if code == "operation_id_collision"
        ));
    }
}
