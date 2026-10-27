//! Typed diagnostics, evidence, registry, restart, and tmux operations for the native SDKs.
//!
//! Existing model and service operations own all semantics; these functions only validate
//! SDK arguments and compose results that have no single owning model type.

use crate::tmux::{
    CatalogPage, ServiceTracks, StopRequest as TmuxStopRequest, TmuxClient, TmuxService,
};
use crate::{
    analysis::AnalysisService, artifacts::ArtifactService, integrity::IntegrityService,
    registry::RegistryService, Error, Result,
};
use schemars::JsonSchema;
use serde::Serialize;
use std::{path::Path, time::Duration};
use termtrace_model::{
    artifact::{
        bundle::{BundleManifest, BundleProfile, BundleVerificationPolicy},
        presentation::MarkerRecord,
        save::{SaveFormat, SaveOutcome},
    },
    execution::{
        environment::EnvironmentSpec,
        event_log::RecoveryReport,
        session::{VerificationLevel, VerificationReport},
        tmux::TmuxRecordConfiguration,
    },
    registry::{
        GarbageCollectionReport, RegistryEntry, RegistryListEntry, RegistrySessionResolution,
        RestartConfiguration, RestartReport,
    },
    terminal::{
        analysis::{DiagnoseReport, DiagnosticCheck, ExplainReport, LastActionAnalysis},
        diff::SemanticDiff,
        observation::{ReplayCachePruneReport, ReplayCacheStatus, StatePoint},
        replay::SourceRange,
        reviewer::{ReviewExportManifest, ReviewSearchResult, ReviewSummary},
        timeline::TimelinePage,
    },
    value::ExactPath,
};
use termtrace_tmux::{
    artifact::{
        TmuxCatalogSummary, TmuxGapCatalogEntry, TmuxRecordingManifest, TmuxRecordingStart,
        TmuxStreamingBundleManifest, TmuxTrackCatalogEntry,
    },
    status::TmuxStatusReport,
    store::EventPage as TmuxEventPage,
};

fn invalid(reason: &str) -> Error {
    return Error::InvalidOperation {
        reason: reason.to_owned(),
        source: None,
    };
}

/// One aggregate history page with the manifest it belongs to.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub struct TmuxEvents {
    /// Aggregate recording manifest.
    pub manifest: TmuxRecordingManifest,
    /// Requested history page.
    pub page: TmuxEventPage,
}

/// One track or coverage-gap catalog page with its manifest and summary.
#[derive(Clone, Debug, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TmuxCatalog {
    /// Track incarnations.
    Tracks {
        /// Aggregate recording manifest.
        manifest: TmuxRecordingManifest,
        /// Catalog totals.
        summary: TmuxCatalogSummary,
        /// Requested page of track entries.
        entries: Vec<TmuxTrackCatalogEntry>,
    },
    /// Coverage intervals.
    Gaps {
        /// Aggregate recording manifest.
        manifest: TmuxRecordingManifest,
        /// Catalog totals.
        summary: TmuxCatalogSummary,
        /// Requested page of gap entries.
        entries: Vec<TmuxGapCatalogEntry>,
    },
}

/// Bounded timeline through the canonical analysis service.
/// # Errors
/// Rejects a limit outside 1..=4096 or an empty range, and propagates replay failures.
pub fn timeline(
    path: &Path,
    offset: u64,
    limit: u16,
    hidden: bool,
    controls: bool,
    range: Option<SourceRange>,
) -> Result<TimelinePage> {
    if !(1..=4096).contains(&limit)
        || range.is_some_and(|range| return range.start_monotonic_ns >= range.end_monotonic_ns)
    {
        return Err(invalid("invalid timeline limit or half-open range"));
    }
    return AnalysisService::timeline_page(path, hidden, controls, range, offset, limit);
}

/// Full semantic difference between two states.
/// # Errors
/// Propagates endpoint validation and replay failures.
pub fn diff(path: &Path, from: StatePoint, to: StatePoint) -> Result<SemanticDiff> {
    return AnalysisService::diff(path, from, to).map(|(diff, _, _)| return diff);
}

/// Source-bound VT explanation.
/// # Errors
/// Propagates replay and validation failures.
pub fn explain(path: &Path, target: StatePoint, context: u64) -> Result<ExplainReport> {
    return AnalysisService::explain(path, target, context);
}

/// Conservative diagnostics, including exact native evidence limitations.
/// # Errors
/// Rejects a change percent above 100 and propagates validation and replay failures.
pub fn diagnose(path: &Path, checks: Vec<DiagnosticCheck>, broad: u8) -> Result<DiagnoseReport> {
    if broad > 100 {
        return Err(invalid("broad change percent exceeds 100"));
    }
    return AnalysisService::diagnose(path, checks, broad);
}

/// Both last-action reports from one shared triage operation.
/// # Errors
/// Rejects a change percent above 100 and propagates absent-action and replay failures.
pub fn last_action(path: &Path, broad: u8) -> Result<LastActionAnalysis> {
    if broad > 100 {
        return Err(invalid("broad change percent exceeds 100"));
    }
    return AnalysisService::last_action(path, broad);
}

/// Marker positions and nominal frames.
/// # Errors
/// Rejects zero fps and propagates canonical evidence failures.
pub fn markers(path: &Path, fps: u16) -> Result<Vec<MarkerRecord>> {
    if fps == 0 {
        return Err(invalid("fps must be positive"));
    }
    return AnalysisService::markers(path, fps);
}

/// Cache inventory.
/// # Errors
/// Propagates filesystem failures.
pub fn cache_status(path: &Path) -> Result<ReplayCacheStatus> {
    return AnalysisService::cache_status(path);
}

/// Prune only disposable checkpoints; cancelling an await does not undo deletion.
/// # Errors
/// Propagates filesystem failures.
pub fn cache_prune(path: &Path) -> Result<ReplayCachePruneReport> {
    return AnalysisService::cache_prune(path);
}

/// Read-only forensic review summary.
/// # Errors
/// Propagates native-manifest and canonical validation failures.
pub fn review_summary(path: &Path) -> Result<ReviewSummary> {
    return AnalysisService::review_summary(path);
}

/// Semantic viewport search.
/// # Errors
/// Propagates replay and query failures.
pub fn review_search(path: &Path, query: &str) -> Result<ReviewSearchResult> {
    return AnalysisService::review_search(path, query);
}

/// No-replace reviewer export.
/// # Errors
/// Propagates replay and publication failures.
pub fn review_export(path: &Path, state: u64, destination: &Path) -> Result<ReviewExportManifest> {
    return AnalysisService::review_export_state(path, state, destination);
}

/// Atomic save of all requested representations and state-selector variants.
/// # Errors
/// Propagates state resolution, rendering, and publication failures.
pub fn save(
    path: &Path,
    destination: &Path,
    point: StatePoint,
    formats: &[SaveFormat],
    hidden: bool,
) -> Result<SaveOutcome> {
    return ArtifactService::save(path, destination, point, formats, hidden);
}

/// Explicit-profile evidence bundle.
/// # Errors
/// Propagates bundle failures.
pub fn bundle(path: &Path, destination: &Path, profile: BundleProfile) -> Result<BundleManifest> {
    return ArtifactService::create_bundle(path, destination, profile);
}

/// Verify against caller-owned explicit source and profile, never the bundle's self-claim.
/// # Errors
/// Propagates invalid, tampered, wrong-source, and wrong-profile failures.
pub fn verify_bundle(path: &Path, source: &Path, profile: BundleProfile) -> Result<BundleManifest> {
    return ArtifactService::verify_bundle(path, BundleVerificationPolicy::new(source, profile));
}

/// Verification validity, authenticity, and trust remain separate facts, including invalid reports.
#[must_use]
pub fn verify(path: &Path, level: VerificationLevel, trust: Option<&Path>) -> VerificationReport {
    return IntegrityService::verify(path, level, trust);
}

/// Sign an exact receipt; this never implies trust. Abandoning the await does not undo signing.
/// # Errors
/// Propagates verification, key, signing, and publication failures.
pub fn sign(path: &Path, key: &Path, signer: &str) -> Result<ExactPath> {
    return IntegrityService::sign(path, key, signer).map(ExactPath);
}

/// Recover into a separate destination without changing the source.
/// # Errors
/// Propagates invalid prefix and destination failures.
pub fn recover(path: &Path, destination: &Path, log: bool) -> Result<RecoveryReport> {
    if log {
        return IntegrityService::recover_log(path, destination);
    }
    return IntegrityService::recover_session(path, destination);
}

/// Registry discovery, including exact u128 timestamps.
/// # Errors
/// Propagates state-root errors; unhealthy rows retain diagnostics.
pub fn registry_list(registry: &RegistryService, all: bool) -> Result<Vec<RegistryListEntry>> {
    return registry.list(all);
}

/// Register an immutable identity under an alias.
/// # Errors
/// Propagates invalid aliases, collisions, security, and canonical identity errors.
pub fn registry_register(
    registry: &RegistryService,
    alias: &str,
    session: &Path,
) -> Result<RegistryEntry> {
    return registry.register(alias, session);
}

/// Resolve; only a missing alias record maps to `None`.
/// # Errors
/// Corruption, permissions, missing canonical evidence, and invalid aliases propagate.
pub fn registry_resolve(registry: &RegistryService, alias: &str) -> Result<Option<RegistryEntry>> {
    return registry.resolve_optional(alias);
}

/// Complete derived lifecycle, including pending and incompatible diagnostics.
/// # Errors
/// Registry-record failures propagate.
pub fn registry_lifecycle(
    registry: &RegistryService,
    alias: &str,
) -> Result<RegistrySessionResolution> {
    return registry.resolve_lifecycle(alias);
}

/// Remove only the alias; canonical evidence is preserved.
/// # Errors
/// Missing aliases, permissions, and invalid records propagate.
pub fn registry_remove(registry: &RegistryService, alias: &str) -> Result<RegistryEntry> {
    return registry.remove(alias);
}

/// Explicit retention, dry-run unless the caller chooses apply; cancellation is not rollback.
/// # Errors
/// Inspection and deletion failures propagate.
pub fn registry_gc(
    registry: &RegistryService,
    days: u64,
    keep: u64,
    quota: Option<u64>,
    apply: bool,
) -> Result<GarbageCollectionReport> {
    return registry.gc(days, keep, quota, apply);
}

/// Shared typed restart, preserving lineage, readiness, and alias CAS; no automatic retry.
/// # Errors
/// Reports retained predecessor, orphan, and log diagnostics on successor admission failure.
pub fn restart(
    registry: &RegistryService,
    binary: &Path,
    alias: &str,
    options: &RestartConfiguration,
    environment: Option<&EnvironmentSpec>,
    timeout: Duration,
) -> Result<RestartReport> {
    return registry.restart(binary, alias, options, environment, timeout);
}

/// Server status through the underlying tmux service.
/// # Errors
/// Propagates connection and malformed integration failures.
pub fn tmux_status(client: &TmuxClient) -> Result<TmuxStatusReport> {
    return TmuxService::status(client);
}

/// Start through the existing aggregate recorder using the shared canonical-track adapter.
/// # Errors
/// Propagates setup and track failures with the existing aggregate rollback rules.
pub fn tmux_record(
    tracks: &ServiceTracks,
    client: TmuxClient,
    options: TmuxRecordConfiguration,
) -> Result<TmuxRecordingStart> {
    return TmuxService::record_configured(tracks, client, options);
}

/// Aggregate lifecycle through the underlying tmux service.
/// # Errors
/// Propagates manifest, catalog, and identity errors.
pub fn tmux_recording(root: &Path, name: &str) -> Result<TmuxRecordingManifest> {
    return TmuxService::recording_status(root, name);
}

/// Stop canonical tracks, retaining the tmux service's repeated-stop and gap semantics.
/// # Errors
/// Track and identity failures propagate; prior effects are not rolled back.
pub fn tmux_stop(
    tracks: &ServiceTracks,
    name: &str,
    wait: bool,
    timeout: Duration,
) -> Result<TmuxRecordingManifest> {
    return TmuxService::stop(
        tracks,
        TmuxStopRequest {
            name,
            wait,
            timeout,
        },
    );
}

/// Canonical aggregate history page.
/// # Errors
/// Propagates manifest and journal errors and invalid paging bounds.
pub fn tmux_events(root: &Path, name: &str, from: u64, limit: u16) -> Result<TmuxEvents> {
    let (manifest, page) = TmuxService::events(root, name, from, limit)?;

    return Ok(TmuxEvents { manifest, page });
}

/// Typed track or gap catalog page.
/// # Errors
/// Propagates catalog, manifest, and paging failures.
pub fn tmux_catalog(
    root: &Path,
    name: &str,
    gaps: bool,
    from: u64,
    limit: u16,
    active: bool,
) -> Result<TmuxCatalog> {
    let (manifest, summary, page) = TmuxService::catalog(root, name, gaps, from, limit, active)?;

    return Ok(match page {
        CatalogPage::Tracks(entries) => TmuxCatalog::Tracks {
            manifest,
            summary,
            entries,
        },
        CatalogPage::Gaps(entries) => TmuxCatalog::Gaps {
            manifest,
            summary,
            entries,
        },
    });
}

/// Bundle or verify existing aggregate evidence using an explicit aggregate manifest.
/// # Errors
/// Propagates mismatched or incomplete source and publication or verification failures.
pub fn tmux_bundle(
    manifest: &Path,
    destination: &Path,
    verify: bool,
) -> Result<TmuxStreamingBundleManifest> {
    if verify {
        return TmuxService::verify_bundle(destination, manifest);
    }
    return TmuxService::bundle(manifest, destination);
}
