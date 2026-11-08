//! Original-execution capture supervision: status sidecar, failure facts and window resizes.
//!
//! These records live outside canonical evidence. The status sidecar is published beside the
//! session directory and atomically replaced on every transition. `window-resizes.jsonl` is a
//! member of the published `visual-original` artifact.

use crate::{
    artifact::sdk::{DecimalU64, MediaArtifactResult},
    execution::session::SessionCompletion,
    value::ByteString,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Stable format identifier of the original-capture status sidecar.
pub const ORIGINAL_CAPTURE_STATUS_FORMAT: &str = "termtrace.original-capture-status";
/// Current original-capture status sidecar version.
pub const ORIGINAL_CAPTURE_STATUS_VERSION: u32 = 1;
/// Artifact member recording every supervisor window-resize decision.
pub const WINDOW_RESIZES_NAME: &str = "window-resizes.jsonl";

/// Sidecar path for an original-capture session directory.
#[must_use]
pub fn original_capture_status_path(session: &Path) -> PathBuf {
    let mut name = session.file_name().unwrap_or_default().to_os_string();
    name.push(".original-capture.json");

    return session.with_file_name(name);
}

/// Supervisor lifecycle state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OriginalCaptureState {
    /// Display, encoder and terminal are starting; the program has not started.
    Acquiring,
    /// The program runs inside the captured terminal and accepts live control.
    Live,
    /// The program completed; acquisition is stopping and the artifact is being published.
    Finalizing,
    /// The artifact was published; `result` names it.
    Published,
    /// Capture failed; `failure` names the code and the paths that survived.
    Failed,
    /// Capture was cancelled; `failure` names the paths that survived.
    Cancelled,
    /// Derived by readers only: a non-terminal state whose supervisor is no longer running.
    Abandoned,
}

impl OriginalCaptureState {
    /// Whether the supervisor has written its final state.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        return matches!(self, Self::Published | Self::Failed | Self::Cancelled);
    }
}

/// Stable code for an original capture that did not publish an artifact.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OriginalCaptureFailure {
    /// Startup or capture was cancelled by the caller.
    Cancelled,
    /// The display, terminal or encoder failed, or the acquired video did not verify.
    AcquisitionFailed,
    /// The observed terminal grid differs from the requested exact grid.
    GridMismatch,
    /// The supervisor stopped running before it published a final state.
    SupervisorAbandoned,
    /// The canonical recording failed or its completion did not match the captured run.
    RecordingFailed,
    /// The verified capture could not be committed beside the session.
    PublicationFailed,
    /// The supervisor could not start the capture environment.
    StartupFailed,
}

impl OriginalCaptureFailure {
    /// Stable wire code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        return match self {
            Self::Cancelled => "cancelled",
            Self::AcquisitionFailed => "acquisition_failed",
            Self::GridMismatch => "grid_mismatch",
            Self::SupervisorAbandoned => "supervisor_abandoned",
            Self::RecordingFailed => "recording_failed",
            Self::PublicationFailed => "publication_failed",
            Self::StartupFailed => "startup_failed",
        };
    }
}

/// What a surviving path holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SurvivingKind {
    /// Canonical session directory.
    Session,
    /// Preserved failed capture staging (`visual-failed-*`).
    FailedVisual,
    /// Capture staging that could not be preserved under its failed name.
    Staging,
    /// Original-capture status sidecar.
    Sidecar,
    /// Supervisor or recorder diagnostic log.
    Log,
}

/// A path verified to exist when the failure was reported.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SurvivingPathRecord {
    /// Exact Linux path bytes.
    #[serde(with = "crate::value::path_bytes")]
    #[schemars(with = "crate::value::path_bytes::PathValue")]
    pub path: ByteString,
    /// What the path holds.
    pub kind: SurvivingKind,
}

/// Terminal failure facts written by the supervisor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OriginalCaptureFailureRecord {
    /// Stable failure code.
    pub code: OriginalCaptureFailure,
    /// Primary failure description.
    pub reason: String,
    /// Paths that existed after bounded cleanup; nothing else is implied to exist.
    pub surviving: Vec<SurvivingPathRecord>,
    /// Teardown or preservation failure reported beside the primary failure.
    pub cleanup: Option<String>,
}

/// What a caller learns when an original capture did not publish, or did not finish in time.
///
/// The captured application may have run and completed; `completion` is its canonical outcome
/// when one validates. This is a lifecycle failure, not an actor mutation, so it carries no
/// request effect and no automatic retry.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OriginalCaptureFacts {
    /// Failure code; `None` when finalization did not reach a final state before the deadline.
    pub code: Option<OriginalCaptureFailure>,
    /// Supervisor state observed when the error was built, including derived `abandoned`.
    pub state: OriginalCaptureState,
    /// Exact canonical session path bytes.
    #[serde(with = "crate::value::path_bytes")]
    #[schemars(with = "crate::value::path_bytes::PathValue")]
    pub session: ByteString,
    /// Validated canonical completion, when one exists.
    pub completion: Option<Box<SessionCompletion>>,
    /// Paths verified to exist when the error was built.
    pub surviving: Vec<SurvivingPathRecord>,
    /// Teardown, preservation or reaping failure reported beside the primary failure.
    pub cleanup: Option<String>,
    /// Primary failure description.
    pub reason: String,
}

/// Terminal grid in cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CellGrid {
    /// Columns.
    pub cols: u16,
    /// Rows.
    pub rows: u16,
}

/// Captured display size in pixels; constant for the whole video.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PixelCanvas {
    /// Width in pixels.
    pub width: u16,
    /// Height in pixels.
    pub height: u16,
}

/// Owner-only supervisor status, published beside the session as
/// `<session>.original-capture.json`. Not canonical evidence.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OriginalCaptureStatus {
    /// Exactly [`ORIGINAL_CAPTURE_STATUS_FORMAT`].
    pub format: String,
    /// Exactly [`ORIGINAL_CAPTURE_STATUS_VERSION`].
    pub version: u32,
    /// Pixel source; `private_x11` is the only supervised backend.
    pub backend: String,
    /// Supervisor process ID.
    pub supervisor_pid: DecimalU64,
    /// Supervisor kernel start ticks, so a reused PID never reads as the supervisor.
    pub supervisor_start_ticks: DecimalU64,
    /// Supervisor state as written; readers derive `abandoned`.
    pub state: OriginalCaptureState,
    /// Canonical session this supervisor records, written when the capture becomes live and
    /// kept afterwards; `None` only before the session exists.
    pub session_id: Option<Uuid>,
    /// Requested exact grid, or the observed grid once live for a viewport-derived grid.
    pub grid: Option<CellGrid>,
    /// Captured display size.
    pub canvas: PixelCanvas,
    /// Acquisition frame rate.
    pub fps: u16,
    /// Published artifact, present only in `published`.
    pub result: Option<MediaArtifactResult>,
    /// Failure facts, present only in `failed` and `cancelled`.
    pub failure: Option<OriginalCaptureFailureRecord>,
}

impl OriginalCaptureStatus {
    /// Validate the format, version and state-dependent fields.
    ///
    /// # Errors
    /// Returns a description of the first violated invariant.
    pub fn validate(&self) -> Result<(), String> {
        if self.format != ORIGINAL_CAPTURE_STATUS_FORMAT
            || self.version != ORIGINAL_CAPTURE_STATUS_VERSION
        {
            return Err("unsupported original-capture status format".to_owned());
        }

        if self.backend != "private_x11" {
            return Err("unsupported original-capture backend".to_owned());
        }

        if self.state == OriginalCaptureState::Abandoned {
            return Err("abandoned is derived by readers and never written".to_owned());
        }

        if (self.state == OriginalCaptureState::Published) != self.result.is_some() {
            return Err("only a published capture carries a result".to_owned());
        }

        if matches!(
            self.state,
            OriginalCaptureState::Failed | OriginalCaptureState::Cancelled
        ) != self.failure.is_some()
        {
            return Err("only a failed or cancelled capture carries failure facts".to_owned());
        }

        if self.session_id.is_none()
            && matches!(
                self.state,
                OriginalCaptureState::Live
                    | OriginalCaptureState::Finalizing
                    | OriginalCaptureState::Published
            )
        {
            return Err("a live, finalizing or published capture names its session".to_owned());
        }

        if let Some(result) = &self.result {
            result.validate()?;
            if Some(result.source.session_id) != self.session_id {
                return Err("the published capture belongs to another session".to_owned());
            }
        }

        return Ok(());
    }
}

/// Supervisor answer to one window-resize request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WindowResizeDecision {
    /// The window was configured; the grid change is observed separately.
    Configured,
    /// The window was not touched.
    Rejected,
}

/// One line of `window-resizes.jsonl`: the window-rect timeline of a capture.
///
/// The video canvas never changes; this records where the terminal window sat inside it.
/// No presentation claim is made about when a frame shows the new grid.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WindowResizeRecord {
    /// Caller request identity, shared with the canonical resize events.
    pub request_id: Uuid,
    /// Requested grid.
    pub requested: CellGrid,
    /// Whether the window was configured.
    pub decision: WindowResizeDecision,
    /// Stable rejection code.
    pub code: Option<String>,
    /// Configured window width in pixels, including Ghostty's window overhead.
    pub window_width: Option<u32>,
    /// Configured window height in pixels, including Ghostty's window overhead.
    pub window_height: Option<u32>,
    /// Wall-clock time of the configure request.
    pub configured_unix_ns: Option<DecimalU64>,
    /// Grid observed on the captured terminal's PTY after configuration.
    pub observed: Option<CellGrid>,
    /// Canonical `resize_intent` event carrying this request identity.
    pub event_seq: Option<DecimalU64>,
}
