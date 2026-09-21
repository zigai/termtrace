//! Durable mutation replay and bounded in-memory retry caching.

use super::super::{
    input::input_sequence_operation_id,
    support::{control_error_response, control_rejected},
};
use crate::Result;
use std::collections::{HashMap, VecDeque};
use termtrace_evidence::request_journal::{guarded_command_hash, MutationJournal};
use termtrace_model::protocol::control::{
    ControlCommand, ControlData, ControlResponse, ControlResult,
};
use uuid::Uuid;

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
            let attempted = match &response.result {
                ControlResult::Ok { data } => match data {
                    ControlData::InputSequenceApplied { atom_count, .. } => {
                        usize::try_from(*atom_count).unwrap_or(usize::MAX)
                    }
                    ControlData::ActionApplied { input, .. } => {
                        usize::try_from(input.atom_count).unwrap_or(usize::MAX)
                    }
                    _ => 0,
                },
                ControlResult::Error { details, .. } => {
                    let mut atoms_applied = 0;
                    if let Some(details) = details
                        && let Some(sequence) = &details.input_sequence
                    {
                        atoms_applied =
                            usize::try_from(sequence.atoms_applied).unwrap_or(usize::MAX);
                    }
                    atoms_applied
                }
            };

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
    use termtrace_evidence::request_journal::{MutationJournalCursor, MUTATION_JOURNAL_NAME};
    use termtrace_model::{
        execution::{event::InputKind, event_log::CanonicalBoundary},
        protocol::control::{
            ActionInputResult, ActionPostcondition, ActionPostconditionResult, ControlErrorDetails,
            InputSequenceAtom, InputSequenceFailureDetails, CONTROL_PROTOCOL_VERSION,
        },
        terminal::{observation::ActorBoundaries, screen::ScreenProjectionRequest},
        value::ByteString,
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

    fn dummy_boundaries() -> ActorBoundaries {
        let boundary = CanonicalBoundary {
            next_event_seq: 10,
            output_bytes: 0,
            event_hash: "a".repeat(64),
        };
        return ActorBoundaries {
            durable: boundary.clone(),
            observed: boundary,
        };
    }

    #[test]
    fn remember_input_sequence_applied_derives_exact_atom_count_from_response() {
        let temporary = tempfile::TempDir::new().expect("temporary directory");
        let journal = MutationJournal::create(temporary.path()).expect("journal");
        let mut history = MutationHistory::new(journal, 1);

        let op1 = Uuid::from_u128(1);
        let op2 = Uuid::from_u128(2);
        let op3 = Uuid::from_u128(3);
        let command = ControlCommand::InputSequence {
            atoms: [op1, op2, op3]
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
        let request_id = Uuid::now_v7();
        let response = ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id,
            result: ControlResult::Ok {
                data: ControlData::InputSequenceApplied {
                    sequence_id: request_id,
                    atom_count: 2,
                    bytes_written: 2,
                    first_intent_event_seq: 1,
                    last_result_event_seq: 4,
                    pace_ns: 0,
                    atoms: Vec::new(),
                    boundaries: dummy_boundaries(),
                },
            },
        };

        // Even if event_start_seq=0 and event_end_seq=10 (which would have been (10-0)/2 = 5 atoms under the old formula),
        // it must take exactly 2 atoms as reported by atom_count.
        history
            .remember(
                request_id,
                &command,
                "cmd_hash".to_owned(),
                0,
                10,
                "evt_hash".to_owned(),
                response,
            )
            .expect("remember succeeds");

        let mut cursor =
            MutationJournalCursor::open(&temporary.path().join(MUTATION_JOURNAL_NAME))
                .expect("cursor");
        let record = cursor
            .next_record()
            .expect("read")
            .expect("record must exist");
        assert_eq!(record.operation_ids, vec![op1, op2]);
    }

    #[test]
    fn remember_action_applied_derives_exact_atom_count_from_response() {
        let temporary = tempfile::TempDir::new().expect("temporary directory");
        let journal = MutationJournal::create(temporary.path()).expect("journal");
        let mut history = MutationHistory::new(journal, 1);

        let op1 = Uuid::from_u128(1);
        let op2 = Uuid::from_u128(2);
        let command = ControlCommand::Act {
            atoms: [op1, op2]
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
            postcondition: ActionPostcondition::None,
            projection: ScreenProjectionRequest::default(),
            pace_ns: 0,
            include_atom_outcomes: false,
        };
        let request_id = Uuid::now_v7();
        let response = ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id,
            result: ControlResult::Ok {
                data: ControlData::ActionApplied {
                    input: ActionInputResult {
                        sequence_id: request_id,
                        atom_count: 1,
                        bytes_written: 1,
                        first_intent_event_seq: 1,
                        last_result_event_seq: 2,
                        pace_ns: 0,
                        atoms: Vec::new(),
                    },
                    postcondition: ActionPostconditionResult::None {
                        matched: true,
                        state_hash: None,
                        frame: None,
                    },
                    boundaries: dummy_boundaries(),
                },
            },
        };

        history
            .remember(
                request_id,
                &command,
                "cmd_hash".to_owned(),
                0,
                8,
                "evt_hash".to_owned(),
                response,
            )
            .expect("remember succeeds");

        let mut cursor =
            MutationJournalCursor::open(&temporary.path().join(MUTATION_JOURNAL_NAME))
                .expect("cursor");
        let record = cursor
            .next_record()
            .expect("read")
            .expect("record must exist");
        assert_eq!(record.operation_ids, vec![op1]);
    }

    #[test]
    fn remember_error_derives_atoms_applied_from_input_sequence_details() {
        let temporary = tempfile::TempDir::new().expect("temporary directory");
        let journal = MutationJournal::create(temporary.path()).expect("journal");
        let mut history = MutationHistory::new(journal, 1);

        let op1 = Uuid::from_u128(1);
        let op2 = Uuid::from_u128(2);
        let command = ControlCommand::InputSequence {
            atoms: [op1, op2]
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
        let request_id = Uuid::now_v7();
        let response = ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id,
            result: ControlResult::Error {
                code: "input_sequence_failed".to_owned(),
                message: "write failed".to_owned(),
                details: Some(ControlErrorDetails {
                    current_presented_state_hash: "hash".to_owned(),
                    boundaries: dummy_boundaries(),
                    scope: "input".to_owned(),
                    input_sequence: Some(InputSequenceFailureDetails {
                        sequence_id: request_id,
                        atom_count: 2,
                        atoms_applied: 1,
                        failed_atom_index: 1,
                        failed_operation_id: op2,
                        bytes_written: 1,
                        failed_atom_bytes_written: 0,
                        first_intent_event_seq: 1,
                        last_result_event_seq: 2,
                        resume_from_atom_index: 1,
                    }),
                    screen_frame: None,
                }),
            },
        };

        history
            .remember(
                request_id,
                &command,
                "cmd_hash".to_owned(),
                0,
                4,
                "evt_hash".to_owned(),
                response,
            )
            .expect("remember succeeds");

        let mut cursor =
            MutationJournalCursor::open(&temporary.path().join(MUTATION_JOURNAL_NAME))
                .expect("cursor");
        let record = cursor
            .next_record()
            .expect("read")
            .expect("record must exist");
        assert_eq!(record.operation_ids, vec![op1]);
    }

    #[test]
    fn remember_error_defaults_to_zero_atoms_when_no_input_sequence_details() {
        let temporary = tempfile::TempDir::new().expect("temporary directory");
        let journal = MutationJournal::create(temporary.path()).expect("journal");
        let mut history = MutationHistory::new(journal, 1);

        let op1 = Uuid::from_u128(1);
        let op2 = Uuid::from_u128(2);
        let command = ControlCommand::InputSequence {
            atoms: [op1, op2]
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
        let request_id = Uuid::now_v7();
        let response = ControlResponse {
            version: CONTROL_PROTOCOL_VERSION,
            request_id,
            result: ControlResult::Error {
                code: "rejected".to_owned(),
                message: "rejected".to_owned(),
                details: None,
            },
        };

        history
            .remember(
                request_id,
                &command,
                "cmd_hash".to_owned(),
                0,
                4,
                "evt_hash".to_owned(),
                response,
            )
            .expect("remember succeeds");

        let mut cursor =
            MutationJournalCursor::open(&temporary.path().join(MUTATION_JOURNAL_NAME))
                .expect("cursor");
        let record = cursor
            .next_record()
            .expect("read")
            .expect("record must exist");
        assert!(record.operation_ids.is_empty());
    }
}
