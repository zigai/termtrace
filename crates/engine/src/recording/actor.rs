//! Explicit single-owner state for one live recording session.

use std::{io::Write, time::Instant};

use portable_pty::ExitStatus;
use termtrace_evidence::{event_log::EventLogWriter, request_journal::MutationJournal};
use termtrace_model::execution::session::{ResourceLimitOutcome, ResourceLimits};
use uuid::Uuid;

use super::{
    control::{MutationHistory, PendingInputSequence, ScreenWaiter},
    model::SpawnedCommand,
    presentation::LiveTerminal,
};

pub(super) struct RuntimeProgress {
    pub(super) output_closed: bool,
    pub(super) mirror_open: bool,
    pub(super) output_bytes: u64,
    pub(super) resource_limit: Option<ResourceLimitOutcome>,
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
    pub(super) exit_status: Option<ExitStatus>,
    pub(super) drain_deadline: Option<Instant>,
    pub(super) resource_kill_deadline: Option<Instant>,
    pub(super) host_signal_kill_deadline: Option<Instant>,
    pub(super) process_group_killed: bool,
    pub(super) resource_limits: ResourceLimits,
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
                output_closed: false,
                mirror_open: true,
                output_bytes: 0,
                resource_limit: None,
            },
            exit_status: None,
            drain_deadline: None,
            resource_kill_deadline: None,
            host_signal_kill_deadline: None,
            process_group_killed: false,
            resource_limits,
        };
    }

    pub(super) fn into_process_and_log(self) -> (SpawnedCommand, EventLogWriter) {
        return (self.spawned, self.event_log);
    }
}
