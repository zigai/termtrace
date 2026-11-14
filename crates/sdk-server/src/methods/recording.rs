use super::{method, Registry};
use crate::failure::Failure;
use crate::server::Call;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;
use termtrace_model::execution::session::SessionCompletion;
use termtrace_sdk::client::Session;
use termtrace_sdk::contracts::control::parse_request_id;
use termtrace_sdk::contracts::StopReport;
use termtrace_sdk::defaults;
use termtrace_sdk::input::MutationGuard;
use termtrace_sdk::session::{StopOutcome, StopRequest};
use termtrace_sdk::Uuid;

const STOPPED: &str = "recording has already been stopped or detached";

/// A recording this client started; the owned session is absent while a lifecycle call holds it
/// and after the recording stops or detaches.
pub(crate) struct Recording {
    session: Mutex<Option<Session>>,
    lifecycle: Session,
}

impl Recording {
    pub(crate) fn new(session: Session) -> Self {
        let lifecycle = session.lifecycle_observer();

        return Self {
            session: Mutex::new(Some(session)),
            lifecycle,
        };
    }

    fn lock(&self) -> MutexGuard<'_, Option<Session>> {
        return self
            .session
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner());
    }

    pub(crate) fn is_active(&self) -> bool {
        return self
            .lock()
            .as_ref()
            .is_some_and(|session| return !session.lifecycle_finished())
            || (self.lifecycle.lifecycle_force_requested()
                && !self.lifecycle.lifecycle_finished());
    }

    pub(crate) fn ensure_active(&self) -> Result<(), Failure> {
        if self.is_active() {
            return Ok(());
        }
        return Err(Failure::plain(STOPPED.to_owned()));
    }

    fn take(&self) -> Option<Session> {
        return self.lock().take();
    }

    fn restore(&self, session: Session) {
        if session.lifecycle_finished() || session.lifecycle_force_requested() {
            return;
        }

        *self.lock() = Some(session);
    }

    fn stop(&self, request_id: Uuid, wait: Option<Duration>) -> Result<StopOutcome, Failure> {
        let mut session = self
            .take()
            .ok_or_else(|| return Failure::plain(STOPPED.to_owned()))?;

        let request = StopRequest {
            guard: MutationGuard {
                request_id: Some(request_id),
                if_state: None,
            },
        };

        return match session.stop(request, wait) {
            Ok(completed @ StopOutcome::Completed { .. }) => Ok(completed),
            Ok(requested @ StopOutcome::Requested { .. }) => {
                self.restore(session);

                Ok(requested)
            }
            Err(error) => {
                self.restore(session);

                Err(Failure::mutation(&error, request_id))
            }
        };
    }
}

/// Params of a call on one recording.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RecordingParams {
    /// Recording handle.
    recording: String,
}

/// Params of `recording.stop` and `recording.wait_for_completion`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TimedRecordingParams {
    /// Recording handle.
    recording: String,
    /// Bound on waiting for validated completion; the default stop timeout when absent.
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// Params of `recording.request_stop`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RequestStopParams {
    /// Recording handle.
    recording: String,
    /// Identity of this stop, to retry it safely; allocated when absent.
    #[serde(default)]
    request_id: Option<String>,
    /// Wait this long for validated completion; without it, return once the stop is accepted.
    #[serde(default)]
    wait_ms: Option<u64>,
}

pub(super) fn register(registry: &mut Registry) {
    method(
        registry,
        "recording.is_active",
        "Report whether the recording is still running and owned by this client.",
        is_active,
    );
    method(
        registry,
        "recording.stop",
        "Stop the recording and wait for validated completion.",
        stop,
    );
    method(
        registry,
        "recording.request_stop",
        "Ask the recording to end; with `wait_ms`, wait that long for validated completion.",
        request_stop,
    );
    method(
        registry,
        "recording.wait_for_completion",
        "Wait for validated completion evidence and reap the recorder.",
        wait_for_completion,
    );
    method(
        registry,
        "recording.discard_completed",
        "Delete this completed session, reaping an owned recorder.",
        discard_completed,
    );
    method(
        registry,
        "recording.force_close",
        "Terminate and reap the recorded process group and the owned recorder.",
        force_close,
    );
    method(
        registry,
        "recording.detach",
        "Release ownership so the recording keeps running in the background.",
        detach,
    );
    method(
        registry,
        "recording.close",
        "Release the handle; an active recording keeps running as after `recording.detach`.",
        close,
    );
}

fn is_active(call: &Call<'_>, params: RecordingParams) -> Result<bool, Failure> {
    return Ok(call.server.recordings.get(&params.recording)?.is_active());
}

fn stop(call: &Call<'_>, params: TimedRecordingParams) -> Result<(), Failure> {
    let recording = call.server.recordings.get(&params.recording)?;
    let timeout = super::millis(params.timeout_ms, defaults::STOP_TIMEOUT);
    let _outcome = recording.stop(Uuid::now_v7(), Some(timeout))?;

    return Ok(());
}

fn request_stop(call: &Call<'_>, params: RequestStopParams) -> Result<StopReport, Failure> {
    let recording = call.server.recordings.get(&params.recording)?;
    let request_id = params
        .request_id
        .as_deref()
        .map(parse_request_id)
        .transpose()
        .map_err(|error| return Failure::sdk(&error))?
        .unwrap_or_else(Uuid::now_v7);

    let wait = params.wait_ms.map(Duration::from_millis);
    let outcome = recording.stop(request_id, wait)?;

    return Ok(StopReport {
        request_id,
        outcome,
    });
}

fn wait_for_completion(
    call: &Call<'_>,
    params: TimedRecordingParams,
) -> Result<SessionCompletion, Failure> {
    let recording = call.server.recordings.get(&params.recording)?;
    let timeout = super::millis(params.timeout_ms, defaults::STOP_TIMEOUT);
    let mut session = recording
        .take()
        .ok_or_else(|| return Failure::plain(STOPPED.to_owned()))?;

    return session.complete(timeout).map_err(|error| {
        let failure = Failure::sdk(&error);

        recording.restore(session);
        return failure;
    });
}

fn discard_completed(call: &Call<'_>, params: RecordingParams) -> Result<(), Failure> {
    let recording = call.server.recordings.get(&params.recording)?;
    let owned = recording.take();
    let attached = owned.is_some();
    let mut session = match owned {
        Some(session) => session,
        None => call
            .terminal()?
            .attach(recording.lifecycle.path())
            .map_err(|error| return Failure::sdk(&error))?,
    };

    return session.discard_completed().map_err(|error| {
        let failure = Failure::sdk(&error);

        if attached {
            recording.restore(session);
        }

        return failure;
    });
}

fn force_close(call: &Call<'_>, params: RecordingParams) -> Result<(), Failure> {
    let recording = call.server.recordings.get(&params.recording)?;
    let mut session = match recording.take() {
        Some(session) => session,
        None if recording.lifecycle.lifecycle_force_requested() => {
            recording.lifecycle.lifecycle_observer()
        }
        None => return Ok(()),
    };

    return session.force_close().map_err(|error| {
        let failure = Failure::sdk(&error);

        recording.restore(session);
        return failure;
    });
}

fn detach(call: &Call<'_>, params: RecordingParams) -> Result<(), Failure> {
    let session = call
        .server
        .recordings
        .get(&params.recording)?
        .take()
        .ok_or_else(|| return Failure::plain(STOPPED.to_owned()))?;
    let _path = session.detach();
    return Ok(());
}

fn close(call: &Call<'_>, params: RecordingParams) -> Result<(), Failure> {
    return call.server.recordings.remove(&params.recording);
}
