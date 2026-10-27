//! Recording options, actor messages, and terminal geometry.

use super::{
    outer_resize::Dispatch,
    support::{finish_with_cleanup, terminal_error},
};
use crate::analysis::profile::ResolvedGhosttyProfile;
use crate::Result;
use portable_pty::{Child, MasterPty, PtySize};
use std::{
    ffi::OsString,
    io::{Read, Write},
    path::PathBuf,
    sync::mpsc,
    time::Instant,
};
use termtrace_evidence::event_log::DurabilityPolicy;
use termtrace_model::{
    execution::environment::EnvironmentSpec,
    execution::event::TerminalColors,
    execution::provenance::ProvenancePolicy,
    execution::session::{
        GeometryProvenance, ImportProvenance, RecordingFailurePolicy, ResourceLimitOutcome,
        ResourceLimits, SessionLineage, TmuxPaneProvenance,
    },
    protocol::control::{ControlRequest, ControlResponse, ProcessSignal},
};
use termtrace_process::ProcessGroup;

pub(in crate::recording) struct SpawnedCommand {
    pub(in crate::recording) master: Box<dyn MasterPty + Send>,
    pub(in crate::recording) reader: Box<dyn Read + Send>,
    pub(in crate::recording) writer: Box<dyn Write + Send>,
    pub(in crate::recording) child: Box<dyn Child + Send + Sync>,
    pub(in crate::recording) pid: Option<u32>,
}

impl SpawnedCommand {
    /// The session and process group the PTY child leads.
    pub(in crate::recording) fn process_group(&self) -> Option<ProcessGroup> {
        return self.pid.and_then(ProcessGroup::from_leader);
    }

    /// `SIGKILL` the recorded process group; one that is already gone needs nothing.
    pub(in crate::recording) fn kill_process_group(&self) {
        if let Some(group) = self.process_group() {
            let _kill = group.kill();
        }
    }

    /// Kill the recorded process group and child, then reap the child after a recorder failure.
    pub(in crate::recording) fn reap_after_failure(&mut self) -> Result<()> {
        self.kill_process_group();
        // An exited child is already reaped, and signalling its released PID would fail
        // spuriously and mask the primary failure.
        if let Ok(Some(_status)) = self.child.try_wait() {
            return Ok(());
        }

        let killed = self
            .child
            .kill()
            .map_err(|error| return terminal_error("kill child", error.to_string()));
        let reaped = self
            .child
            .wait()
            .map(|_status| {})
            .map_err(|error| return terminal_error("wait for child", error.to_string()));

        return finish_with_cleanup(killed, reaped);
    }
}

/// Configuration for one recorded command.
#[derive(Clone, Debug)]
pub struct RecordOptions {
    /// Stop owned tests or preserve interactive applications after recorder failure.
    pub failure_policy: RecordingFailurePolicy,
    /// New session directory.
    pub output_directory: PathBuf,
    /// Optional friendly invocation alias retained only as creation context.
    pub creation_alias: Option<String>,
    /// Immutable predecessor evidence for a restarted successor.
    pub lineage: Option<SessionLineage>,
    /// Chain-bound provenance for an imported ANSI byte stream.
    pub import_provenance: Option<ImportProvenance>,
    /// Immutable tmux pane identity when the recorder owns a tmux pane child PTY.
    pub tmux_pane: Option<TmuxPaneProvenance>,
    /// Executable and arguments.
    pub command: Vec<OsString>,
    /// Child working directory.
    pub working_directory: Option<PathBuf>,
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
    /// Cell width reported to the PTY, in pixels.
    pub cell_width_px: u16,
    /// Cell height reported to the PTY, in pixels.
    pub cell_height_px: u16,
    /// Provenance for manual, inherited, or Ghostty-fitted geometry.
    pub geometry_provenance: GeometryProvenance,
    /// Canonical per-recording Ghostty visual profile, when configured.
    pub ghostty_profile: Option<ResolvedGhosttyProfile>,
    /// Terminal identity exposed to the child.
    pub term: String,
    /// Deterministic child-environment policy.
    pub environment: EnvironmentSpec,
    /// Default colors exposed by the terminal host and pinned into replay.
    pub colors: TerminalColors,
    /// Forward standard input to the child PTY.
    pub forward_stdin: bool,
    /// Continue canonical recording when the presentation output sink disappears.
    pub continue_after_output_mirror_closes: bool,
    /// Output persistence policy committed into canonical provenance.
    pub durability_policy: DurabilityPolicy,
    /// Sensitivity-aware execution provenance collection policy.
    pub provenance_policy: ProvenancePolicy,
    /// Runtime limits that terminate runaway work without truncating evidence.
    pub resource_limits: ResourceLimits,
}

/// Completed command information.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordOutcome {
    /// Normal process exit code when available.
    pub exit_code: Option<i32>,
    /// Signal description when signal termination occurred.
    pub signal_name: Option<String>,
    /// Number of output bytes recorded.
    pub output_bytes: u64,
    /// Resource budget that terminated the recording, when one was exceeded.
    pub resource_limit: Option<ResourceLimitOutcome>,
    /// Canonical event-log path.
    pub event_log: PathBuf,
}

pub(super) enum RuntimeMessage {
    Output {
        data: Vec<u8>,
        observed: Instant,
    },
    OutputClosed,
    ReadFailed(String),
    Input(Vec<u8>),
    InputFailed(String),
    TerminalInputClosed,
    Resize(TerminalGeometry),
    ResizeFailed(String),
    HostSignal(ProcessSignal),
    /// A child of the recorder changed state; the recorded child may be reapable.
    ChildStateChanged,
    Control {
        request: ControlRequest,
        response: mpsc::SyncSender<ControlResponse>,
    },
    ControlServerFailed(String),
    /// The control server thread finished and accepts no further requests.
    ControlServerStopped,
    /// The supervisor exchange for one outer-terminal resize finished.
    OuterResizeReply {
        request_id: uuid::Uuid,
        dispatch: Dispatch,
    },
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct TerminalGeometry {
    pub(super) cols: u16,
    pub(super) rows: u16,
    pub(super) pixel_width: u16,
    pub(super) pixel_height: u16,
}

impl From<PtySize> for TerminalGeometry {
    fn from(size: PtySize) -> Self {
        return Self {
            cols: size.cols,
            rows: size.rows,
            pixel_width: size.pixel_width,
            pixel_height: size.pixel_height,
        };
    }
}

impl From<TerminalGeometry> for PtySize {
    fn from(geometry: TerminalGeometry) -> Self {
        return Self {
            rows: geometry.rows,
            cols: geometry.cols,
            pixel_width: geometry.pixel_width,
            pixel_height: geometry.pixel_height,
        };
    }
}
