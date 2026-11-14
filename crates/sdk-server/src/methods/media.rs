use super::{method, millis, Registry};
use crate::failure::Failure;
use crate::server::Call;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use termtrace_model::artifact::visual::CaptureOutcome;
use termtrace_model::scenario::ScenarioSuiteReport;
use termtrace_model::terminal::observation::StatePoint;
use termtrace_model::value::ExactPath;
use termtrace_sdk::artifacts::{
    output_frames_for_source_frame, source_for_output_frame, ArtifactService, EditSource,
    OutputFrameSource, PresentationPlan, PreviewOptions, VideoMode, VideoOptions, VideoOutcome,
    VideoPreview, VideoRender,
};
use termtrace_sdk::defaults;
use termtrace_sdk::scenario::{self, run_suite, ScenarioSuiteOptions};
use termtrace_sdk::session::{VisualProfile, VisualProfileReport};
use termtrace_sdk::system::SystemService;

/// Params of `video.plan`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlanParams {
    /// Completed session directory.
    session: ExactPath,
    /// Edit to present.
    edit: EditSource,
    /// Renderer, frame rate, and overwrite policy.
    #[serde(default)]
    options: VideoOptions,
}

/// Params of `video.preview`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PreviewParams {
    /// Completed session directory.
    session: ExactPath,
    /// Edit to present.
    edit: EditSource,
    /// New directory for the contact sheet and review video.
    directory: ExactPath,
    /// Renderer, frame rate, and overwrite policy.
    #[serde(default)]
    options: VideoOptions,
}

/// Params of `video.render`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenderParams {
    /// Completed session directory.
    session: ExactPath,
    /// Edit to present.
    edit: EditSource,
    /// Output file; its `.mp4`, `.gif`, or `.webm` extension selects the format.
    out: ExactPath,
    /// Renderer, frame rate, and overwrite policy.
    #[serde(default)]
    options: VideoOptions,
}

/// Params of `edit.normalize`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NormalizeParams {
    /// Edit to validate.
    edit: EditSource,
}

/// Params of `plan.source_for_output_frame`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OutputFrameParams {
    /// Presentation plan.
    plan: PresentationPlan,
    /// Output frame index.
    output_frame: u64,
}

/// Params of `plan.output_frames_for_source_frame`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SourceFrameParams {
    /// Presentation plan.
    plan: PresentationPlan,
    /// Source frame index.
    source_frame: u64,
}

/// Params of `native.screenshot`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ScreenshotParams {
    /// Completed session directory.
    session: ExactPath,
    /// New directory for the PNG, its manifest, and the replay endpoint.
    destination: ExactPath,
    /// State to capture; the latest presented state when absent.
    #[serde(default)]
    point: Option<StatePoint>,
    /// Bound on the replay terminal's startup; 30 seconds when absent.
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// Params of `visual.preflight`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PreflightParams {
    /// Visual profile to check.
    profile: VisualProfile,
    /// Grid columns; the default when absent.
    #[serde(default)]
    cols: Option<u16>,
    /// Grid rows; the default when absent.
    #[serde(default)]
    rows: Option<u16>,
}

/// Params of `scenario.run`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RunScenariosParams {
    /// Scenario documents.
    scenarios: Vec<ExactPath>,
    /// Parent of failure artifacts; snapshot baselines live in its sibling `snapshots`.
    failure_root: ExactPath,
    /// Parent of each case's session; the client's evidence root when absent.
    #[serde(default)]
    evidence_root: Option<ExactPath>,
    /// Maximum cases run at once; 1 when absent.
    #[serde(default)]
    jobs: Option<u32>,
    /// Zero-based shard index; requires `shard_count`.
    #[serde(default)]
    shard_index: Option<u32>,
    /// Total shard count; requires `shard_index`.
    #[serde(default)]
    shard_count: Option<u32>,
    /// Whether to replace snapshot baselines instead of comparing them.
    #[serde(default)]
    update_snapshots: bool,
    /// Fresh-session reruns of a failed case.
    #[serde(default)]
    reruns: u8,
    /// Whether a flaky case fails CI.
    #[serde(default)]
    fail_on_flaky: bool,
}

/// Params of `scenario.write_junit`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct JunitParams {
    /// New XML file.
    path: ExactPath,
    /// Suite report.
    report: ScenarioSuiteReport,
}

/// Params of `scenario.ci_failed`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CiParams {
    /// Suite report.
    report: ScenarioSuiteReport,
}

pub(super) fn register(registry: &mut Registry) {
    method(
        registry,
        "video.plan",
        "Resolve the presentation plan of an edit without rendering.",
        plan,
    );
    method(
        registry,
        "video.preview",
        "Render a contact sheet and review video of an edit.",
        preview,
    );
    method(
        registry,
        "video.render",
        "Render an edit to an MP4, GIF, or WebM file with its sidecar.",
        render,
    );
    method(
        registry,
        "edit.normalize",
        "Validate an edit, resolve its subtitle paths, and hash it.",
        |_call, params: NormalizeParams| {
            return sdk(ArtifactService::normalize_edit(&params.edit));
        },
    );
    method(
        registry,
        "plan.source_for_output_frame",
        "Report the clip and source frame an output frame shows; null past the end.",
        |_call, params: OutputFrameParams| -> Result<Option<OutputFrameSource>, Failure> {
            return Ok(source_for_output_frame(&params.plan, params.output_frame));
        },
    );
    method(
        registry,
        "plan.output_frames_for_source_frame",
        "List the half-open output-frame spans that show a source frame.",
        |_call, params: SourceFrameParams| -> Result<Vec<(u64, u64)>, Failure> {
            return Ok(
                output_frames_for_source_frame(&params.plan, params.source_frame)
                    .into_iter()
                    .map(|span| return (span.start, span.end))
                    .collect(),
            );
        },
    );
    method(
        registry,
        "native.screenshot",
        "Replay a completed session in Ghostty and capture one state as a PNG.",
        screenshot,
    );
    method(
        registry,
        "visual.preflight",
        "Validate a visual profile and report what a launch with it would use.",
        preflight,
    );
    method(
        registry,
        "scenario.run",
        "Run scenario documents as one suite.",
        run_scenarios,
    );
    method(
        registry,
        "scenario.write_junit",
        "Write a suite report as JUnit XML to a new file.",
        |_call, params: JunitParams| {
            return scenario::write_scenario_junit(&params.path.0, &params.report)
                .map_err(|error| return scenario_failure(&error));
        },
    );
    method(
        registry,
        "scenario.ci_failed",
        "Report whether CI must fail for a suite report.",
        |_call, params: CiParams| -> Result<bool, Failure> {
            return Ok(params.report.ci_failed());
        },
    );
}

fn sdk<T>(result: termtrace_sdk::Result<T>) -> Result<T, Failure> {
    return result.map_err(|error| return Failure::sdk(&error));
}

fn scenario_failure(error: &scenario::Error) -> Failure {
    return Failure::sdk(&termtrace_sdk::Error::InvalidOperation {
        reason: error.to_string(),
        source: None,
    });
}

fn video(
    call: &Call<'_>,
    session: ExactPath,
    edit: EditSource,
    mode: VideoMode,
    options: VideoOptions,
) -> Result<VideoOutcome, Failure> {
    let binary = call.terminal()?.binary().to_path_buf();
    let request = sdk(options.request(session.0, edit, mode))?;
    let cancelled = AtomicBool::new(false);

    return sdk(ArtifactService::video(
        &request,
        &binary,
        &mut |id| return SystemService::require(id, &cancelled),
        &mut |_| {},
    ));
}

fn other_outcome(mode: &str) -> Failure {
    return Failure::internal(format!("{mode} returned another outcome"));
}

fn plan(call: &Call<'_>, params: PlanParams) -> Result<PresentationPlan, Failure> {
    let outcome = video(
        call,
        params.session,
        params.edit,
        VideoMode::Plan,
        params.options,
    )?;

    let VideoOutcome::Planned(plan) = outcome else {
        return Err(other_outcome("plan"));
    };
    return Ok(*plan);
}

fn preview(call: &Call<'_>, params: PreviewParams) -> Result<VideoPreview, Failure> {
    let mode = VideoMode::Preview {
        directory: params.directory.0,
        options: PreviewOptions::default(),
    };
    let outcome = video(call, params.session, params.edit, mode, params.options)?;
    let VideoOutcome::Previewed(preview) = outcome else {
        return Err(other_outcome("preview"));
    };
    return Ok(*preview);
}

fn render(call: &Call<'_>, params: RenderParams) -> Result<VideoRender, Failure> {
    let mode = VideoMode::Render { out: params.out.0 };
    let outcome = video(call, params.session, params.edit, mode, params.options)?;
    let VideoOutcome::Rendered(render) = outcome else {
        return Err(other_outcome("render"));
    };
    return Ok(*render);
}

fn screenshot(call: &Call<'_>, params: ScreenshotParams) -> Result<CaptureOutcome, Failure> {
    let session = sdk(call.terminal()?.attach(&params.session.0))?;
    let timeout = millis(params.timeout_ms, defaults::NATIVE_REPLAY_TIMEOUT);

    return sdk(session.native().screenshot(
        &params.point.unwrap_or(StatePoint::LatestPresented),
        &params.destination.0,
        None,
        Arc::new(AtomicBool::new(false)),
        timeout,
    ));
}

fn preflight(call: &Call<'_>, params: PreflightParams) -> Result<VisualProfileReport, Failure> {
    return sdk(call.terminal()?.preflight_visual_profile(
        &params.profile,
        params.cols.unwrap_or(defaults::COLS),
        params.rows.unwrap_or(defaults::ROWS),
    ));
}

fn run_scenarios(
    call: &Call<'_>,
    params: RunScenariosParams,
) -> Result<ScenarioSuiteReport, Failure> {
    let terminal = call.terminal()?;
    let options = ScenarioSuiteOptions {
        binary: terminal.binary().to_path_buf(),
        evidence_root: params.evidence_root.map_or_else(
            || return terminal.evidence_root().to_path_buf(),
            |root| return root.0,
        ),
        failure_root: params.failure_root.0,
        jobs: params.jobs.map_or(1, |jobs| return jobs as usize),
        shard_index: params.shard_index.map(|index| return index as usize),
        shard_count: params.shard_count.map(|count| return count as usize),
        update_snapshots: params.update_snapshots,
        reruns: params.reruns,
        fail_on_flaky: params.fail_on_flaky,
    };

    let paths = params.scenarios.into_iter().map(|path| return path.0);

    return run_suite(paths, &options).map_err(|error| return scenario_failure(&error));
}
