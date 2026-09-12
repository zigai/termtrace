//! Preserve an interactive PTY after a recorder failure without claiming complete evidence.

use super::{actor::SessionActor, model::RuntimeMessage};
use crate::Error;
use nix::{
    sys::signal::{killpg, Signal},
    unistd::Pid,
};
use std::{
    io::Write as _,
    mem::take,
    sync::mpsc::{Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};
use termtrace_evidence::{artifact::write_secure, event_log::CanonicalBoundary};
use termtrace_model::protocol::control::{
    ControlResponse, ControlResult, ProcessSignal, CONTROL_PROTOCOL_VERSION,
};
use termtrace_model::{execution::event::Event, value::ByteString};

#[derive(serde::Serialize)]
struct RecordingHealth {
    version: u32,
    state: &'static str,
    complete_evidence: bool,
    degraded_started_monotonic_ns: u64,
    degraded_ended_monotonic_ns: Option<u64>,
    durable_boundary: CanonicalBoundary,
    observed_boundary: CanonicalBoundary,
    unrecorded_output_bytes: u64,
    unrecorded_input_bytes: u64,
    reason: &'static str,
}

/// Keep ownership of the terminal connection until the application actually exits.
/// No further agent mutations are admitted after the original recording error.
pub(super) fn preserve_terminal(
    actor: &mut SessionActor<'_>,
    receiver: &Receiver<RuntimeMessage>,
    error: &Error,
) {
    tracing::error!(%error, "recording degraded; preserving the interactive application");
    actor.fail_screen_waiters(
        "recording_degraded",
        "recording failed; interactive PTY forwarding continues",
    );
    let degraded_started_monotonic_ns = actor.event_log.elapsed_ns();
    let _boundary = actor.event_log.append(Event::HostError {
        code: "recording_degraded".to_owned(),
        message:
            "interactive forwarding continues; semantic control and input auditing are unavailable"
                .to_owned(),
    });
    let path = actor
        .event_log
        .path()
        .with_file_name("recording-health.json");

    let mut unrecorded_output_bytes =
        u64::try_from(actor.progress.unrecorded_output.len()).unwrap_or(u64::MAX);

    let mut unrecorded_input_bytes = 0_u64;
    let unrecorded = take(&mut actor.progress.unrecorded_output);
    let _mirror = actor
        .output
        .write_all(&unrecorded)
        .and_then(|()| return actor.output.flush());

    let mut health = RecordingHealth {
        version: 1,
        state: "degraded",
        complete_evidence: false,
        degraded_started_monotonic_ns,
        degraded_ended_monotonic_ns: None,
        durable_boundary: actor.event_log.durable_boundary(),
        observed_boundary: actor.event_log.observed_boundary(),
        unrecorded_output_bytes,
        unrecorded_input_bytes,
        reason: "recording degraded; output capture continues when storage remains writable, but input and semantic auditing are incomplete",
    };

    if let Ok(bytes) = serde_json::to_vec(&health) {
        let _health = write_secure(&path, &bytes);
    }

    let mut output_closed = actor.progress.output_closed;
    let mut exited_at = None;
    let mut signal_deadline = actor.host_signal_kill_deadline;

    loop {
        let message = match receiver.recv_timeout(Duration::from_millis(20)) {
            Ok(RuntimeMessage::TerminalInputClosed) => {
                Ok(RuntimeMessage::HostSignal(ProcessSignal::Hangup))
            }
            message => message,
        };
        match message {
            Ok(RuntimeMessage::Output { data, observed }) => {
                if actor
                    .event_log
                    .append_observed_output(
                        Event::Output {
                            data: ByteString::from(data.as_slice()),
                        },
                        Some(observed),
                    )
                    .is_err()
                {
                    unrecorded_output_bytes = unrecorded_output_bytes
                        .saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
                }

                let _mirror = actor
                    .output
                    .write_all(&data)
                    .and_then(|()| return actor.output.flush());
            }
            Ok(RuntimeMessage::Input(data)) => {
                unrecorded_input_bytes = unrecorded_input_bytes
                    .saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));

                let _input = actor
                    .spawned
                    .writer
                    .write_all(&data)
                    .and_then(|()| return actor.spawned.writer.flush());
            }
            Ok(RuntimeMessage::Resize(geometry)) => {
                let _resize = actor.spawned.master.resize(geometry.into());
            }
            Ok(RuntimeMessage::HostSignal(signal)) => {
                signal_deadline.get_or_insert_with(|| {
                    return Instant::now() + super::runtime::HOST_SIGNAL_GRACE_PERIOD;
                });

                // This is an external user/lifecycle signal, not recorder-error cleanup.
                if let Some(pid) = actor
                    .spawned
                    .pid
                    .and_then(|pid| return i32::try_from(pid).ok())
                {
                    let signal = match signal {
                        ProcessSignal::Interrupt => Signal::SIGINT,
                        ProcessSignal::Terminate => Signal::SIGTERM,
                        ProcessSignal::Hangup => Signal::SIGHUP,
                        ProcessSignal::User1 => Signal::SIGUSR1,
                        ProcessSignal::User2 => Signal::SIGUSR2,
                    };

                    let _signal = killpg(Pid::from_raw(pid), signal);
                }
            }
            Ok(RuntimeMessage::Control { request, response }) => {
                let _response = response.send(ControlResponse {
                    version: CONTROL_PROTOCOL_VERSION,
                    request_id: request.request_id,
                    result: ControlResult::Error {
                        code: "recording_degraded".to_owned(),
                        message: "canonical recording failed; use the attached terminal to control the preserved application".to_owned(),
                        details: None,
                    },
                });
            }
            Ok(RuntimeMessage::OutputClosed | RuntimeMessage::ReadFailed(_)) => {
                output_closed = true
            }
            Err(RecvTimeoutError::Disconnected) => output_closed = true,
            Ok(
                RuntimeMessage::InputFailed(_)
                | RuntimeMessage::ResizeFailed(_)
                | RuntimeMessage::ControlServerFailed(_)
                | RuntimeMessage::TerminalInputClosed,
            )
            | Err(RecvTimeoutError::Timeout) => {}
        }

        if actor.exit_status.is_none()
            && let Ok(status) = actor.spawned.child.try_wait()
        {
            actor.exit_status = status;
        }

        if actor.exit_status.is_none()
            && signal_deadline.is_some_and(|deadline| return Instant::now() >= deadline)
        {
            super::process::terminate_process_group(actor.spawned.pid);

            signal_deadline = None;
        }

        if actor.exit_status.is_some() {
            let exited = exited_at.get_or_insert_with(Instant::now);

            if output_closed || exited.elapsed() >= Duration::from_secs(2) {
                // Descendants retaining the PTY after the root exits follow normal lifecycle cleanup.
                if !output_closed {
                    super::process::terminate_process_group(actor.spawned.pid);
                }

                health.unrecorded_output_bytes = unrecorded_output_bytes;
                health.unrecorded_input_bytes = unrecorded_input_bytes;
                health.degraded_ended_monotonic_ns = Some(actor.event_log.elapsed_ns());
                health.durable_boundary = actor.event_log.durable_boundary();
                health.observed_boundary = actor.event_log.observed_boundary();

                if let Ok(bytes) = serde_json::to_vec(&health) {
                    let _health = write_secure(&path, &bytes);
                }

                return;
            }
        }
    }
}
