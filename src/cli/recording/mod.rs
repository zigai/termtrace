use crate::cli::output::ResponseWriter;

pub(super) mod args;

use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io::{self, Read as _, Write as _},
    os::{
        fd::AsFd as _,
        unix::{
            ffi::{OsStrExt as _, OsStringExt as _},
            fs::OpenOptionsExt as _,
        },
    },
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode, Stdio},
    result::Result as StdResult,
    thread,
    time::{Duration, Instant},
};

use miette::{IntoDiagnostic as _, Result};
use nix::{
    errno::Errno,
    poll::{PollFd, PollFlags, PollTimeout, poll},
    sys::termios::{SetArg, cfmakeraw, tcgetattr, tcsetattr},
    unistd::read,
};
use termtrace_engine::{
    analysis::profile::{builtin_terminal_colors, terminal_colors_for_profile},
    inspection::wait_for_completion,
    recording::{
        EffectiveEnvironment, RecordOptions, RecordOutcome, ResolvedGhosttyProfile,
        inherited_geometry, parse_environment_spec, read_environment_spec, record_with_output,
        resolve_ghostty_profile, restore_ghostty_profile,
    },
    registry::{SessionRegistry, recorder_identity},
};
use termtrace_evidence::{
    event_log::{DurabilityPolicy, EventLogSummary},
    session::{manifest_hash, read_manifest},
};
use termtrace_media::native_capture::probe_ghostty_geometry;
use termtrace_model::{
    execution::environment::{ColorPolicy, EnvironmentBase, EnvironmentSpec},
    execution::event_log::DurabilityMode,
    execution::provenance::{ProvenanceCapture, ProvenancePolicy},
    execution::session::{
        GHOSTTY_REPLAY_PROFILE_NAME, GeometryProvenance, GeometrySource, GhosttyVisualOverrides,
        ImportProvenance, ResourceLimits, SessionCompletion, SessionGeometry, SessionLineage,
        SessionManifest,
    },
    protocol::control::{
        ControlCommand, ControlData, ControlRequest, ScreenPredicate, ScreenWaitQuality,
        StableCaptureQuality,
    },
    registry::{AliasReservation, DetachedSessionPaths, RegistryEntry},
    terminal::screen::ScreenProjectionRequest,
    value::ByteString,
    value::geometry::TerminalGeometryReport,
};
use termtrace_sdk::{request, send_request, wait_for_screen};

use crate::cli::{
    output::{response_output, write_json_line},
    recording::args::{
        ColorPolicyArg, DurabilityModeArg, GeometryMode, GeometryProbeArgs, ProvenancePolicyArg,
        RecordArgs, RecordingArgs, RestartArgs, RestartOverrides, StartArgs,
    },
};

const DEFAULT_DURABILITY_MAX_INTERVAL_MS: u64 = 50;
const DEFAULT_DURABILITY_MAX_UNCOMMITTED_BYTES: u64 = 65_536;

const GEOMETRY_QUERY_TIMEOUT: Duration = Duration::from_secs(3);

mod geometry;
mod original;
mod record;
mod restart;
mod start;

use geometry::resolve_record_geometry;
use record::durability_policy_for;
use restart::restarted_record_options;

pub(super) use geometry::run_geometry_probe;
pub(super) use record::run_record;
pub(super) use restart::run_restart;
pub(super) use start::{open_detached_log, run_start};
