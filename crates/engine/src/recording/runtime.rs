//! Runtime orchestration for one-shot PTY recording.

use super::{
    actor::SessionActor,
    control::{apply_signal, PendingInputSequence, ScreenWaiter},
    input::{apply, apply_resize},
    model::{RuntimeMessage, TerminalGeometry},
    outer_resize::{OuterResize, ResizeAuthority},
    presentation::{process_semantic_bytes, process_semantic_output, resize_semantic},
    process::RuntimeWorkers,
    support::{finish_with_cleanup, terminal_error},
    termination::{Effect, Fact, Tick},
};
use crate::{Error, Result};
use portable_pty::ExitStatus;
use signal_hook::iterator::Signals;
use std::{
    io,
    mem::{replace, take},
    path::PathBuf,
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};
use termtrace_model::execution::session::RecordingFailurePolicy;
use termtrace_model::{
    execution::event::{Event, InputDescriptor, InputKind, InputSource, LifecycleKind},
    execution::session::{ResourceLimitKind, ResourceLimitOutcome},
    protocol::control::ProcessSignal,
    value::ByteString,
};
use uuid::Uuid;

pub(super) struct RuntimeOutcome {
    pub(super) status: ExitStatus,
    pub(super) output_bytes: u64,
    pub(super) resource_limit: Option<ResourceLimitOutcome>,
}

/// Record a command running in a real pseudo-terminal and mirror its output.
///
/// # Errors
///
/// Returns an error when configuration is invalid, secure artifacts cannot be
/// created, PTY setup or child execution fails, output cannot be mirrored, or
pub(super) fn run(
    actor: &mut SessionActor<'_>,
    forward_stdin: bool,
    initial_geometry: TerminalGeometry,
    control_socket: &str,
    host_signals: Signals,
    startup_error: Option<Error>,
    resize_authority: &ResizeAuthority,
) -> Result<RuntimeOutcome> {
    let (sender, receiver) = mpsc::sync_channel::<RuntimeMessage>(64);
    let reader = replace(&mut actor.spawned.reader, Box::new(io::empty()));
    let master_fd = actor.spawned.master.as_raw_fd();
    actor.outer_resize = match OuterResize::start(resize_authority, &sender) {
        Ok(outer_resize) => outer_resize,
        Err(error) => {
            return finish_with_cleanup(Err(error), actor.spawned.reap_after_failure());
        }
    };
    let started = RuntimeWorkers::start(
        reader,
        master_fd,
        &sender,
        receiver,
        forward_stdin,
        initial_geometry,
        control_socket,
        host_signals,
    );
    let (workers, receiver) = match started {
        Ok(started) => started,
        Err(error) => {
            return finish_with_cleanup(Err(error), actor.spawned.reap_after_failure());
        }
    };

    drop(sender);

    let result = (|| {
        if let Some(error) = startup_error {
            return Err(error);
        }

        let started_at = Instant::now();
        let mut last_activity_at = Instant::now();

        let mut control_server_stopped = false;
        loop {
            let deadline = actor.next_deadline(started_at, last_activity_at);
            let received = deadline.map_or_else(
                || {
                    return receiver
                        .recv()
                        .map_err(|_disconnected| return RecvTimeoutError::Disconnected);
                },
                |deadline| {
                    return receiver
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()));
                },
            );

            let mut received_output = false;
            match received {
                Ok(RuntimeMessage::ControlServerStopped) => control_server_stopped = true,
                Ok(message) => {
                    received_output = matches!(&message, RuntimeMessage::Output { .. });

                    if matches!(
                        &message,
                        RuntimeMessage::Output { .. }
                            | RuntimeMessage::Input(_)
                            | RuntimeMessage::Control { .. }
                            | RuntimeMessage::Resize(_)
                    ) {
                        last_activity_at = Instant::now();
                    }

                    actor.process_runtime_message(message)?;
                }
                Err(RecvTimeoutError::Disconnected) if !actor.termination.output_closed() => {
                    return Err(terminal_error(
                        "receive runtime event",
                        "PTY reader disconnected before reporting closure",
                    ));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    // A closed inbox returns at once, so wait out the deadline here instead.
                    let Some(deadline) = deadline else {
                        return Err(terminal_error(
                            "receive runtime event",
                            "runtime inbox closed with no pending deadline",
                        ));
                    };
                    thread::sleep(deadline.saturating_duration_since(Instant::now()));
                }
                Err(RecvTimeoutError::Timeout) => {}
            }

            if !actor.termination.child_exited()
                && let Some(status) = actor
                    .spawned
                    .child
                    .try_wait()
                    .map_err(|error| return terminal_error("poll child", error.to_string()))?
            {
                actor.termination.observe(Fact::ChildExited {
                    status,
                    at: Instant::now(),
                });
            }

            let child_exited = actor.termination.child_exited();
            let output_closed = actor.termination.output_closed();
            actor.advance_input_sequence(child_exited, output_closed)?;
            actor.expire_outer_resize(false)?;
            actor.event_log.flush_due()?;
            actor.evaluate_screen_waiters(child_exited, output_closed)?;
            actor.enforce_resource_limits(started_at, last_activity_at)?;
            let effects = actor.termination.step(Tick {
                now: Instant::now(),
                received_output,
                output_bytes: actor.progress.output_bytes,
                input_sequence_pending: actor.pending_input_sequence.is_some(),
            });

            let mut control_shutdown_finished = false;
            for effect in effects {
                match effect {
                    Effect::AppendHostError { code, message } => {
                        actor.event_log.append(Event::HostError {
                            code: code.to_owned(),
                            message: message.to_owned(),
                        })?;
                    }
                    Effect::KillProcessGroup => actor.spawned.kill_process_group(),
                    Effect::Fail { operation, reason } => {
                        return Err(terminal_error(operation, reason));
                    }
                    Effect::BeginControlShutdown => workers.begin_control_shutdown()?,
                    Effect::AwaitControlShutdown => {
                        control_shutdown_finished = control_server_stopped;
                    }
                }
            }

            if control_shutdown_finished {
                break;
            }
        }

        actor.live.ensure_presented()?;
        actor.evaluate_screen_waiters(true, actor.termination.output_closed())?;
        actor.fail_screen_waiters(
            "child_completed",
            "child completed before screen predicate matched",
        );
        actor.expire_outer_resize(true)?;

        return actor
            .termination
            .exit_status()
            .cloned()
            .ok_or_else(|| return terminal_error("wait for child", "missing exit status"));
    })();
    let result = match result {
        Err(error) if actor.failure_policy == RecordingFailurePolicy::Preserve => {
            super::degraded::preserve_terminal(actor, &receiver, &error);

            Err(error)
        }
        Err(error) => finish_with_cleanup(Err(error), actor.spawned.reap_after_failure()),
        Ok(status) => Ok(status),
    };

    let status = workers.finish(receiver, result)?;

    actor.live.ensure_presented()?;

    return Ok(RuntimeOutcome {
        status,
        output_bytes: actor.progress.output_bytes,
        resource_limit: actor.progress.resource_limit.clone(),
    });
}

impl SessionActor<'_> {
    fn forward_host_signal(
        &mut self,
        signal: ProcessSignal,
        code: &str,
        message: &str,
    ) -> Result<()> {
        self.termination
            .observe(Fact::SignalForwarded { at: Instant::now() });

        self.event_log.append(Event::HostError {
            code: code.to_owned(),
            message: message.to_owned(),
        })?;

        let operation_id = Uuid::now_v7();
        self.mutation_history.reserve_operation(operation_id)?;

        return apply_signal(
            self.spawned.pid,
            &mut self.event_log,
            operation_id,
            signal,
            true,
        );
    }

    /// Earliest instant at which the runtime loop must act without a new message.
    fn next_deadline(&self, started_at: Instant, last_activity_at: Instant) -> Option<Instant> {
        let limits = self
            .progress
            .resource_limit
            .is_none()
            .then(|| {
                return [
                    self.resource_limits.max_duration_ms.and_then(|limit| {
                        return started_at.checked_add(Duration::from_millis(limit));
                    }),
                    self.resource_limits.idle_timeout_ms.and_then(|limit| {
                        return last_activity_at.checked_add(Duration::from_millis(limit));
                    }),
                ];
            })
            .into_iter()
            .flatten()
            .flatten();

        return [
            self.pending_input_sequence
                .as_ref()
                .map(PendingInputSequence::next_deadline),
            self.termination.next_deadline(),
            self.event_log.flush_deadline(),
            self.outer_resize_deadline(),
        ]
        .into_iter()
        .flatten()
        .chain(self.screen_waiters.iter().map(ScreenWaiter::next_deadline))
        .chain(limits)
        .min();
    }

    fn enforce_resource_limits(
        &mut self,
        started_at: Instant,
        last_activity_at: Instant,
    ) -> Result<()> {
        if self.progress.resource_limit.is_some() {
            return Ok(());
        }

        let detected = (|| {
            if let Some(limit) = self.resource_limits.output_bytes
                && self.progress.output_bytes > limit
            {
                return Some((
                    ResourceLimitOutcome {
                        kind: ResourceLimitKind::OutputBytes,
                        limit,
                        observed: self.progress.output_bytes,
                    },
                    "output_budget_exceeded",
                    format!(
                        "output budget of {limit} bytes was exceeded at {} observed bytes; every observed byte was retained",
                        self.progress.output_bytes
                    ),
                ));
            }

            if let Some(limit_ms) = self.resource_limits.max_duration_ms {
                let elapsed_ms =
                    u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);

                if elapsed_ms >= limit_ms {
                    return Some((
                        ResourceLimitOutcome {
                            kind: ResourceLimitKind::MaxDuration,
                            limit: limit_ms,
                            observed: elapsed_ms,
                        },
                        "max_duration_exceeded",
                        format!(
                            "session duration limit of {limit_ms}ms was exceeded at {elapsed_ms}ms; terminating process group"
                        ),
                    ));
                }
            }

            if let Some(limit_ms) = self.resource_limits.idle_timeout_ms {
                let idle_ms =
                    u64::try_from(last_activity_at.elapsed().as_millis()).unwrap_or(u64::MAX);

                if idle_ms >= limit_ms {
                    return Some((
                        ResourceLimitOutcome {
                            kind: ResourceLimitKind::IdleTimeout,
                            limit: limit_ms,
                            observed: idle_ms,
                        },
                        "idle_timeout_exceeded",
                        format!(
                            "idle timeout of {limit_ms}ms without activity was exceeded at {idle_ms}ms; terminating process group"
                        ),
                    ));
                }
            }

            return None;
        })();
        let Some((outcome, code, message)) = detected else {
            return Ok(());
        };
        self.event_log.append(Event::HostError {
            code: code.to_owned(),
            message,
        })?;

        let operation_id = Uuid::now_v7();
        self.mutation_history.reserve_operation(operation_id)?;
        apply_signal(
            self.spawned.pid,
            &mut self.event_log,
            operation_id,
            ProcessSignal::Terminate,
            true,
        )?;

        self.termination.observe(Fact::ResourceLimitExceeded {
            kind: outcome.kind,
            limit: outcome.limit,
            at: Instant::now(),
        });

        self.progress.resource_limit = Some(outcome);

        return Ok(());
    }

    fn mirror_bytes(&mut self, data: &[u8]) -> Result<()> {
        if self.progress.mirror_open
            && let Err(source) = self
                .output
                .write_all(data)
                .and_then(|()| return self.output.flush())
        {
            if !self.continue_after_output_mirror_closes {
                return Err(Error::Filesystem {
                    path: PathBuf::from("<stdout>"),
                    source,
                });
            }

            self.progress.mirror_open = false;
            self.event_log.append(Event::HostError {
                code: "output_mirror_closed".to_owned(),
                message: format!(
                    "presentation output closed while canonical PTY recording continued: {source}"
                ),
            })?;
        }

        return Ok(());
    }

    fn process_runtime_message(&mut self, message: RuntimeMessage) -> Result<()> {
        match message {
            RuntimeMessage::Output { data, observed } => {
                // Establish canonical causality before any fallible mirror, parser, or reply.
                let source_event_seq = match self.event_log.append_observed_output(
                    Event::Output {
                        data: ByteString::from(data.as_slice()),
                    },
                    Some(observed),
                ) {
                    Ok(sequence) => sequence,
                    Err(error) => {
                        self.progress.unrecorded_output = data;

                        return Err(error.into());
                    }
                };
                self.live.source_event_seq = Some(source_event_seq);
                self.progress.output_bytes = self
                    .progress
                    .output_bytes
                    .saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));

                process_semantic_output(
                    &mut self.live,
                    self.spawned.writer.as_mut(),
                    &mut self.event_log,
                    &mut self.mutation_history,
                    &data,
                )?;
                let mirrored = if self.live.suppress_mirrored_queries {
                    take(&mut self.live.mirror_output)
                } else {
                    data
                };
                self.mirror_bytes(&mirrored)?;
                if !self.screen_waiters.is_empty() || self.pending_input_sequence.is_some() {
                    self.live.ensure_presented()?;
                }
            }
            RuntimeMessage::OutputClosed => {
                let pending = self.live.presentation_tracker.finish();
                if !pending.is_empty() {
                    process_semantic_bytes(
                        &mut self.live,
                        self.spawned.writer.as_mut(),
                        &mut self.event_log,
                        &mut self.mutation_history,
                        &pending,
                    )?;
                }

                self.live.ensure_presented()?;

                if self.live.suppress_mirrored_queries {
                    self.live
                        .mirror_output
                        .append(&mut self.live.mirror_sequence);
                    let mirrored = take(&mut self.live.mirror_output);
                    self.mirror_bytes(&mirrored)?;
                }

                self.termination.observe(Fact::OutputClosed);
                self.event_log.append(Event::Lifecycle {
                    lifecycle_kind: LifecycleKind::OutputClosed,
                    pid: self.spawned.pid,
                })?;
            }
            RuntimeMessage::ReadFailed(reason) => {
                return Err(terminal_error("read PTY output", reason));
            }
            RuntimeMessage::Input(data) => {
                let operation_id = Uuid::now_v7();
                self.mutation_history.reserve_operation(operation_id)?;
                apply(
                    self.spawned.writer.as_mut(),
                    &mut self.event_log,
                    operation_id,
                    InputSource::Human,
                    InputKind::Raw,
                    InputDescriptor::Raw,
                    None,
                    Some("raw".to_owned()),
                    None,
                    &data,
                )?;
            }
            RuntimeMessage::InputFailed(reason) => {
                return Err(terminal_error("read standard input", reason));
            }
            RuntimeMessage::Resize(geometry) if self.owns_outer_resize() => {
                self.observe_outer_grid(geometry)?;
            }
            RuntimeMessage::OuterResizeReply {
                request_id,
                dispatch,
            } => {
                self.handle_outer_resize_reply(request_id, dispatch)?;
            }
            RuntimeMessage::Resize(geometry) => {
                let operation_id = Uuid::now_v7();
                self.mutation_history.reserve_operation(operation_id)?;
                apply_resize(
                    self.spawned.master.as_mut(),
                    &mut self.event_log,
                    operation_id,
                    geometry,
                )?;

                resize_semantic(&mut self.live.semantic, geometry)?;
                self.live.geometry = geometry;
                self.live.presented_stale = true;
            }
            RuntimeMessage::ResizeFailed(reason) => {
                return Err(terminal_error("read terminal geometry", reason));
            }
            RuntimeMessage::HostSignal(signal) => {
                self.forward_host_signal(
                    signal,
                    "recorder_signal_received",
                    &format!(
                        "recorder received {signal:?}; forwarding it to the child process group"
                    ),
                )?;
            }
            RuntimeMessage::TerminalInputClosed => {
                self.forward_host_signal(
                    ProcessSignal::Hangup,
                    "terminal_input_closed",
                    "attached terminal input closed; sending hangup to the child process group",
                )?;
            }
            RuntimeMessage::Control { request, response } => {
                self.live.ensure_presented()?;
                self.handle_control_request(request, response)?;
            }
            RuntimeMessage::ControlServerFailed(reason) => {
                return Err(terminal_error("serve control socket", reason));
            }
            RuntimeMessage::ChildStateChanged | RuntimeMessage::ControlServerStopped => {}
        }

        return Ok(());
    }
}
