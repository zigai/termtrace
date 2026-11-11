//! Thin adapters for shared immutable assertions and owned-session finalization.

use crate::host::{index, millis, Arg, Model};
use crate::locator::{query_error, JsQueryResult, JsScreenMatch};
use crate::{blocking_failure, to_napi_err, typed_error, typed_error_with, JsFrame, JsRecording};
use napi::{
    bindgen_prelude::{ClassInstance, PromiseRaw},
    Env,
};
use napi_derive::napi;
use std::{path::Path, time::Duration};
use termtrace_model::protocol::mutation::MutationEffect;
use termtrace_model::terminal::{screen::ScreenSnapshotOptions, state::CursorState};
use termtrace_sdk::{assertions as core, testing as lifecycle};

#[derive(serde::Serialize)]
struct Mismatch {
    details: core::ExpectationMismatchError,
}

fn mismatch(error: core::ExpectationMismatchError) -> napi::Error {
    return typed_error_with(
        error.to_string(),
        "expectation_mismatch",
        "expectation_mismatch",
        MutationEffect::NotApplied,
        Mismatch { details: error },
    );
}

fn assertion(error: core::AssertionError) -> napi::Error {
    return match error {
        core::AssertionError::Mismatch(error) => mismatch(error),
        core::AssertionError::Query(error) => query_error(&error),
        core::AssertionError::Snapshot(error) => validation(error.to_string()),
    };
}

fn validation(message: String) -> napi::Error {
    return typed_error(
        message,
        "snapshot_validation",
        "snapshot_validation",
        MutationEffect::NotApplied,
    );
}

fn terminal_error(error: lifecycle::TerminalAssertionError) -> napi::Error {
    return match error {
        lifecycle::TerminalAssertionError::Assertion(error) => assertion(error),
        lifecycle::TerminalAssertionError::Operation(error) => to_napi_err(error),
        other => validation(other.to_string()),
    };
}

/// Options for existing unmasked baseline bytes. Updates are never implicit.
#[napi(object, object_to_js = false)]
pub struct JsSnapshotOptions {
    /// `text` (default), `spans`, or `cells`.
    pub profile: Option<Arg<core::SnapshotProfile>>,
    /// Include cursor facts (default false).
    pub include_cursor: Option<bool>,
    /// Include mode facts (default false).
    pub include_modes: Option<bool>,
    /// Explicitly create/replace a baseline (default false).
    pub update: Option<bool>,
}
fn snapshot_options(
    options: Option<JsSnapshotOptions>,
) -> (core::SnapshotProfile, ScreenSnapshotOptions, bool) {
    let options = options.unwrap_or(JsSnapshotOptions {
        profile: None,
        include_cursor: None,
        include_modes: None,
        update: None,
    });

    let profile = options
        .profile
        .map(|v| v.0)
        .unwrap_or(core::SnapshotProfile::Text);

    return (
        profile,
        ScreenSnapshotOptions {
            include_cursor: options.include_cursor.unwrap_or(false),
            include_modes: options.include_modes.unwrap_or(false),
        },
        options.update.unwrap_or(false),
    );
}

/// Report a successful positive expectation inside negation with shared diagnostics.
///
/// # Errors
/// Always returns a typed expectation mismatch.
#[napi]
pub fn fail_negation() -> napi::Result<()> {
    return Err(mismatch(core::negation_mismatch()));
}

/// Normalize this exact capture, without observing or retrying.
///
/// # Errors
/// Rejects invalid options and insufficient frame detail.
#[napi]
pub fn normalized_snapshot(
    frame: &JsFrame,
    options: Option<JsSnapshotOptions>,
) -> napi::Result<String> {
    let (profile, options, _) = snapshot_options(options);
    return core::normalized_snapshot(&frame.frame, profile, options).map_err(assertion);
}
/// Normalize a separate masked cells comparison. Masks are not redaction.
///
/// # Errors
/// Rejects invalid or split-wide masks and incomplete cell maps.
#[napi]
pub fn masked_snapshot(
    frame: &JsFrame,
    options: Arg<core::MaskedSnapshotOptions>,
) -> napi::Result<String> {
    let options = options.0;
    return core::masked_snapshot(&frame.frame, &options).map_err(assertion);
}
/// Compare/create an unmasked file baseline. Only `update: true` permits writes.
///
/// # Errors
/// Returns a mismatch for missing/different baselines, operational errors otherwise.
#[napi]
pub fn assert_snapshot_file(
    frame: &JsFrame,
    path: String,
    options: Option<JsSnapshotOptions>,
) -> napi::Result<()> {
    let (profile, options, update) = snapshot_options(options);
    return lifecycle::assert_snapshot_file(
        &frame.frame,
        Path::new(&path),
        profile,
        options,
        update,
    )
    .map_err(terminal_error);
}
/// Compare/create a separate masked file baseline. Masks are not redaction.
///
/// # Errors
/// Validates masks before writes; only comparison mismatches are negatable.
#[napi]
pub fn assert_masked_snapshot_file(
    frame: &JsFrame,
    path: String,
    options: Arg<core::MaskedSnapshotOptions>,
    update: Option<bool>,
) -> napi::Result<()> {
    let options = options.0;
    return lifecycle::assert_masked_snapshot_file(
        &frame.frame,
        Path::new(&path),
        &options,
        update.unwrap_or(false),
    )
    .map_err(terminal_error);
}

/// Shared Rust immutable `assert_screen_text` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only typed expectation mismatches or non-negatable validation/query errors.
#[napi]
pub fn assert_screen_text(frame: &JsFrame, expected: String) -> napi::Result<()> {
    return core::assert_screen_text(&frame.frame, &expected).map_err(assertion);
}

/// Shared Rust immutable `assert_cursor` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only typed expectation mismatches or non-negatable validation/query errors.
#[napi]
pub fn assert_cursor(frame: &JsFrame, expected: Arg<CursorState>) -> napi::Result<()> {
    let expected = expected.0;
    return core::assert_cursor(&frame.frame, &expected).map_err(mismatch);
}

/// Shared Rust immutable `assert_no_wrapped_rows` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only typed expectation mismatches or non-negatable validation/query errors.
#[napi]
pub fn assert_no_wrapped_rows(frame: &JsFrame) -> napi::Result<()> {
    return core::assert_no_wrapped_rows(&frame.frame).map_err(assertion);
}

/// Shared Rust immutable `assert_neutral_region` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only typed expectation mismatches or non-negatable validation/query errors.
#[napi]
pub fn assert_neutral_region(
    frame: &JsFrame,
    region: Arg<termtrace_sdk::ScreenRegion>,
) -> napi::Result<()> {
    let region = region.0;
    return core::assert_neutral_region(&frame.frame, region).map_err(assertion);
}

/// Shared Rust immutable `assert_no_presented_transient` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only typed expectation mismatches or non-negatable validation/query errors.
#[napi]
pub fn assert_no_presented_transient(frame: &JsFrame) -> napi::Result<()> {
    return core::assert_no_presented_transient(&frame.frame).map_err(mismatch);
}

/// Shared Rust immutable `assert_exists` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only typed expectation mismatches or non-negatable validation/query errors.
#[napi]
pub fn assert_exists(result: &JsQueryResult) -> napi::Result<()> {
    return core::assert_exists(&result.inner).map_err(mismatch);
}

/// Shared Rust immutable `assert_not_exists` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only typed expectation mismatches or non-negatable validation/query errors.
#[napi]
pub fn assert_not_exists(result: &JsQueryResult) -> napi::Result<()> {
    return core::assert_not_exists(&result.inner).map_err(mismatch);
}

/// Shared Rust immutable `assert_count` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only typed expectation mismatches or non-negatable validation/query errors.
#[napi]
pub fn assert_count(result: &JsQueryResult, expected: f64) -> napi::Result<()> {
    let expected = index(expected, "expected count")?;
    return core::assert_count(&result.inner, expected).map_err(mismatch);
}

/// Shared Rust immutable `assert_match_text` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only typed expectation mismatches or non-negatable validation/query errors.
#[napi]
pub fn assert_match_text(selected: &JsScreenMatch, expected: String) -> napi::Result<()> {
    return core::assert_match_text(&selected.inner, &expected).map_err(mismatch);
}

/// Shared Rust immutable `assert_match_style` expectation. No hidden capture or retry.
///
/// # Errors
/// Raises only typed expectation mismatches or non-negatable validation/query errors.
#[napi]
pub fn assert_match_style(
    selected: &JsScreenMatch,
    expected: Arg<termtrace_sdk::locator::StyleFilter>,
) -> napi::Result<()> {
    let expected = expected.0;
    return core::assert_match_style(&selected.inner, &expected).map_err(assertion);
}

/// Finalize owned recordings using the shared Rust lifecycle/verification/retention engine.
/// Adapters must seal launches and call this once; all sibling sessions are attempted.
///
/// # Errors
/// Rejects invalid outcomes, policies or timeouts before transferring session ownership.
#[napi]
pub fn finalize_recordings<'env>(
    env: &'env Env,
    recordings: Vec<ClassInstance<JsRecording>>,
    root: String,
    outcome: Arg<lifecycle::TestOutcome>,
    policy: Arg<lifecycle::EvidencePolicy>,
    timeout_ms: Option<f64>,
) -> napi::Result<PromiseRaw<'env, Model<lifecycle::FinalizationReport>>> {
    let recordings: Vec<JsRecording> = recordings
        .iter()
        .map(|rec| JsRecording::clone(rec))
        .collect();
    let outcome = outcome.0;
    let policy = policy.0;
    let timeout = millis(timeout_ms, Duration::from_secs(10))?;
    if recordings.iter().any(|rec| {
        !Path::new(&rec.path).starts_with(Path::new(&root))
            || Path::new(&rec.path) == Path::new(&root)
    }) {
        return Err(validation(
            "owned recording is outside the test root".to_owned(),
        ));
    }
    return env.spawn_future(async move {
        return tokio::task::spawn_blocking(move || {
            let mut recordings = recordings;
            recordings.sort_by(|left, right| left.path.cmp(&right.path));
            recordings.dedup_by(|left, right| left.path == right.path);
            // Lock cached reports in path order across the entire operation. Concurrent/layered
            // wrappers cannot take a second ownership, capture duplicate artifacts or finalize twice.
            let mut reports: Vec<_> = recordings
                .iter()
                .map(|rec| rec.finalization.lock().unwrap_or_else(|e| e.into_inner()))
                .collect();
            if let Some(first) = reports.first().and_then(|report| report.as_ref())
                && first.root == Path::new(&root)
                && first.policy == policy
                && reports.iter().all(|cached| cached.as_ref() == Some(first))
            {
                return Ok(Model(first.report.clone()));
            }
            let sessions = recordings
                .iter()
                .zip(&reports)
                .map(|(rec, cached)| {
                    if let Some(report) = cached.as_ref() {
                        return lifecycle::OwnedSessionFinalization::Finalized {
                            report: report.report.clone(),
                            path: rec.path.clone().into(),
                        };
                    }
                    let owned = rec.session.lock().unwrap_or_else(|e| e.into_inner()).take();
                    return lifecycle::OwnedSessionFinalization::Open(Ok(
                        owned.unwrap_or_else(|| rec.lifecycle.lifecycle_observer())
                    ));
                })
                .collect();
            let report = lifecycle::finalize_owned_sessions(
                sessions,
                Path::new(&root),
                outcome,
                policy,
                timeout,
                None,
            );
            for cached in &mut reports {
                **cached = Some(lifecycle::CachedFinalization {
                    root: root.clone().into(),
                    policy,
                    report: report.clone(),
                });
            }
            return Ok(Model(report));
        })
        .await
        .map_err(blocking_failure)?;
    });
}
