//! Record one tmux pane process with the engine recorder (`termtrace tmux run`).

use miette::{IntoDiagnostic as _, Result};
use std::process::ExitCode;
use termtrace_engine::{
    analysis::profile::{builtin_terminal_colors, terminal_colors_for_profile},
    recording::{inherited_geometry, record, resolve_ghostty_profile, RecordOptions},
    registry::SessionRegistry,
};
use termtrace_evidence::event_log::DurabilityPolicy;
use termtrace_model::execution::{
    environment::EnvironmentSpec,
    provenance::ProvenancePolicy,
    session::{
        GeometryProvenance, GeometrySource, GhosttyVisualOverrides, RecordingFailurePolicy,
        ResourceLimits,
    },
};
use termtrace_tmux::{
    pane::{exec_unrecorded, prepare, PaneLaunch, PaneRecording, PaneRunRequest, Publication},
    setup::GhosttyProfileMode,
};

pub(super) fn run(request: PaneRunRequest) -> Result<ExitCode> {
    let recording = match prepare(request).into_diagnostic()? {
        PaneLaunch::Unrecorded(command) => return Err(exec_unrecorded(command)).into_diagnostic(),
        PaneLaunch::Record(recording) => *recording,
    };

    return record_pane(recording);
}

fn record_pane(mut recording: PaneRecording) -> Result<ExitCode> {
    let geometry =
        inherited_geometry(recording.cell_width, recording.cell_height).into_diagnostic()?;
    recording.provenance.pane_width = geometry.cols;
    recording.provenance.pane_height = geometry.rows;

    let ghostty_config = recording.ghostty_config.as_deref();
    let ghostty_profile = match recording.profile_mode {
        GhosttyProfileMode::Off => None,
        GhosttyProfileMode::Required => Some(
            resolve_ghostty_profile(ghostty_config, GhosttyVisualOverrides::default())
                .into_diagnostic()?,
        ),
        GhosttyProfileMode::Auto => {
            match resolve_ghostty_profile(ghostty_config, GhosttyVisualOverrides::default()) {
                Ok(profile) => Some(profile),
                Err(error) if ghostty_config.is_none() => {
                    tracing::debug!(%error, "Ghostty profile is unavailable for tmux pane recording");

                    None
                }
                Err(error) => return Err(error).into_diagnostic(),
            }
        }
    };

    let colors = ghostty_profile
        .as_ref()
        .map_or_else(builtin_terminal_colors, terminal_colors_for_profile)
        .into_diagnostic()?;

    let registry = SessionRegistry::open(&recording.output_root).into_diagnostic()?;
    let paths = registry.allocate_detached_paths().into_diagnostic()?;

    let cell_width_px = geometry
        .pixel_width
        .checked_div(geometry.cols)
        .ok_or_else(|| return miette::miette!("inherited tmux pane has no terminal columns"))?;
    let cell_height_px = geometry
        .pixel_height
        .checked_div(geometry.rows)
        .ok_or_else(|| return miette::miette!("inherited tmux pane has no terminal rows"))?;

    let publication = Publication::start(&recording, paths.session.clone()).into_diagnostic()?;
    let options = RecordOptions {
        failure_policy: RecordingFailurePolicy::Preserve,
        output_directory: paths.session,
        creation_alias: Some(recording.creation_alias()),
        lineage: None,
        import_provenance: None,
        tmux_pane: Some(recording.provenance),
        command: recording.command,
        working_directory: None,
        cols: geometry.cols,
        rows: geometry.rows,
        cell_width_px,
        cell_height_px,
        geometry_provenance: GeometryProvenance {
            source: GeometrySource::Inherited,
            capture_target: None,
        },
        ghostty_profile,
        term: recording.term,
        environment: EnvironmentSpec {
            unset: [String::from("TERMTRACE_RESPONSE")].into_iter().collect(),
            ..EnvironmentSpec::default()
        },
        colors,
        forward_stdin: true,
        continue_after_output_mirror_closes: true,
        durability_policy: DurabilityPolicy::default(),
        provenance_policy: ProvenancePolicy::default(),
        resource_limits: ResourceLimits::default(),
    };
    let outcome = record(&options);

    if let Err(error) = publication.finish() {
        tracing::debug!(%error, pane = %recording.pane, "could not publish tmux pane recording path");
    }

    return outcome
        .into_diagnostic()?
        .exit_code
        .and_then(|code| return u8::try_from(code).ok())
        .map_or(Ok(ExitCode::FAILURE), |code| {
            return Ok(ExitCode::from(code));
        });
}
