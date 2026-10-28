use crate::cli::output::ResponseWriter;

pub(super) mod args;

use crate::cli::{
    control::args::millis_or,
    output::{response_output, write_json_line, write_json_pretty, write_stdout},
    recording::args::{
        ColorPolicyArg, DurabilityModeArg, GeometryMode, GeometryProbeArgs, ProvenancePolicyArg,
        RecordArgs, RecordingArgs, RestartArgs, RestartOverrides,
    },
    session_helpers::attach_session,
};
use miette::{IntoDiagnostic as _, Result};
use nix::{
    errno::Errno,
    poll::{poll, PollFd, PollFlags, PollTimeout},
    sys::termios::{cfmakeraw, tcgetattr, tcsetattr, SetArg},
    unistd::read,
};
use std::{
    env,
    ffi::{OsStr, OsString},
    fs::OpenOptions,
    io::{self, Read as _, Write as _},
    os::{
        fd::AsFd as _,
        unix::{
            ffi::{OsStrExt as _, OsStringExt as _},
            fs::OpenOptionsExt as _,
        },
    },
    path::{Path, PathBuf},
    process::ExitCode,
    result::Result as StdResult,
    time::{Duration, Instant},
};
use termtrace_engine::{
    analysis::profile::{builtin_terminal_colors, terminal_colors},
    inspection::wait_for_completion,
    recording::{
        inherited_geometry, parse_environment_spec, read_environment_spec,
        record_with_resize_authority, resolve_ghostty_profile, resolve_visual_profile,
        restore_ghostty_profile, EffectiveEnvironment, RecordOptions, RecordOutcome,
        ResizeAuthority, ResolvedGhosttyProfile,
    },
};
use termtrace_evidence::{
    event_log::{DurabilityPolicy, EventLogSummary},
    session::{manifest_hash, read_manifest},
};
use termtrace_media::native_capture::probe_ghostty_geometry;
use termtrace_model::{
    execution::environment::{ColorPolicy, EnvironmentBase, EnvironmentSpec},
    execution::event_log::DurabilityMode,
    execution::layout::SessionLayout,
    execution::provenance::{ProvenanceCapture, ProvenancePolicy},
    execution::session::{
        GeometryProvenance, GeometrySource, GhosttyVisualOverrides, ImportProvenance,
        ResourceLimits, SessionCompletion, SessionGeometry, SessionLineage, SessionManifest,
    },
    execution::visual_profile::{
        NativeVisualProfile, RecordedVisualProfile, VisualProfile, VisualProfileFailure,
    },
    protocol::control::{ScreenPredicate, ScreenWaitQuality},
    registry::{DetachedSessionPaths, RegistryEntry},
    terminal::screen::{ScreenFrame, ScreenProjectionRequest},
    terminal::state::{supported_geometry_cell_count, terminal_geometry_limits},
    value::geometry::TerminalGeometryReport,
    value::ByteString,
};
use termtrace_sdk::{defaults, screen::frame_text, StableCaptureOptions};
use termtrace_service::registry::{DetachedStart, RegistryService};

const DEFAULT_DURABILITY_MAX_INTERVAL_MS: u64 = 50;
const DEFAULT_DURABILITY_MAX_UNCOMMITTED_BYTES: u64 = 65_536;
const GEOMETRY_QUERY_TIMEOUT: Duration = Duration::from_secs(3);

mod geometry;
mod original;
mod record;
mod restart;
mod start;

pub(super) use geometry::run_geometry_probe;
use geometry::{manual_geometry, resolve_record};
use record::durability_policy_for;
pub(super) use record::run_record;
use restart::restarted_record_options;
pub(super) use restart::run_restart;
