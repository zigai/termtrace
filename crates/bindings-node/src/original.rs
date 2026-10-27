//! Original-execution recordings for Node.js.

use crate::host::{self, Arg, Model};
use crate::locator::JsCancelToken;
use crate::{actions::mutation_err, blocking_failure, to_napi_err, JsSession};
use napi::bindgen_prelude::{Either, Uint8Array};
use napi_derive::napi;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use termtrace_sdk::client::Terminal;
use termtrace_sdk::contracts::OriginalLaunchOptions;
use termtrace_sdk::defaults;
use termtrace_sdk::input::MutationGuard;
use termtrace_sdk::locator::WaitCancellation;
use termtrace_sdk::session::{
    OriginalLaunchSpec, OriginalRecording as OriginalRecordingModel, OriginalSession,
    SessionTarget, StopRequest,
};
use termtrace_sdk::Uuid;

pub(crate) type PathInput = Either<String, Uint8Array>;

fn input_bytes(input: &PathInput) -> &[u8] {
    return match input {
        Either::A(text) => text.as_bytes(),
        Either::B(bytes) => bytes,
    };
}

/// Strings and bytes both follow the `path:`/`name:` reference rules.
pub(crate) fn session_target(target: &PathInput) -> napi::Result<SessionTarget> {
    return SessionTarget::parse(OsStr::from_bytes(input_bytes(target))).map_err(to_napi_err);
}

pub(crate) fn state_root(root: Option<&PathInput>) -> Option<PathBuf> {
    return root.map(|root| return PathBuf::from(OsStr::from_bytes(input_bytes(root))));
}

pub(crate) fn original_launch_spec(
    command: Vec<String>,
    options: Option<Arg<OriginalLaunchOptions>>,
) -> napi::Result<OriginalLaunchSpec> {
    let options = options.map(|v| v.0).unwrap_or_default();

    return options
        .to_spec(command.into_iter().map(OsString::from).collect())
        .map_err(to_napi_err);
}

// Mirrors `cancel` into the start's flag until it returns; a cancellation the bridge had not
// forwarded yet still owns the late recording.
pub(crate) async fn start_original(
    terminal: Terminal,
    spec: OriginalLaunchSpec,
    cancel: Option<JsCancelToken>,
) -> napi::Result<JsOriginalRecording> {
    return tokio::task::spawn_blocking(move || {
        let token = cancel.map(|token| return token.inner);
        let original = WaitCancellation::forward(token.as_ref(), |cancelled| {
            return terminal.start_original(&spec, cancelled);
        })
        .map_err(to_napi_err)?;

        if token.is_some_and(|token| return token.is_cancelled()) {
            return Err(to_napi_err(original.cancel()));
        }
        return Ok(JsOriginalRecording::new(terminal, original));
    })
    .await
    .map_err(blocking_failure)?;
}

/// An original recording: a canonical session whose terminal pixels are captured while it runs.
#[napi(js_name = "OriginalRecording")]
#[derive(Clone)]
pub struct JsOriginalRecording {
    terminal: Terminal,
    original: Arc<Mutex<Option<OriginalSession>>>,
    session_id: String,
    path: String,
    canonical: PathBuf,
}

impl JsOriginalRecording {
    pub(crate) fn new(terminal: Terminal, original: OriginalSession) -> Self {
        return Self {
            session_id: original.session().session_id().to_string(),
            path: original.session().path().display().to_string(),
            canonical: original.session().path().to_path_buf(),
            terminal,
            original: Arc::new(Mutex::new(Some(original))),
        };
    }

    fn take(&self) -> napi::Result<OriginalSession> {
        return self
            .original
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner())
            .take()
            .ok_or_else(|| {
                return napi::Error::from_reason(
                    "original recording has already finished or been detached",
                );
            });
    }

    fn restore(&self, original: OriginalSession) {
        *self
            .original
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner()) = Some(original);
    }

    // A capture that timed out or whose stop failed keeps its owner for another wait or a
    // force-close; a capture failure leaves nothing to own.
    fn settle(
        &self,
        original: OriginalSession,
        result: termtrace_sdk::Result<OriginalRecordingModel>,
        request_id: Option<Uuid>,
    ) -> napi::Result<Model<OriginalRecordingModel>> {
        return match result {
            Ok(recording) => Ok(Model(recording)),
            Err(error) => {
                if !matches!(error, termtrace_sdk::Error::OriginalCaptureFailed { .. }) {
                    self.restore(original);
                }

                // Capture lifecycle failures are management facts, not the stop request's.
                let lifecycle = matches!(
                    error,
                    termtrace_sdk::Error::OriginalCaptureFailed { .. }
                        | termtrace_sdk::Error::OriginalFinalizationTimeout { .. }
                );

                Err(match request_id {
                    Some(request_id) if !lifecycle => mutation_err(error, request_id),
                    _ => to_napi_err(error),
                })
            }
        };
    }
}

#[napi]
impl JsOriginalRecording {
    /// Unique session identifier.
    #[napi(getter)]
    pub fn session_id(&self) -> String {
        return self.session_id.clone();
    }

    /// Filesystem path to the canonical session directory.
    #[napi(getter)]
    pub fn path(&self) -> String {
        return self.path.clone();
    }

    /// Whether this handle still owns the recording.
    #[napi]
    pub fn is_active(&self) -> bool {
        return self
            .original
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner())
            .is_some();
    }

    /// Exact canonical session directory bytes.
    #[napi(getter)]
    pub fn path_bytes(&self) -> Uint8Array {
        return Uint8Array::from(self.canonical.as_os_str().as_bytes());
    }

    /// Live control and observation of the canonical session; resizes move the captured window.
    #[napi]
    pub fn session(&self) -> JsSession {
        return JsSession {
            terminal: self.terminal.clone(),
            session_path: self.canonical.clone(),
            session_id: self.session_id.clone(),
        };
    }

    /// Supervisor status, with `abandoned` derived when the supervisor is gone.
    ///
    /// # Errors
    ///
    /// Rejects when the status sidecar is missing or invalid.
    #[napi]
    pub async fn capture_status(
        &self,
    ) -> napi::Result<Model<termtrace_sdk::session::OriginalCaptureStatus>> {
        let path = self.canonical.clone();

        return tokio::task::spawn_blocking(move || {
            let status =
                termtrace_sdk::session::original_capture_status(&path).map_err(to_napi_err)?;

            return Ok(Model(status));
        })
        .await
        .map_err(blocking_failure)?;
    }

    /// Stop the application and wait until the capture is published.
    ///
    /// # Errors
    ///
    /// Rejects with `original_capture_failed` when the capture did not publish,
    /// `original_finalization_timeout` when it was still finalizing at the deadline (the
    /// recording stays owned), and stop failures with `request_id`.
    #[napi]
    pub async fn finish(
        &self,
        timeout_ms: Option<u32>,
        request_id: Option<String>,
    ) -> napi::Result<Model<OriginalRecordingModel>> {
        let timeout = timeout_ms.map_or(defaults::STOP_TIMEOUT, |millis| {
            return Duration::from_millis(u64::from(millis));
        });
        let request_id = request_id
            .map(|id| return Uuid::parse_str(&id))
            .transpose()
            .map_err(|error| return host::type_error(format!("invalid request_id: {error}")))?
            .unwrap_or_else(Uuid::now_v7);
        let mut original = self.take()?;
        let request = StopRequest {
            guard: MutationGuard {
                request_id: Some(request_id),
                if_state: None,
            },
        };
        let (original, result) = tokio::task::spawn_blocking(move || {
            let result = original.finish(request, timeout);

            return (original, result);
        })
        .await
        .map_err(blocking_failure)?;

        return self.settle(original, result, Some(request_id));
    }

    /// Wait without stopping until the application exits and the capture is published.
    ///
    /// # Errors
    ///
    /// As `finish`, without a stop request.
    #[napi]
    pub async fn wait_finished(
        &self,
        timeout_ms: Option<u32>,
    ) -> napi::Result<Model<OriginalRecordingModel>> {
        let timeout = timeout_ms.map_or(defaults::STOP_TIMEOUT, |millis| {
            return Duration::from_millis(u64::from(millis));
        });
        let mut original = self.take()?;
        let (original, result) = tokio::task::spawn_blocking(move || {
            let result = original.wait_finished(timeout);

            return (original, result);
        })
        .await
        .map_err(blocking_failure)?;

        return self.settle(original, result, None);
    }

    /// Kill the application's process group and the capture supervisor; the capture is
    /// abandoned. A no-op once the recording finished or was detached.
    ///
    /// # Errors
    ///
    /// Rejects when the processes cannot be terminated and reaped, leaving the recording owned.
    #[napi]
    pub async fn force_close(&self) -> napi::Result<()> {
        let Ok(mut original) = self.take() else {
            return Ok(());
        };
        let (original, result) = tokio::task::spawn_blocking(move || {
            let result = original.force_close();

            return (original, result);
        })
        .await
        .map_err(blocking_failure)?;

        if let Err(error) = result {
            self.restore(original);

            return Err(to_napi_err(error));
        }

        return Ok(());
    }

    /// Stop managing the recording without stopping it.
    ///
    /// # Errors
    ///
    /// Fails when the recording already finished or was detached.
    #[napi]
    pub fn detach(&self) -> napi::Result<()> {
        let _path = self.take()?.detach();

        return Ok(());
    }
}
