use super::super::{
    diagnose_session, diff_session_states_with_semantic_states, explain_session,
    validated_visual_manifests, visit_session_timeline_while, DiagnoseReport, DiagnosticCheck,
    ExplainReport, Result, SemanticDiff, Session, StatePoint, TimelineEntry,
};
use crate::analysis::AnalysisService as LocalAnalysisService;
use std::path::Path as StdPath;
use termtrace_model::artifact::presentation::MarkerRecord as ModelMarkerRecord;
use termtrace_model::terminal::analysis::LastActionAnalysis as ModelLastActionAnalysis;
use termtrace_model::terminal::observation::ReplayCachePruneReport as ModelReplayCachePruneReport;
use termtrace_model::terminal::observation::ReplayCacheStatus as ModelReplayCacheStatus;
use termtrace_model::terminal::replay::SourceRange as ModelSourceRange;
use termtrace_model::terminal::reviewer::ReviewExportManifest as ModelReviewExportManifest;
use termtrace_model::terminal::reviewer::ReviewSearchResult as ModelReviewSearchResult;
use termtrace_model::terminal::reviewer::ReviewSummary as ModelReviewSummary;
use termtrace_model::terminal::timeline::TimelinePage as ModelTimelinePage;

/// Completed semantic inspection and diagnosis operations.
pub struct Semantic<'session> {
    pub(in crate::client) session: &'session Session,
}

impl Semantic<'_> {
    /// Build the ordered semantic/canonical timeline.
    ///
    /// # Errors
    /// Returns an error when the session's canonical evidence cannot be read, validated, or
    /// replayed.
    pub fn timeline(&self, all_states: bool, controls: bool) -> Result<Vec<TimelineEntry>> {
        let mut entries = Vec::new();
        visit_session_timeline_while(self.session.path(), all_states, controls, None, |entry| {
            entries.push(entry);

            return Ok(true);
        })?;

        return Ok(entries);
    }

    /// Compare two complete semantic endpoints.
    ///
    /// # Errors
    /// Returns an error when either point cannot be resolved or the session's evidence cannot be
    /// validated or replayed.
    pub fn diff(&self, from: StatePoint, to: StatePoint) -> Result<SemanticDiff> {
        let (difference, _, _) =
            diff_session_states_with_semantic_states(self.session.path(), from, to)?;
        return Ok(difference);
    }

    /// Explain exact VT frames and transitions around one point.
    ///
    /// # Errors
    /// Returns an error when `point` cannot be resolved or the session's evidence cannot be
    /// validated or replayed.
    pub fn explain(&self, point: StatePoint, context_events: u64) -> Result<ExplainReport> {
        return Ok(explain_session(self.session.path(), point, context_events)?);
    }

    /// Run conservative source-bound detectors.
    ///
    /// # Errors
    /// Returns an error when native visual evidence fails validation or the session's evidence
    /// cannot be validated or replayed.
    pub fn diagnose(
        &self,
        checks: Vec<DiagnosticCheck>,
        broad_change_percent: u8,
    ) -> Result<DiagnoseReport> {
        let visual_evidence = validated_visual_manifests(self.session.path())?;

        return Ok(diagnose_session(
            self.session.path(),
            checks,
            broad_change_percent,
            &visual_evidence,
        )?);
    }
}

impl Semantic<'_> {
    /// Bounded canonical timeline with explicit half-open source range.
    /// # Errors
    /// Propagates range, paging, evidence and replay failures.
    pub fn timeline_page(
        &self,
        hidden: bool,
        controls: bool,
        range: Option<ModelSourceRange>,
        offset: u64,
        limit: u16,
    ) -> Result<ModelTimelinePage> {
        return LocalAnalysisService::timeline_page(
            self.session.path(),
            hidden,
            controls,
            range,
            offset,
            limit,
        );
    }

    /// Last-action triage through the shared service.
    /// # Errors
    /// Propagates absent-action, source validation and replay failures.
    pub fn last_action(&self, broad: u8) -> Result<ModelLastActionAnalysis> {
        return LocalAnalysisService::last_action(self.session.path(), broad);
    }

    /// Canonical marker positions and nominal frames.
    /// # Errors
    /// Propagates invalid fps and evidence failures.
    pub fn markers(&self, fps: u16) -> Result<Vec<ModelMarkerRecord>> {
        return LocalAnalysisService::markers(self.session.path(), fps);
    }

    /// Disposable cache inventory.
    /// # Errors
    /// Propagates filesystem failures.
    pub fn cache_status(&self) -> Result<ModelReplayCacheStatus> {
        return LocalAnalysisService::cache_status(self.session.path());
    }

    /// Explicit pruning; cancellation does not restore deleted checkpoints.
    /// # Errors
    /// Propagates filesystem failures.
    pub fn cache_prune(&self) -> Result<ModelReplayCachePruneReport> {
        return LocalAnalysisService::cache_prune(self.session.path());
    }

    /// Forensic reviewer summary with validated native relationships.
    /// # Errors
    /// Propagates validation and replay failures.
    pub fn review_summary(&self) -> Result<ModelReviewSummary> {
        return LocalAnalysisService::review_summary(self.session.path());
    }

    /// Search semantic viewport history.
    /// # Errors
    /// Propagates query and source failures.
    pub fn review_search(&self, query: &str) -> Result<ModelReviewSearchResult> {
        return LocalAnalysisService::review_search(self.session.path(), query);
    }

    /// No-replace review export of an exact semantic state.
    /// # Errors
    /// Propagates state resolution and publication failures.
    pub fn review_export(
        &self,
        state_id: u64,
        destination: &StdPath,
    ) -> Result<ModelReviewExportManifest> {
        return LocalAnalysisService::review_export_state(
            self.session.path(),
            state_id,
            destination,
        );
    }
}
