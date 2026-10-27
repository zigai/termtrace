//! Termination lifecycle of one recorded child as a pure state machine.
//!
//! The runtime loop reports facts as it learns them and ticks the machine once
//! per iteration; the machine answers with effects the loop executes in order.

use portable_pty::ExitStatus;
use std::{
    mem::replace,
    time::{Duration, Instant},
};
use termtrace_model::execution::session::ResourceLimitKind;

/// Output drain window after the root child exits or a resource limit terminates it.
pub(super) const DESCENDANT_GRACE_PERIOD: Duration = Duration::from_millis(250);
/// Ceiling for draining output after exit, and the window after a retained group is killed.
pub(super) const DESCENDANT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
/// Window after a forwarded termination signal before the process group is killed.
pub(super) const HOST_SIGNAL_GRACE_PERIOD: Duration = Duration::from_secs(2);
const RUNAWAY_OUTPUT_SLACK_BYTES: u64 = 8 * 1024;

/// Where the recorded child is on its way to a completed recording.
pub(super) struct Termination {
    stage: Stage,
    kill: Kill,
}

enum Stage {
    /// The root child has not been reaped.
    Running { output_closed: bool },
    /// The root child exited while descendants still hold the PTY open.
    Draining {
        status: ExitStatus,
        until: Instant,
        ceiling: Instant,
    },
    /// The child exited and output closed; an admitted input sequence may still be settling.
    Settling { status: ExitStatus },
    /// Control shutdown has begun.
    Closing { status: ExitStatus },
}

/// The process group is killed at most once; escalations are pending only before that.
enum Kill {
    Pending {
        resource: Option<ResourceEscalation>,
        signal: Option<Instant>,
    },
    Done,
}

#[derive(Clone, Copy)]
struct ResourceEscalation {
    kind: ResourceLimitKind,
    limit: u64,
    at: Instant,
}

/// Something the runtime learned between ticks.
pub(super) enum Fact {
    /// The PTY reader reported end of output.
    OutputClosed,
    /// The root child was reaped.
    ChildExited { status: ExitStatus, at: Instant },
    /// A resource limit was exceeded and the child was sent a termination signal.
    ResourceLimitExceeded {
        kind: ResourceLimitKind,
        limit: u64,
        at: Instant,
    },
    /// A host or agent termination signal was forwarded to the child process group.
    SignalForwarded { at: Instant },
}

/// Level observations sampled once per runtime-loop iteration.
#[derive(Clone, Copy)]
pub(super) struct Tick {
    pub(super) now: Instant,
    pub(super) received_output: bool,
    pub(super) output_bytes: u64,
    pub(super) input_sequence_pending: bool,
}

/// Work the runtime loop performs, in order, for one tick.
#[derive(Debug, Eq, PartialEq)]
pub(super) enum Effect {
    AppendHostError {
        code: &'static str,
        message: &'static str,
    },
    KillProcessGroup,
    Fail {
        operation: &'static str,
        reason: &'static str,
    },
    BeginControlShutdown,
    AwaitControlShutdown,
}

impl Termination {
    pub(super) const fn new() -> Self {
        return Self {
            stage: Stage::Running {
                output_closed: false,
            },
            kill: Kill::Pending {
                resource: None,
                signal: None,
            },
        };
    }

    pub(super) const fn child_exited(&self) -> bool {
        return !matches!(self.stage, Stage::Running { .. });
    }

    pub(super) const fn output_closed(&self) -> bool {
        return match self.stage {
            Stage::Running { output_closed } => output_closed,
            Stage::Draining { .. } => false,
            Stage::Settling { .. } | Stage::Closing { .. } => true,
        };
    }

    pub(super) const fn exit_status(&self) -> Option<&ExitStatus> {
        return match &self.stage {
            Stage::Running { .. } => None,
            Stage::Draining { status, .. }
            | Stage::Settling { status }
            | Stage::Closing { status } => Some(status),
        };
    }

    /// Deadline for force-killing the group after a forwarded signal, when still pending.
    pub(super) const fn signal_kill_deadline(&self) -> Option<Instant> {
        return match self.kill {
            Kill::Pending { signal, .. } => signal,
            Kill::Done => None,
        };
    }

    /// Earliest instant at which [`Self::step`] acts without a new fact or output.
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        let drain = match self.stage {
            Stage::Draining { until, .. } => Some(until),
            Stage::Running { .. } | Stage::Settling { .. } | Stage::Closing { .. } => None,
        };

        let (resource, signal) = match self.kill {
            Kill::Pending { resource, signal } => {
                (resource.map(|escalation| return escalation.at), signal)
            }
            Kill::Done => (None, None),
        };

        return [drain, resource, signal].into_iter().flatten().min();
    }

    pub(super) fn observe(&mut self, fact: Fact) {
        match fact {
            Fact::OutputClosed => {
                self.transition(|stage| {
                    return match stage {
                        Stage::Running { .. } => Stage::Running {
                            output_closed: true,
                        },
                        Stage::Draining { status, .. } => Stage::Settling { status },
                        settled @ (Stage::Settling { .. } | Stage::Closing { .. }) => settled,
                    };
                });
            }
            Fact::ChildExited { status, at } => {
                let Stage::Running { output_closed } = self.stage else {
                    return;
                };
                self.stage = if output_closed {
                    Stage::Settling { status }
                } else {
                    Stage::Draining {
                        status,
                        until: at + DESCENDANT_GRACE_PERIOD,
                        ceiling: at + DESCENDANT_DRAIN_TIMEOUT,
                    }
                };

                if let Kill::Pending { signal, .. } = &mut self.kill {
                    *signal = None;
                }
            }
            Fact::ResourceLimitExceeded { kind, limit, at } => {
                if let Kill::Pending { resource, .. } = &mut self.kill {
                    *resource = Some(ResourceEscalation {
                        kind,
                        limit,
                        at: at + DESCENDANT_GRACE_PERIOD,
                    });
                }
            }
            Fact::SignalForwarded { at } => {
                if let Kill::Pending { signal, .. } = &mut self.kill {
                    signal.get_or_insert(at + HOST_SIGNAL_GRACE_PERIOD);
                }
            }
        }
    }

    pub(super) fn step(&mut self, tick: Tick) -> Vec<Effect> {
        let mut effects = Vec::new();

        if tick.received_output
            && let Stage::Draining { until, ceiling, .. } = &mut self.stage
        {
            *until = (tick.now + DESCENDANT_GRACE_PERIOD).min(*ceiling);
        }

        if let Some((code, message)) = self.due_force_kill(tick) {
            effects.push(Effect::AppendHostError { code, message });
            effects.push(Effect::KillProcessGroup);
            self.kill = Kill::Done;
        }

        if let Stage::Draining { until, ceiling, .. } = &mut self.stage
            && tick.now >= *until
        {
            match self.kill {
                Kill::Done => {
                    effects.push(Effect::Fail {
                        operation: "drain PTY output",
                        reason: "PTY remained open after terminating retained process-group descendants",
                    });

                    return effects;
                }
                Kill::Pending { .. } => {
                    effects.push(Effect::KillProcessGroup);
                    self.kill = Kill::Done;
                    *until = tick.now + DESCENDANT_DRAIN_TIMEOUT;
                    *ceiling = *until;
                }
            }
        }

        if !tick.input_sequence_pending {
            self.transition(|stage| {
                let Stage::Settling { status } = stage else {
                    return stage;
                };

                effects.push(Effect::BeginControlShutdown);

                return Stage::Closing { status };
            });

            if let Stage::Closing { .. } = self.stage {
                effects.push(Effect::AwaitControlShutdown);
            }
        }

        return effects;
    }

    fn transition(&mut self, next: impl FnOnce(Stage) -> Stage) {
        let current = replace(
            &mut self.stage,
            Stage::Running {
                output_closed: false,
            },
        );
        self.stage = next(current);
    }

    fn due_force_kill(&self, tick: Tick) -> Option<(&'static str, &'static str)> {
        let Kill::Pending { resource, signal } = self.kill else {
            return None;
        };
        if let Some(escalation) = resource
            && (tick.now >= escalation.at || escalation.runaway(tick.output_bytes))
        {
            return Some(match escalation.kind {
                ResourceLimitKind::MaxDuration => (
                    "max_duration_force_kill",
                    "child did not exit after duration-limit termination; process group was killed",
                ),
                ResourceLimitKind::IdleTimeout => (
                    "idle_timeout_force_kill",
                    "child did not exit after idle-timeout termination; process group was killed",
                ),
                ResourceLimitKind::OutputBytes => (
                    "output_budget_force_kill",
                    "child did not exit after output-budget termination; process group was killed",
                ),
            });
        }

        if signal.is_some_and(|deadline| return tick.now >= deadline) {
            return Some((
                "recorder_signal_force_kill",
                "child did not exit after a signal delivered to the recorder; process group was killed",
            ));
        }
        return None;
    }
}

impl ResourceEscalation {
    /// Output kept flowing far past the budget, so the termination grace is skipped.
    fn runaway(self, output_bytes: u64) -> bool {
        return self.kind == ResourceLimitKind::OutputBytes
            && output_bytes
                >= self
                    .limit
                    .saturating_mul(2)
                    .max(self.limit.saturating_add(RUNAWAY_OUTPUT_SLACK_BYTES));
    }
}

#[cfg(test)]
mod tests;
