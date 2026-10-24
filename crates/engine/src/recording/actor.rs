//! Explicit single-owner state for one live recording session.

use super::{
    control::{MutationHistory, PendingInputSequence, ScreenWaiter},
    model::SpawnedCommand,
    presentation::LiveTerminal,
    termination::Termination,
};
use std::io::Write;
use termtrace_evidence::{event_log::EventLogWriter, request_journal::MutationJournal};
use termtrace_model::execution::session::{
    RecordingFailurePolicy, ResourceLimitOutcome, ResourceLimits,
};
use uuid::Uuid;

pub(super) struct RuntimeProgress {
    pub(super) mirror_open: bool,
    pub(super) output_bytes: u64,
    pub(super) resource_limit: Option<ResourceLimitOutcome>,
    pub(super) unrecorded_output: Vec<u8>,
}

/// Sole mutable owner of a recording process and its ordered actor state.
///
/// All PTY input, canonical-log mutation, live-terminal updates, waiter
/// evaluation, idempotency decisions, and deadline transitions happen through
/// this owner on the recording thread.
pub(super) struct SessionActor<'runtime> {
    pub(super) spawned: SpawnedCommand,
    pub(super) event_log: EventLogWriter,
    pub(super) session_id: Uuid,
    pub(super) output: &'runtime mut dyn Write,
    pub(super) continue_after_output_mirror_closes: bool,
    pub(super) live: LiveTerminal,
    pub(super) mutation_history: MutationHistory,
    pub(super) screen_waiters: Vec<ScreenWaiter>,
    pub(super) pending_input_sequence: Option<PendingInputSequence>,
    pub(super) progress: RuntimeProgress,
    pub(super) termination: Termination,
    pub(super) resource_limits: ResourceLimits,
    pub(super) failure_policy: RecordingFailurePolicy,
}

impl<'runtime> SessionActor<'runtime> {
    pub(super) fn new(
        spawned: SpawnedCommand,
        event_log: EventLogWriter,
        session_id: Uuid,
        output: &'runtime mut dyn Write,
        continue_after_output_mirror_closes: bool,
        live: LiveTerminal,
        journal: MutationJournal,
        mutation_cache_capacity: usize,
        resource_limits: ResourceLimits,
    ) -> Self {
        return Self {
            spawned,
            event_log,
            session_id,
            output,
            continue_after_output_mirror_closes,
            live,
            mutation_history: MutationHistory::new(journal, mutation_cache_capacity),
            screen_waiters: Vec::new(),
            pending_input_sequence: None,
            progress: RuntimeProgress {
                mirror_open: true,
                output_bytes: 0,
                resource_limit: None,
                unrecorded_output: Vec::new(),
            },
            termination: Termination::new(),
            resource_limits,
            failure_policy: RecordingFailurePolicy::Stop,
        };
    }
}
