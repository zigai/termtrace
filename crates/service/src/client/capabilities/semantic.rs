use super::super::{
    diagnose_session, diff_session_states_with_semantic_states, explain_session,
    validated_visual_manifests, visit_session_timeline_while, DiagnoseReport, DiagnosticCheck,
    ExplainReport, Result, SemanticDiff, Session, StatePoint, TimelineEntry,
};

/// Completed semantic inspection and diagnosis operations.
pub struct Semantic<'session> {
    pub(in crate::client) session: &'session Session,
}

impl Semantic<'_> {
    /// Build the ordered semantic/canonical timeline.
    pub fn timeline(&self, all_states: bool, controls: bool) -> Result<Vec<TimelineEntry>> {
        let mut entries = Vec::new();
        visit_session_timeline_while(self.session.path(), all_states, controls, None, |entry| {
            entries.push(entry);

            return Ok(true);
        })?;

        return Ok(entries);
    }

    /// Compare two complete semantic endpoints.
    pub fn diff(&self, from: StatePoint, to: StatePoint) -> Result<SemanticDiff> {
        let (difference, _, _) =
            diff_session_states_with_semantic_states(self.session.path(), from, to)?;

        return Ok(difference);
    }

    /// Explain exact VT frames and transitions around one point.
    pub fn explain(&self, point: StatePoint, context_events: u64) -> Result<ExplainReport> {
        return Ok(explain_session(self.session.path(), point, context_events)?);
    }

    /// Run conservative source-bound detectors.
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
