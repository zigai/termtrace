//! Versioned newline-framed process driver for language-neutral clients.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    env,
    ffi::OsString,
    io::{BufRead, BufReader, Error as IoError, Read, Result as IoResult, Write},
    os::unix::ffi::OsStringExt as _,
    path::{Path, PathBuf},
    result::Result as StdResult,
    time::Duration,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    analysis::{DiagnoseReport, DiagnosticCheck, diagnose},
    artifact::sync_directory,
    bundle::{BundleManifest, BundleProfile, create as create_bundle},
    capability::{NATIVE_SCREENSHOT_CAPABILITY, require_capability},
    client::Terminal,
    control::{
        CONTROL_PROTOCOL_VERSION, ControlCommand, ControlData, ControlErrorDetails, ControlRequest,
        ControlResult, send_request_response,
    },
    diff::{SemanticDiff, diff_states, inspect_point},
    environment::EnvironmentSpec,
    event::{ByteString, EVENT_LOG_VERSION, EventProjection},
    event_log::EventLog,
    inspection::{WaitOutcome, wait_for_completion, wait_for_output},
    observation::{Observation, StatePoint, observe_session_with_options},
    replay::{STATE_FORMAT_VERSION, StateSelection},
    save::{SaveFormat, save},
    session::{
        CaptureTarget, ResourceLimits, SessionCompletion, VerificationLevel, VerificationReport,
        read_completion, read_manifest, validate_completed_log, verify_session,
    },
    session_registry::SessionRegistry,
    timeline::{TimelineEntry, build_timeline_page},
    visual::{CaptureKind, CaptureOptions, capture, native_capture_target},
};

mod framing;
mod operations;
mod protocol;
mod runtime;

use framing::*;
use operations::*;
pub use protocol::*;
pub use runtime::serve;
use runtime::*;

#[cfg(test)]
mod tests;
