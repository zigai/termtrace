use super::{method, millis, Registry};
use crate::failure::Failure;
use crate::server::Call;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::sync::{Mutex, MutexGuard};
use termtrace_model::value::ExactPath;
use termtrace_sdk::contracts::control::parse_request_id;
use termtrace_sdk::contracts::OriginalLaunchOptions;
use termtrace_sdk::defaults;
use termtrace_sdk::input::MutationGuard;
use termtrace_sdk::locator::WaitCancellation;
use termtrace_sdk::session::{
    original_capture_status, OriginalCaptureStatus, OriginalRecording as OriginalOutcome,
    OriginalSession, StopRequest,
};
use termtrace_sdk::Uuid;

const FINISHED: &str = "original recording has already finished or been detached";

/// An original recording this client owns; the session is absent while a lifecycle call holds it
/// and after it finishes or detaches.
pub(crate) struct Original {
    session: Mutex<Option<OriginalSession>>,
    path: ExactPath,
}

impl Original {
    fn new(session: OriginalSession) -> Self {
        return Self {
            path: ExactPath(session.session().path().to_path_buf()),
            session: Mutex::new(Some(session)),
        };
    }

    fn lock(&self) -> MutexGuard<'_, Option<OriginalSession>> {
        return self
            .session
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner());
    }

    fn take(&self) -> Result<OriginalSession, Failure> {
        return self
            .lock()
            .take()
            .ok_or_else(|| return Failure::plain(FINISHED.to_owned()));
    }

    fn restore(&self, session: OriginalSession) {
        *self.lock() = Some(session);
    }

    /// A capture that timed out or whose stop failed keeps its owner for another wait or a
    /// force-close; a capture failure leaves nothing to own.
    fn settle(
        &self,
        session: OriginalSession,
        result: termtrace_sdk::Result<OriginalOutcome>,
        request_id: Option<Uuid>,
    ) -> Result<OriginalOutcome, Failure> {
        return result.map_err(|error| {
            if !matches!(error, termtrace_sdk::Error::OriginalCaptureFailed { .. }) {
                self.restore(session);
            }

            let lifecycle = matches!(
                error,
                termtrace_sdk::Error::OriginalCaptureFailed { .. }
                    | termtrace_sdk::Error::OriginalFinalizationTimeout { .. }
            );

            return match request_id {
                Some(request_id) if !lifecycle => Failure::mutation(&error, request_id),
                _ => Failure::sdk(&error),
            };
        });
    }
}

/// An original recording this client owns.
#[derive(Serialize, JsonSchema)]
struct OriginalInfo {
    /// Handle that addresses the recording in `original.*` calls.
    original: String,
    /// Canonical session identity.
    session_id: String,
    /// Session directory.
    path: ExactPath,
}

/// Params of `termtrace.record_original`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RecordOriginalParams {
    /// Program and arguments.
    command: Vec<String>,
    /// Grid or viewport, canvas, frame rate, environment, Ghostty settings, and limits.
    #[serde(default)]
    options: OriginalLaunchOptions,
}

/// Params of `termtrace.attach_original`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AttachOriginalParams {
    /// Session directory of the original recording.
    path: ExactPath,
}

/// Params of calls on one original recording.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OriginalParams {
    /// Original recording handle.
    original: String,
}

/// Params of `original.finish`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FinishParams {
    /// Original recording handle.
    original: String,
    /// Bound on finalization; the default stop timeout when absent.
    #[serde(default)]
    timeout_ms: Option<u64>,
    /// Identity of the stop request; allocated when absent.
    #[serde(default)]
    request_id: Option<String>,
}

/// Params of `original.wait_finished`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WaitFinishedParams {
    /// Original recording handle.
    original: String,
    /// Bound on waiting; the default stop timeout when absent.
    #[serde(default)]
    timeout_ms: Option<u64>,
}

pub(super) fn register(registry: &mut Registry) {
    method(
        registry,
        "termtrace.record_original",
        "Launch a command as an original recording captured from its first byte.",
        record_original,
    );
    method(
        registry,
        "termtrace.attach_original",
        "Own an existing original recording without its capture supervisor.",
        attach_original,
    );
    method(
        registry,
        "original.is_active",
        "Report whether this client still owns the original recording.",
        |call, params: OriginalParams| -> Result<bool, Failure> {
            return Ok(call
                .server
                .originals
                .get(&params.original)?
                .lock()
                .is_some());
        },
    );
    method(
        registry,
        "original.capture_status",
        "Report the native capture status.",
        capture_status,
    );
    method(
        registry,
        "original.finish",
        "Stop the program and finalize the capture.",
        finish,
    );
    method(
        registry,
        "original.wait_finished",
        "Wait until the program exits and the capture is finalized.",
        wait_finished,
    );
    method(
        registry,
        "original.force_close",
        "Terminate the program and the capture without finalizing.",
        force_close,
    );
    method(
        registry,
        "original.detach",
        "Leave the original recording running and give up ownership.",
        |call, params: OriginalParams| -> Result<(), Failure> {
            let original = call.server.originals.get(&params.original)?;
            let _path = original.take()?.detach();

            return Ok(());
        },
    );
    method(
        registry,
        "original.close",
        "Release the handle.",
        |call, params: OriginalParams| return call.server.originals.remove(&params.original),
    );
}

fn sdk<T>(result: termtrace_sdk::Result<T>) -> Result<T, Failure> {
    return result.map_err(|error| return Failure::sdk(&error));
}

fn own(call: &Call<'_>, session: OriginalSession) -> OriginalInfo {
    let session_id = session.session().session_id().to_string();
    let original = Original::new(session);
    let path = original.path.clone();

    return OriginalInfo {
        original: call.server.originals.insert(original),
        session_id,
        path,
    };
}

fn record_original(call: &Call<'_>, params: RecordOriginalParams) -> Result<OriginalInfo, Failure> {
    let terminal = call.terminal()?;
    let command = params.command.into_iter().map(OsString::from).collect();
    let spec = sdk(params.options.to_spec(command))?;
    let session = sdk(WaitCancellation::forward(
        Some(&call.cancellation),
        |cancelled| return terminal.start_original(&spec, cancelled),
    ))?;

    if call.cancellation.is_cancelled() {
        return Err(Failure::sdk(&session.cancel()));
    }
    return Ok(own(call, session));
}

fn attach_original(call: &Call<'_>, params: AttachOriginalParams) -> Result<OriginalInfo, Failure> {
    let session = sdk(call.terminal()?.attach_original(&params.path.0))?;

    return Ok(own(call, session));
}

fn capture_status(
    call: &Call<'_>,
    params: OriginalParams,
) -> Result<OriginalCaptureStatus, Failure> {
    let original = call.server.originals.get(&params.original)?;
    return sdk(original_capture_status(&original.path.0));
}

fn finish(call: &Call<'_>, params: FinishParams) -> Result<OriginalOutcome, Failure> {
    let original = call.server.originals.get(&params.original)?;
    let request_id = params
        .request_id
        .as_deref()
        .map(parse_request_id)
        .transpose()
        .map_err(|error| return Failure::invalid_params(error.to_string()))?
        .unwrap_or_else(Uuid::now_v7);

    let mut session = original.take()?;
    let request = StopRequest {
        guard: MutationGuard {
            request_id: Some(request_id),
            if_state: None,
        },
    };
    let result = session.finish(request, millis(params.timeout_ms, defaults::STOP_TIMEOUT));

    return original.settle(session, result, Some(request_id));
}

fn wait_finished(call: &Call<'_>, params: WaitFinishedParams) -> Result<OriginalOutcome, Failure> {
    let original = call.server.originals.get(&params.original)?;
    let mut session = original.take()?;
    let result = session.wait_finished(millis(params.timeout_ms, defaults::STOP_TIMEOUT));

    return original.settle(session, result, None);
}

fn force_close(call: &Call<'_>, params: OriginalParams) -> Result<(), Failure> {
    let original = call.server.originals.get(&params.original)?;
    let Ok(mut session) = original.take() else {
        return Ok(());
    };
    if let Err(error) = session.force_close() {
        original.restore(session);

        return Err(Failure::sdk(&error));
    }
    return Ok(());
}
