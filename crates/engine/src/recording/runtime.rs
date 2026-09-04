//! Runtime orchestration for one-shot PTY recording.

use std::{
    io,
    mem::replace,
    path::PathBuf,
    sync::mpsc::{self, RecvTimeoutError},
    time::{Duration, Instant},
};

use portable_pty::ExitStatus;
use uuid::Uuid;

use termtrace_model::{
    execution::event::{Event, InputDescriptor, InputKind, InputSource, LifecycleKind},
    execution::session::{ResourceLimitKind, ResourceLimitOutcome},
    protocol::control::ProcessSignal,
    terminal::state::SemanticState,
    value::ByteString,
};

use super::{
    actor::SessionActor,
    control::apply_signal,
    input::{apply_input, apply_resize},
    model::{RuntimeMessage, TerminalGeometry},
    presentation::{process_semantic_bytes, process_semantic_output, resize_semantic},
    process::{RuntimeWorkers, terminate_process_group},
    support::terminal_error,
};
use crate::{Error, Result};

const DESCENDANT_GRACE_PERIOD: Duration = Duration::from_millis(250);
const DESCENDANT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const HOST_SIGNAL_GRACE_PERIOD: Duration = Duration::from_secs(2);

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
) -> Result<RuntimeOutcome> {
    let (sender, receiver) = mpsc::sync_channel::<RuntimeMessage>(64);
    let reader = replace(&mut actor.spawned.reader, Box::new(io::empty()));
    let workers = RuntimeWorkers::start(
        reader,
        &sender,
        forward_stdin,
        initial_geometry,
        control_socket,
    )?;
    drop(sender);

    let result = (|| {
        let mut control_shutdown_started = false;
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
                    received_output = matches!(&message, RuntimeMessage::Output(_));
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

            if actor.progress.resource_limit.is_none()
                && let Some(limit) = actor.resource_limits.output_bytes
                && actor.progress.output_bytes > limit
            {
                let outcome = ResourceLimitOutcome {
                    kind: ResourceLimitKind::OutputBytes,
                    limit,
                    observed: actor.progress.output_bytes,
                };
                actor.event_log.append(Event::HostError {
                    code: "output_budget_exceeded".to_owned(),
                    message: format!(
                        "output budget of {limit} bytes was exceeded at {} observed bytes; every observed byte was retained",
                        actor.progress.output_bytes
                    ),
                })?;
                apply_signal(
                    actor.spawned.pid,
                    &mut actor.event_log,
                    Uuid::now_v7(),
                    ProcessSignal::Terminate,
                    true,
                )?;
                actor.progress.resource_limit = Some(outcome);
                actor.resource_kill_deadline = Some(Instant::now() + DESCENDANT_GRACE_PERIOD);
            }

            if received_output && actor.exit_status.is_some() && !actor.progress.output_closed {
                actor.drain_deadline = Some(Instant::now() + DESCENDANT_GRACE_PERIOD);
            }
            if actor.exit_status.is_none()
                && actor
                    .resource_kill_deadline
                    .is_some_and(|deadline| return Instant::now() >= deadline)
                && !actor.process_group_killed
            {
                actor.event_log.append(Event::HostError {
                    code: "output_budget_force_kill".to_owned(),
                    message: "child did not exit after output-budget termination; process group was killed"
                        .to_owned(),
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

    if result.is_err() {
        terminate_process_group(actor.spawned.pid);
        let _ = actor.spawned.child.kill();
    }
    let status = workers.finish(result)?;

    return Ok(RuntimeOutcome {
        status,
        output_bytes: actor.progress.output_bytes,
        resource_limit: actor.progress.resource_limit.clone(),
        final_presented: actor.live.presented.clone(),
    });
}

impl SessionActor<'_> {
    fn process_runtime_message(&mut self, message: RuntimeMessage) -> Result<()> {
        match message {
            RuntimeMessage::Output(data) => {
                self.progress.output_bytes = self
                    .progress
                    .output_bytes
                    .saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
                self.event_log.append_output(Event::Output {
                    data: ByteString::from(data.as_slice()),
                })?;
                if self.progress.mirror_open
                    && let Err(source) = self
                        .output
                        .write_all(&data)
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
                process_semantic_output(
                    &mut self.live,
                    self.spawned.writer.as_mut(),
                    &mut self.event_log,
                    &data,
                )?;
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
                        &pending,
                    )?;
                    let latest = self.live.semantic.snapshot()?;
                    if !latest.modes.synchronized_output {
                        self.live.presented = latest;
                    }
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
                apply_input(
                    self.spawned.writer.as_mut(),
                    &mut self.event_log,
                    Uuid::now_v7(),
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
                apply_resize(
                    self.spawned.master.as_mut(),
                    &mut self.event_log,
                    Uuid::now_v7(),
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
                self.event_log.append(Event::HostError {
                    code: "recorder_signal_received".to_owned(),
                    message: format!(
                        "recorder received {signal:?}; forwarding it to the child process group"
                    ),
                })?;
                apply_signal(
                    self.spawned.pid,
                    &mut self.event_log,
                    Uuid::now_v7(),
                    signal,
                    true,
                )?;
                if self.host_signal_kill_deadline.is_none() {
                    self.host_signal_kill_deadline =
                        Some(Instant::now() + HOST_SIGNAL_GRACE_PERIOD);
                }
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
