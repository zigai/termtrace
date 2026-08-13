//! Durable mutation replay and bounded in-memory retry caching.

use std::collections::{HashMap, VecDeque};

use uuid::Uuid;

use crate::{
    Result,
    control::{ControlCommand, ControlResponse},
    request_journal::{MutationJournal, MutationJournalRecord, guarded_command_hash},
};

use super::super::{input::input_sequence_operation_id, support::control_error_response};

const MAX_SESSION_MUTATIONS: u64 = 1_000_000;

pub(in crate::record) struct CachedMutation {
    command_hash: String,
    response: ControlResponse,
}

pub(in crate::record) struct MutationHistory {
    pub(in crate::record) journal: MutationJournal,
    pub(in crate::record) cached: HashMap<Uuid, CachedMutation>,
    order: VecDeque<Uuid>,
    capacity: usize,
}

impl MutationHistory {
    pub(in crate::record) fn new(journal: MutationJournal, capacity: usize) -> Self {
        return Self {
            journal,
            cached: HashMap::new(),
            order: VecDeque::new(),
            capacity,
        };
    }

    pub(in crate::record) fn lookup(
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
        return self.journal.len() >= MAX_SESSION_MUTATIONS || !self.journal.has_record_capacity();
    }

    pub(in crate::record) fn remember(
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
        self.cache_record(record);
        return Ok(());
    }

    fn cache_record(&mut self, record: MutationJournalRecord) {
        self.cache(
            record.request_id,
            CachedMutation {
                command_hash: record.command_hash,
                response: record.response,
            },
        );
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
