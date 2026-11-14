use super::locator::{spec, LocatorQuery};
use super::{method, millis, Registry};
use crate::failure::Failure;
use crate::server::Call;
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use termtrace_model::terminal::screen::{ScreenFrame, ScreenRegion, ScreenSnapshotOptions};
use termtrace_model::terminal::state::CursorState;
use termtrace_model::value::ExactPath;
use termtrace_sdk::assertions::{self as core, MaskedSnapshotOptions, SnapshotProfile};
use termtrace_sdk::locator::{QueryResult, ScreenFrameQuery as _, StyleFilter};
use termtrace_sdk::testing::{
    self as lifecycle, EvidencePolicy, FinalizationReport, OwnedSessionFinalization, TestOutcome,
};

const FINALIZATION_TIMEOUT: Duration = Duration::from_secs(10);

/// What an unmasked snapshot includes.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SnapshotSettings {
    /// Detail of the snapshot; `text` when absent.
    #[serde(default)]
    profile: Option<SnapshotProfile>,
    /// Whether cursor facts are included.
    #[serde(default)]
    include_cursor: bool,
    /// Whether mode facts are included.
    #[serde(default)]
    include_modes: bool,
}

impl SnapshotSettings {
    fn split(&self) -> (SnapshotProfile, ScreenSnapshotOptions) {
        return (
            self.profile.unwrap_or(SnapshotProfile::Text),
            ScreenSnapshotOptions {
                include_cursor: self.include_cursor,
                include_modes: self.include_modes,
            },
        );
    }
}

/// Params of `testing.normalized_snapshot`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NormalizedSnapshotParams {
    /// Frame to normalize.
    frame: ScreenFrame,
    #[serde(flatten)]
    settings: SnapshotSettings,
}

/// Params of `testing.masked_snapshot`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MaskedSnapshotParams {
    /// Frame to normalize.
    frame: ScreenFrame,
    /// Masked regions and included facts; masks are not redaction.
    options: MaskedSnapshotOptions,
}

/// Params of `testing.assert_snapshot_file`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SnapshotFileParams {
    /// Frame to compare.
    frame: ScreenFrame,
    /// Baseline file.
    path: ExactPath,
    #[serde(flatten)]
    settings: SnapshotSettings,
    /// Whether to create or replace the baseline instead of comparing.
    #[serde(default)]
    update: bool,
}

/// Params of `testing.assert_masked_snapshot_file`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MaskedSnapshotFileParams {
    /// Frame to compare.
    frame: ScreenFrame,
    /// Baseline file.
    path: ExactPath,
    /// Masked regions and included facts; masks are not redaction.
    options: MaskedSnapshotOptions,
    /// Whether to create or replace the baseline instead of comparing.
    #[serde(default)]
    update: bool,
}

/// Params of assertions on one frame.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FrameAssertionParams {
    /// Frame to check.
    frame: ScreenFrame,
}

/// Params of `testing.assert_screen_text`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ScreenTextParams {
    /// Frame to check.
    frame: ScreenFrame,
    /// Literal text the presented rows must contain.
    expected: String,
}

/// Params of `testing.assert_cursor`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CursorParams {
    /// Frame to check.
    frame: ScreenFrame,
    /// Expected cursor.
    expected: CursorState,
}

/// Params of `testing.assert_neutral_region`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NeutralRegionParams {
    /// Frame to check.
    frame: ScreenFrame,
    /// Region that must hold only default-styled blanks.
    region: ScreenRegion,
}

/// Params of assertions on the matches of a locator in one frame.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct QueryParams {
    /// Frame the locator ran on.
    frame: ScreenFrame,
    /// Locator to evaluate.
    locator: LocatorQuery,
}

/// Params of `testing.assert_count`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CountParams {
    /// Frame the locator ran on.
    frame: ScreenFrame,
    /// Locator to evaluate.
    locator: LocatorQuery,
    /// Required match count.
    expected: usize,
}

/// Params of `testing.assert_match_text`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MatchTextParams {
    /// Frame the locator ran on.
    frame: ScreenFrame,
    /// Locator to evaluate.
    locator: LocatorQuery,
    /// Zero-based index of the match among every match.
    index: usize,
    /// Text the match must equal.
    expected: String,
}

/// Params of `testing.assert_match_style`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MatchStyleParams {
    /// Frame the locator ran on.
    frame: ScreenFrame,
    /// Locator to evaluate.
    locator: LocatorQuery,
    /// Zero-based index of the match among every match.
    index: usize,
    /// Appearance every covered cell must have.
    expected: StyleFilter,
}

/// Params of `testing.finalize`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FinalizeParams {
    /// Recording handles the test owns; each session directory must be inside `root`.
    recordings: Vec<String>,
    /// Evidence root of the test.
    root: ExactPath,
    /// How the test ended.
    outcome: TestOutcome,
    /// When the evidence is kept.
    #[serde(default)]
    policy: EvidencePolicy,
    /// Bound on stopping and verifying each recording; 10 seconds when absent.
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Empty {}

pub(super) fn register(registry: &mut Registry) {
    method(
        registry,
        "testing.normalized_snapshot",
        "Normalize a frame into a snapshot, without observing or retrying.",
        |_call, params: NormalizedSnapshotParams| {
            let (profile, options) = params.settings.split();
            return core::normalized_snapshot(&params.frame, profile, options)
                .map_err(Failure::assertion);
        },
    );
    method(
        registry,
        "testing.masked_snapshot",
        "Normalize a frame into a masked cells snapshot.",
        |_call, params: MaskedSnapshotParams| {
            return core::masked_snapshot(&params.frame, &params.options)
                .map_err(Failure::assertion);
        },
    );
    method(
        registry,
        "testing.assert_snapshot_file",
        "Compare a frame with a snapshot baseline file, or write it when updating.",
        |_call, params: SnapshotFileParams| {
            let (profile, options) = params.settings.split();
            return lifecycle::assert_snapshot_file(
                &params.frame,
                &params.path.0,
                profile,
                options,
                params.update,
            )
            .map_err(Failure::terminal_assertion);
        },
    );
    method(
        registry,
        "testing.assert_masked_snapshot_file",
        "Compare a frame with a masked snapshot baseline file, or write it when updating.",
        |_call, params: MaskedSnapshotFileParams| {
            return lifecycle::assert_masked_snapshot_file(
                &params.frame,
                &params.path.0,
                &params.options,
                params.update,
            )
            .map_err(Failure::terminal_assertion);
        },
    );
    method(
        registry,
        "testing.assert_screen_text",
        "Require literal text in the presented rows of a frame.",
        |_call, params: ScreenTextParams| {
            return core::assert_screen_text(&params.frame, &params.expected)
                .map_err(Failure::assertion);
        },
    );
    method(
        registry,
        "testing.assert_cursor",
        "Require the cursor of a frame.",
        |_call, params: CursorParams| {
            return core::assert_cursor(&params.frame, &params.expected).map_err(Failure::mismatch);
        },
    );
    method(
        registry,
        "testing.assert_no_wrapped_rows",
        "Require that no row of a frame soft-wraps.",
        |_call, params: FrameAssertionParams| {
            return core::assert_no_wrapped_rows(&params.frame).map_err(Failure::assertion);
        },
    );
    method(
        registry,
        "testing.assert_neutral_region",
        "Require that a region of a frame holds only default-styled blanks.",
        |_call, params: NeutralRegionParams| {
            return core::assert_neutral_region(&params.frame, params.region)
                .map_err(Failure::assertion);
        },
    );
    method(
        registry,
        "testing.assert_no_presented_transient",
        "Require that a frame was not presented mid-update.",
        |_call, params: FrameAssertionParams| {
            return core::assert_no_presented_transient(&params.frame).map_err(Failure::mismatch);
        },
    );
    method(
        registry,
        "testing.assert_exists",
        "Require at least one match of a locator in a frame.",
        |_call, params: QueryParams| {
            return core::assert_exists(&query(&params.frame, &params.locator)?)
                .map_err(Failure::mismatch);
        },
    );
    method(
        registry,
        "testing.assert_not_exists",
        "Require no match of a locator in a frame.",
        |_call, params: QueryParams| {
            return core::assert_not_exists(&query(&params.frame, &params.locator)?)
                .map_err(Failure::mismatch);
        },
    );
    method(
        registry,
        "testing.assert_count",
        "Require an exact match count of a locator in a frame.",
        |_call, params: CountParams| {
            return core::assert_count(&query(&params.frame, &params.locator)?, params.expected)
                .map_err(Failure::mismatch);
        },
    );
    method(
        registry,
        "testing.assert_match_text",
        "Require the text of one match of a locator in a frame.",
        |_call, params: MatchTextParams| {
            let result = query(&params.frame, &params.locator)?;
            let found = result
                .nth(params.index)
                .map_err(|error| return Failure::query(&error))?;
            return core::assert_match_text(found, &params.expected).map_err(Failure::mismatch);
        },
    );
    method(
        registry,
        "testing.assert_match_style",
        "Require the appearance of one match of a locator in a frame.",
        |_call, params: MatchStyleParams| {
            let result = query(&params.frame, &params.locator)?;
            let found = result
                .nth(params.index)
                .map_err(|error| return Failure::query(&error))?;
            return core::assert_match_style(found, &params.expected).map_err(Failure::assertion);
        },
    );
    method(
        registry,
        "testing.fail_negation",
        "Fail with the mismatch of a negated expectation that held.",
        |_call, _params: Empty| -> Result<(), Failure> {
            return Err(Failure::mismatch(core::negation_mismatch()));
        },
    );
    method(
        registry,
        "testing.finalize",
        "Stop, verify, and retain or discard the recordings a test owns.",
        finalize,
    );
}

fn query(frame: &ScreenFrame, locator: &LocatorQuery) -> Result<QueryResult, Failure> {
    return frame
        .query(spec(locator)?)
        .map_err(|error| return Failure::query(&error));
}

fn finalize(call: &Call<'_>, params: FinalizeParams) -> Result<FinalizationReport, Failure> {
    let root = params.root.0;
    let mut recordings = params
        .recordings
        .iter()
        .map(|handle| return call.server.recordings.get(handle))
        .collect::<Result<Vec<_>, _>>()?;

    if recordings
        .iter()
        .any(|recording| return !inside(recording.path(), &root))
    {
        return Err(Failure::snapshot_validation(
            "owned recording is outside the test root".to_owned(),
        ));
    }

    recordings.sort_by(|left, right| return left.path().cmp(right.path()));
    recordings.dedup_by(|left, right| return Arc::ptr_eq(left, right));
    let sessions = recordings
        .iter()
        .map(|recording| {
            return OwnedSessionFinalization::Open(Ok(recording.take_for_finalization()));
        })
        .collect();

    return Ok(lifecycle::finalize_owned_sessions(
        sessions,
        &root,
        params.outcome,
        params.policy,
        millis(params.timeout_ms, FINALIZATION_TIMEOUT),
        None,
    ));
}

fn inside(path: &Path, root: &Path) -> bool {
    return path.starts_with(root) && path != root;
}
