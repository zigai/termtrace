use super::{method, millis, Registry};
use crate::failure::Failure;
use crate::server::Call;
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::Path;
use termtrace_model::artifact::bundle::{BundleManifest, BundleProfile};
use termtrace_model::artifact::presentation::MarkerRecord;
use termtrace_model::artifact::save::{SaveFormat, SaveOutcome};
use termtrace_model::execution::event_log::RecoveryReport;
use termtrace_model::execution::paging::EventFollowPosition;
use termtrace_model::execution::session::{VerificationLevel, VerificationReport};
use termtrace_model::terminal::analysis::{
    DiagnoseReport, DiagnosticCheck, ExplainReport, LastActionAnalysis,
};
use termtrace_model::terminal::diff::SemanticDiff;
use termtrace_model::terminal::observation::{
    ReplayCachePruneReport, ReplayCacheStatus, StatePoint,
};
use termtrace_model::terminal::replay::SourceRange;
use termtrace_model::terminal::reviewer::{
    ReviewExportManifest, ReviewSearchResult, ReviewSummary,
};
use termtrace_model::terminal::timeline::TimelinePage;
use termtrace_model::value::ExactPath;
use termtrace_sdk::artifacts::ArtifactService;
use termtrace_sdk::contracts::diagnostics as core;
use termtrace_sdk::defaults;
use termtrace_sdk::events::{EventPageCursor, EventsService, ProjectedEventPage};
use termtrace_sdk::inspection::WaitOutcome;
use termtrace_sdk::output::{ByteOutputPage, OutputPageCursor, OutputService};
use termtrace_sdk::ByteString;

/// Params of calls that only read one session directory.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SessionParams {
    /// Session directory.
    session: ExactPath,
}

/// Params of `output.page`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OutputPageParams {
    /// Session directory.
    session: ExactPath,
    /// Position returned by the previous page; the start when absent.
    #[serde(default)]
    cursor: Option<OutputPageCursor>,
    /// Maximum bytes to return; 65536 when absent.
    #[serde(default)]
    limit: Option<u32>,
}

/// Params of `output.wait_for`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OutputWaitParams {
    /// Session directory.
    session: ExactPath,
    /// Exact bytes to wait for.
    pattern: ByteString,
    /// Bound on waiting; the default wait timeout when absent.
    #[serde(default)]
    timeout_ms: Option<u64>,
}

/// Params of `events.page`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EventsPageParams {
    /// Session directory.
    session: ExactPath,
    /// Position returned by the previous page; the start when absent.
    #[serde(default)]
    cursor: Option<EventPageCursor>,
    /// Maximum events to return; 100 when absent.
    #[serde(default)]
    limit: Option<u16>,
    /// Whether to leave out event payloads.
    #[serde(default)]
    metadata_only: bool,
    /// Event types to keep; every type when empty.
    #[serde(default)]
    event_types: Vec<String>,
}

/// Params of `events.follow`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EventsFollowParams {
    /// Session directory.
    session: ExactPath,
    /// Event sequence to reach; the start when absent.
    #[serde(default)]
    next_event_seq: Option<u64>,
}

/// Params of `analysis.timeline`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TimelineParams {
    /// Session directory.
    session: ExactPath,
    /// Index of the first entry.
    #[serde(default)]
    offset: u64,
    /// Maximum entries to return; 100 when absent.
    #[serde(default)]
    limit: Option<u16>,
    /// Whether to include states that were never presented.
    #[serde(default)]
    include_hidden: bool,
    /// Whether to include control entries.
    #[serde(default)]
    controls: bool,
    /// Session-monotonic range to keep, in nanoseconds.
    #[serde(default)]
    range: Option<SourceRange>,
}

/// Params of `analysis.diff`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DiffParams {
    /// Session directory.
    session: ExactPath,
    /// Earlier state.
    from: StatePoint,
    /// Later state.
    to: StatePoint,
}

/// Params of `analysis.explain`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExplainParams {
    /// Session directory.
    session: ExactPath,
    /// State to explain; the latest presented state when absent.
    #[serde(default)]
    target: Option<StatePoint>,
    /// Events of context around the state; 8 when absent.
    #[serde(default)]
    context: Option<u64>,
}

/// Params of `analysis.diagnose`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DiagnoseParams {
    /// Session directory.
    session: ExactPath,
    /// Checks to run; every check when empty.
    #[serde(default)]
    checks: Vec<DiagnosticCheck>,
    /// Changed-cell percent that counts as a broad change; 50 when absent.
    #[serde(default)]
    broad_change_percent: Option<u8>,
}

/// Params of `analysis.last_action`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LastActionParams {
    /// Session directory.
    session: ExactPath,
    /// Changed-cell percent that counts as a broad change; 50 when absent.
    #[serde(default)]
    broad_change_percent: Option<u8>,
}

/// Params of `analysis.markers`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MarkersParams {
    /// Session directory.
    session: ExactPath,
    /// Frame rate of the nominal frame numbers; 30 when absent.
    #[serde(default)]
    fps: Option<u16>,
}

/// Params of `analysis.review_search`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReviewSearchParams {
    /// Session directory.
    session: ExactPath,
    /// Text to search for.
    query: String,
}

/// Params of `analysis.review_export`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReviewExportParams {
    /// Session directory.
    session: ExactPath,
    /// State to export.
    state_id: u64,
    /// New directory to write; an existing one is never replaced.
    destination: ExactPath,
}

/// Params of `evidence.save`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SaveParams {
    /// Session directory.
    session: ExactPath,
    /// Destination path.
    destination: ExactPath,
    /// Representations to write.
    formats: Vec<SaveFormat>,
    /// State to save; the latest presented state when absent.
    #[serde(default)]
    point: Option<StatePoint>,
    /// Whether a hidden state may be selected.
    #[serde(default)]
    include_hidden: bool,
}

/// Params of `evidence.bundle` and `evidence.verify_bundle`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BundleParams {
    /// Session directory.
    session: ExactPath,
    /// Bundle directory.
    bundle: ExactPath,
    /// Bundle profile.
    profile: BundleProfile,
}

/// Params of calls that write one new path from a session.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExportParams {
    /// Session directory.
    session: ExactPath,
    /// Destination path.
    destination: ExactPath,
}

/// Params of `integrity.verify`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct VerifyParams {
    /// Session directory.
    session: ExactPath,
    /// Verification depth; content when absent.
    #[serde(default)]
    level: Option<VerificationLevel>,
    /// Allowed-signers file that decides trust.
    #[serde(default)]
    allowed_signers: Option<ExactPath>,
}

/// Params of `integrity.sign`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SignParams {
    /// Session directory.
    session: ExactPath,
    /// SSH private key.
    key: ExactPath,
    /// Signer identity written into the receipt.
    signer: String,
}

pub(super) fn register(registry: &mut Registry) {
    method(
        registry,
        "output.page",
        "Read one page of raw output bytes.",
        output_page,
    );
    method(
        registry,
        "output.wait_for",
        "Wait until the raw output contains exact bytes.",
        output_wait,
    );
    method(
        registry,
        "events.page",
        "Read one page of canonical events.",
        events_page,
    );
    method(
        registry,
        "events.follow",
        "Wait until the event log reaches a sequence and report its position.",
        events_follow,
    );
    method(
        registry,
        "analysis.timeline",
        "Read one timeline page.",
        timeline,
    );
    method(registry, "analysis.diff", "Compare two states.", diff);
    method(
        registry,
        "analysis.explain",
        "Explain one state from its source.",
        explain,
    );
    method(
        registry,
        "analysis.diagnose",
        "Run conservative diagnostics.",
        diagnose,
    );
    method(
        registry,
        "analysis.last_action",
        "Triage the most recent action.",
        last_action,
    );
    method(
        registry,
        "analysis.markers",
        "List markers and their nominal frames.",
        markers,
    );
    method(
        registry,
        "analysis.cache_status",
        "Inventory the replay cache.",
        cache_status,
    );
    method(
        registry,
        "analysis.cache_prune",
        "Remove disposable replay checkpoints.",
        cache_prune,
    );
    method(
        registry,
        "analysis.review_summary",
        "Summarize the session for review.",
        review_summary,
    );
    method(
        registry,
        "analysis.review_search",
        "Search presented screens.",
        review_search,
    );
    method(
        registry,
        "analysis.review_export",
        "Export one reviewed state.",
        review_export,
    );
    method(
        registry,
        "evidence.save",
        "Save representations of one state.",
        save,
    );
    method(
        registry,
        "evidence.bundle",
        "Create an evidence bundle.",
        bundle,
    );
    method(
        registry,
        "evidence.verify_bundle",
        "Verify a bundle against its session and profile.",
        verify_bundle,
    );
    method(
        registry,
        "evidence.asciicast",
        "Export an asciicast recording.",
        asciicast,
    );
    method(
        registry,
        "integrity.verify",
        "Verify the session evidence.",
        |_call, params: VerifyParams| return Ok(verify(&params)),
    );
    method(
        registry,
        "integrity.sign",
        "Sign a verification receipt.",
        sign,
    );
    method(
        registry,
        "integrity.recover",
        "Recover the valid prefix into a new session directory.",
        |_call, params: ExportParams| return recover(&params, false),
    );
    method(
        registry,
        "integrity.recover_log",
        "Recover the valid prefix of the event log into a new log.",
        |_call, params: ExportParams| return recover(&params, true),
    );
}

fn sdk<T>(result: termtrace_sdk::Result<T>) -> Result<T, Failure> {
    return result.map_err(|error| return Failure::sdk(&error));
}

fn path(value: &ExactPath) -> &Path {
    return &value.0;
}

fn output_page(_call: &Call<'_>, params: OutputPageParams) -> Result<ByteOutputPage, Failure> {
    let limit = params.limit.map_or(65_536, |limit| return limit as usize);

    return sdk(OutputService::page(
        path(&params.session),
        params.cursor,
        limit,
    ));
}

fn output_wait(_call: &Call<'_>, params: OutputWaitParams) -> Result<WaitOutcome, Failure> {
    let timeout = millis(params.timeout_ms, defaults::WAIT_TIMEOUT);

    return sdk(OutputService::wait_for_bytes(
        path(&params.session),
        params.pattern.as_bytes(),
        timeout,
    ));
}

fn events_page(_call: &Call<'_>, params: EventsPageParams) -> Result<ProjectedEventPage, Failure> {
    return sdk(EventsService::page(
        path(&params.session),
        params.cursor,
        params.limit.unwrap_or(100),
        params.metadata_only,
        &params.event_types,
    ));
}

fn events_follow(
    call: &Call<'_>,
    params: EventsFollowParams,
) -> Result<EventFollowPosition, Failure> {
    let session = sdk(call.terminal()?.attach(path(&params.session)))?;
    let position = sdk(session.events().follow(params.next_event_seq.unwrap_or(0)))?;

    return Ok(position.into());
}

fn timeline(_call: &Call<'_>, params: TimelineParams) -> Result<TimelinePage, Failure> {
    return sdk(core::timeline(
        path(&params.session),
        params.offset,
        params.limit.unwrap_or(100),
        params.include_hidden,
        params.controls,
        params.range,
    ));
}

fn diff(_call: &Call<'_>, params: DiffParams) -> Result<SemanticDiff, Failure> {
    return sdk(core::diff(path(&params.session), params.from, params.to));
}

fn explain(_call: &Call<'_>, params: ExplainParams) -> Result<ExplainReport, Failure> {
    return sdk(core::explain(
        path(&params.session),
        params.target.unwrap_or(StatePoint::LatestPresented),
        params.context.unwrap_or(8),
    ));
}

fn diagnose(_call: &Call<'_>, params: DiagnoseParams) -> Result<DiagnoseReport, Failure> {
    return sdk(core::diagnose(
        path(&params.session),
        params.checks,
        params.broad_change_percent.unwrap_or(50),
    ));
}

fn last_action(_call: &Call<'_>, params: LastActionParams) -> Result<LastActionAnalysis, Failure> {
    return sdk(core::last_action(
        path(&params.session),
        params.broad_change_percent.unwrap_or(50),
    ));
}

fn markers(_call: &Call<'_>, params: MarkersParams) -> Result<Vec<MarkerRecord>, Failure> {
    return sdk(core::markers(
        path(&params.session),
        params.fps.unwrap_or(30),
    ));
}

fn cache_status(_call: &Call<'_>, params: SessionParams) -> Result<ReplayCacheStatus, Failure> {
    return sdk(core::cache_status(path(&params.session)));
}

fn cache_prune(_call: &Call<'_>, params: SessionParams) -> Result<ReplayCachePruneReport, Failure> {
    return sdk(core::cache_prune(path(&params.session)));
}

fn review_summary(_call: &Call<'_>, params: SessionParams) -> Result<ReviewSummary, Failure> {
    return sdk(core::review_summary(path(&params.session)));
}

fn review_search(
    _call: &Call<'_>,
    params: ReviewSearchParams,
) -> Result<ReviewSearchResult, Failure> {
    return sdk(core::review_search(path(&params.session), &params.query));
}

fn review_export(
    _call: &Call<'_>,
    params: ReviewExportParams,
) -> Result<ReviewExportManifest, Failure> {
    return sdk(core::review_export(
        path(&params.session),
        params.state_id,
        path(&params.destination),
    ));
}

fn save(_call: &Call<'_>, params: SaveParams) -> Result<SaveOutcome, Failure> {
    return sdk(core::save(
        path(&params.session),
        path(&params.destination),
        params.point.unwrap_or(StatePoint::LatestPresented),
        &params.formats,
        params.include_hidden,
    ));
}

fn bundle(_call: &Call<'_>, params: BundleParams) -> Result<BundleManifest, Failure> {
    return sdk(core::bundle(
        path(&params.session),
        path(&params.bundle),
        params.profile,
    ));
}

fn verify_bundle(_call: &Call<'_>, params: BundleParams) -> Result<BundleManifest, Failure> {
    return sdk(core::verify_bundle(
        path(&params.bundle),
        path(&params.session),
        params.profile,
    ));
}

fn asciicast(_call: &Call<'_>, params: ExportParams) -> Result<ExactPath, Failure> {
    sdk(ArtifactService::export_asciicast(
        path(&params.session),
        path(&params.destination),
    ))?;
    return Ok(params.destination);
}

fn verify(params: &VerifyParams) -> VerificationReport {
    return core::verify(
        path(&params.session),
        params.level.unwrap_or(VerificationLevel::Content),
        params.allowed_signers.as_ref().map(path),
    );
}

fn sign(_call: &Call<'_>, params: SignParams) -> Result<ExactPath, Failure> {
    return sdk(core::sign(
        path(&params.session),
        path(&params.key),
        &params.signer,
    ));
}

fn recover(params: &ExportParams, log: bool) -> Result<RecoveryReport, Failure> {
    return sdk(core::recover(
        path(&params.session),
        path(&params.destination),
        log,
    ));
}
