//! Replay analysis, timeline, semantic diff, diagnostics, markers, and review services.

use crate::error::{Error, Result};
use std::path::Path;
use termtrace_engine::{
    analysis::{
        diagnostics::{diagnose_session, explain_session},
        last_action::analyze_session,
        reviewer::{
            export_state as export_review_state, search as search_review,
            summarize as summarize_review,
        },
    },
    diff::diff_session_states_with_semantic_states,
    observation::{
        prune_replay_cache, replay_cache_status, ReplayCachePruneReport, ReplayCacheStatus,
    },
    timeline::{build_session_timeline_page, SourceRange},
};
use termtrace_media::{
    native_capture::validated_visual_manifests,
    presentation::{markers as list_markers, MarkerRecord},
};
use termtrace_model::terminal::{
    analysis::{DiagnoseReport, DiagnosticCheck, ExplainReport, LastActionAnalysis},
    diff::SemanticDiff,
    observation::StatePoint,
    reviewer::{ReviewExportManifest, ReviewSearchResult, ReviewSummary},
    state::SemanticState,
    timeline::TimelinePage,
};

/// Service orchestrating session replay, forensics, diffing, and diagnostics.
pub struct AnalysisService;

impl AnalysisService {
    /// Compare two semantic states in a canonical session.
    ///
    /// # Errors
    /// Returns an error if either state point cannot be resolved or decoded.
    pub fn diff(
        session: &Path,
        from: StatePoint,
        to: StatePoint,
    ) -> Result<(SemanticDiff, SemanticState, SemanticState)> {
        return diff_session_states_with_semantic_states(session, from, to).map_err(Error::from);
    }

    /// Explain VT and semantic evidence around a target state point.
    ///
    /// # Errors
    /// Returns an error if the state cannot be resolved or explained.
    pub fn explain(session: &Path, target: StatePoint, context: u64) -> Result<ExplainReport> {
        return explain_session(session, target, context).map_err(Error::from);
    }

    /// Run conservative evidence-backed rendering diagnostics.
    ///
    /// # Errors
    /// Returns an error if evidence cannot be validated or diagnosed.
    pub fn diagnose(
        session: &Path,
        check: Vec<DiagnosticCheck>,
        broad_change_percent: u8,
    ) -> Result<DiagnoseReport> {
        let visual = validated_visual_manifests(session).map_err(Error::from)?;

        return diagnose_session(session, check, broad_change_percent, &visual)
            .map_err(Error::from);
    }

    /// Perform full triage analysis on the most recent canonical action.
    ///
    /// # Errors
    /// Returns an error if last-action inspection fails.
    pub fn last_action(session: &Path, broad_change_percent: u8) -> Result<LastActionAnalysis> {
        return analyze_session(session, broad_change_percent).map_err(Error::from);
    }

    /// List all canonical marker positions and nominal source frames.
    ///
    /// # Errors
    /// Returns an error if markers cannot be read.
    pub fn markers(session: &Path, fps: u16) -> Result<Vec<MarkerRecord>> {
        return list_markers(session, fps).map_err(Error::from);
    }

    /// Query replay cache status and inventory.
    ///
    /// # Errors
    /// Returns an error if cache inventory cannot be read.
    pub fn cache_status(session: &Path) -> Result<ReplayCacheStatus> {
        return replay_cache_status(session).map_err(Error::from);
    }

    /// Prune disposable replay checkpoints.
    ///
    /// # Errors
    /// Returns an error if cache pruning fails.
    pub fn cache_prune(session: &Path) -> Result<ReplayCachePruneReport> {
        return prune_replay_cache(session).map_err(Error::from);
    }

    /// Build a bounded page of timeline entries.
    ///
    /// # Errors
    /// Returns an error if timeline construction fails.
    pub fn timeline_page(
        session: &Path,
        include_hidden: bool,
        include_controls: bool,
        range: Option<SourceRange>,
        offset: u64,
        limit: u16,
    ) -> Result<TimelinePage> {
        return build_session_timeline_page(
            session,
            include_hidden,
            include_controls,
            range,
            offset,
            limit,
        )
        .map_err(Error::from);
    }

    /// Produce a read-only review summary of the session.
    ///
    /// # Errors
    /// Returns an error if summary construction fails.
    pub fn review_summary(session: &Path) -> Result<ReviewSummary> {
        let visual = validated_visual_manifests(session).map_err(Error::from)?;
        return summarize_review(session, &visual).map_err(Error::from);
    }

    /// Search for text across recorded semantic states in the reviewer index.
    ///
    /// # Errors
    /// Returns an error if search fails.
    pub fn review_search(session: &Path, query: &str) -> Result<ReviewSearchResult> {
        return search_review(session, query).map_err(Error::from);
    }

    /// Atomically export one semantic state and viewport text to a destination path.
    ///
    /// # Errors
    /// Returns an error if export fails.
    pub fn review_export_state(
        session: &Path,
        state_id: u64,
        destination: &Path,
    ) -> Result<ReviewExportManifest> {
        return export_review_state(session, state_id, destination).map_err(Error::from);
    }
}
