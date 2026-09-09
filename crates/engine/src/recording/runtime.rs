//! Runtime orchestration for one-shot PTY recording.

use super::{
    actor::SessionActor,
    control::apply_signal,
    input::{apply_input, apply_resize},
    model::{RuntimeMessage, TerminalGeometry},
    presentation::{process_semantic_bytes, process_semantic_output, resize_semantic},
    process::{terminate_process_group, RuntimeWorkers},
    support::terminal_error,
};
use crate::{Error, Result};
use portable_pty::ExitStatus;
use signal_hook::iterator::Signals;
use std::{
    io,
    mem::{replace, take},
    path::PathBuf,
    sync::mpsc::{self, RecvTimeoutError},
    time::{Duration, Instant},
};
use termtrace_model::execution::session::RecordingFailurePolicy;
use termtrace_model::{
    execution::event::{Event, InputDescriptor, InputKind, InputSource, LifecycleKind},
    execution::session::{ResourceLimitKind, ResourceLimitOutcome},
    protocol::control::ProcessSignal,
    terminal::state::SemanticState,
    value::ByteString,
};
use uuid::Uuid;

const DESCENDANT_GRACE_PERIOD: Duration = Duration::from_millis(250);
const DESCENDANT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
pub(in crate::recording) const HOST_SIGNAL_GRACE_PERIOD: Duration = Duration::from_secs(2);

pub(super) struct RuntimeOutcome {
    pub(super) status: ExitStatus,
    pub(super) output_bytes: u64,
    pub(super) resource_limit: Option<ResourceLimitOutcome>,
    pub(super) final_presented: SemanticState,
}

/// Record a command running in a real pseudo-terminal and mirror its output.
///
/// # Errors
///
/// Returns an error when configuration is invalid, secure artifacts cannot be
/// created, PTY setup or child execution fails, output cannot be mirrored, or
pub(super) fn consume_runtime(
    actor: &mut SessionActor<'_>,
    forward_stdin: bool,
    initial_geometry: TerminalGeometry,
    control_socket: &str,
    host_signals: Signals,
    startup_error: Option<Error>,
) -> Result<RuntimeOutcome> {
    let (sender, receiver) = mpsc::sync_channel::<RuntimeMessage>(64);
    let reader = replace(&mut actor.spawned.reader, Box::new(io::empty()));
    let started = RuntimeWorkers::start(
        reader,
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
            terminate_process_group(actor.spawned.pid);
            let _ = actor.spawned.child.kill();
            let _ = actor.spawned.child.wait();
            return Err(error);
        }
    };

    drop(sender);

    let result = (|| {
        if let Some(error) = startup_error {
            return Err(error);
        }

        let mut control_shutdown_started = false;
        let started_at = Instant::now();
        let mut last_activity_at = Instant::now();

        loop {
            let receive_timeout = actor.pending_input_sequence.as_ref().map_or(
                Duration::from_millis(10),
                |pending| {
                    return pending
                        .next_deadline()
                        .saturating_duration_since(Instant::now())
                        .min(Duration::from_millis(10));
                },
            );

            let mut received_output = false;
            match receiver.recv_timeout(receive_timeout) {
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
                Err(RecvTimeoutError::Disconnected) if !actor.progress.output_closed => {
                    return Err(terminal_error(
                        "receive runtime event",
                        "PTY reader disconnected before reporting closure",
                    ));
                }
                Err(RecvTimeoutError::Disconnected | RecvTimeoutError::Timeout) => {}
            }

            if actor.exit_status.is_none() {
                actor.exit_status = actor
                    .spawned
                    .child
                    .try_wait()
                    .map_err(|error| return terminal_error("poll child", error.to_string()))?;

                if actor.exit_status.is_some() && !actor.progress.output_closed {
                    actor.drain_deadline = Some(Instant::now() + DESCENDANT_GRACE_PERIOD);
                }
            }

            let child_exited = actor.exit_status.is_some();
            actor.advance_input_sequence(child_exited, actor.progress.output_closed)?;
            actor.event_log.flush_due()?;
            actor.evaluate_screen_waiters(child_exited, actor.progress.output_closed)?;
            actor.enforce_resource_limits(started_at, last_activity_at)?;

            if received_output && actor.exit_status.is_some() && !actor.progress.output_closed {
                actor.drain_deadline = Some(Instant::now() + DESCENDANT_GRACE_PERIOD);
            }

            if actor.exit_status.is_none()
                && actor
                    .resource_kill_deadline
                    .is_some_and(|deadline| return Instant::now() >= deadline)
                && !actor.process_group_killed
            {
                let (code, message) = match actor
                    .progress
                    .resource_limit
                    .as_ref()
                    .map(|o| return o.kind)
                {
                    Some(ResourceLimitKind::MaxDuration) => (
                        "max_duration_force_kill",
                        "child did not exit after duration-limit termination; process group was killed",
                    ),
                    Some(ResourceLimitKind::IdleTimeout) => (
                        "idle_timeout_force_kill",
                        "child did not exit after idle-timeout termination; process group was killed",
                    ),
                    _ => (
                        "output_budget_force_kill",
                        "child did not exit after output-budget termination; process group was killed",
                    ),
                };

                actor.event_log.append(Event::HostError {
                    code: code.to_owned(),
                    message: message.to_owned(),
                })?;

                terminate_process_group(actor.spawned.pid);
                actor.process_group_killed = true;
            }

            if actor.exit_status.is_none()
                && actor
                    .host_signal_kill_deadline
                    .is_some_and(|deadline| return Instant::now() >= deadline)
                && !actor.process_group_killed
            {
                actor.event_log.append(Event::HostError {
                    code: "recorder_signal_force_kill".to_owned(),
                    message: "child did not exit after a signal delivered to the recorder; process group was killed"
                        .to_owned(),
                })?;

                terminate_process_group(actor.spawned.pid);
                actor.process_group_killed = true;
            }

            if actor
                .drain_deadline
                .is_some_and(|deadline| return Instant::now() >= deadline)
                && !actor.progress.output_closed
            {
                if actor.process_group_killed {
                    return Err(terminal_error(
                        "drain PTY output",
                        "PTY remained open after terminating retained process-group descendants",
                    ));
                }

                terminate_process_group(actor.spawned.pid);
                actor.process_group_killed = true;
                actor.drain_deadline = Some(Instant::now() + DESCENDANT_DRAIN_TIMEOUT);
            }

            if actor.progress.output_closed
                && actor.exit_status.is_some()
                && actor.pending_input_sequence.is_none()
            {
                if !control_shutdown_started {
                    workers.begin_control_shutdown()?;

                    control_shutdown_started = true;
                }

                if workers.control_shutdown_finished() {
                    break;
                }
            }
        }

        actor.evaluate_screen_waiters(true, actor.progress.output_closed)?;
        actor.fail_screen_waiters(
            "child_completed",
            "child completed before screen predicate matched",
        );

        return actor
            .exit_status
            .clone()
            .ok_or_else(|| return terminal_error("wait for child", "missing exit status"));
    })();

    if let Err(error) = &result
        && actor.failure_policy == RecordingFailurePolicy::Preserve
    {
        super::degraded::preserve_terminal(actor, &receiver, error);
    } else if result.is_err() {
        terminate_process_group(actor.spawned.pid);
        let _ = actor.spawned.child.kill();
        let _ = actor.spawned.child.wait();
    }

    let status = workers.finish(receiver, result)?;

    return Ok(RuntimeOutcome {
        status,
        output_bytes: actor.progress.output_bytes,
        resource_limit: actor.progress.resource_limit.clone(),
        final_presented: actor.live.presented.clone(),
    });
}

impl SessionActor<'_> {
    fn forward_host_signal(
        &mut self,
        signal: ProcessSignal,
        code: &str,
        message: &str,
    ) -> Result<()> {
        self.host_signal_kill_deadline
            .get_or_insert_with(|| return Instant::now() + HOST_SIGNAL_GRACE_PERIOD);

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

        self.progress.resource_limit = Some(outcome);
        self.resource_kill_deadline = Some(Instant::now() + DESCENDANT_GRACE_PERIOD);
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

                let latest = self.live.semantic.snapshot()?;
                if !latest.modes.synchronized_output {
                    self.live.presented = latest;
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

                    let latest = self.live.semantic.snapshot()?;
                    if !latest.modes.synchronized_output {
                        self.live.presented = latest;
                    }
                }

                if self.live.suppress_mirrored_queries {
                    self.live
                        .mirror_output
                        .append(&mut self.live.mirror_sequence);
                    let mirrored = take(&mut self.live.mirror_output);
                    self.mirror_bytes(&mirrored)?;
                }

                self.progress.output_closed = true;
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
                apply_input(
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
                let latest = self.live.semantic.snapshot()?;
                if !latest.modes.synchronized_output {
                    self.live.presented = latest;
                }
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
                self.handle_control_request(request, response)?;
            }
            RuntimeMessage::ControlServerFailed(reason) => {
                return Err(terminal_error("serve control socket", reason));
            }
        }

        return Ok(());
    }
}
