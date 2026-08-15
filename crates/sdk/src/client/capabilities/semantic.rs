use super::super::*;

/// Completed semantic inspection and diagnosis operations.
pub struct Semantic<'session> {
    pub(in crate::client) session: &'session Session,
}

impl Semantic<'_> {
    /// Inspect one truth-labeled state point.
    pub fn inspect(&self, point: StatePoint, include_hidden: bool) -> Result<Observation> {
        return observe_session_with_options(&self.session.path, point, include_hidden);
    }

    /// Build the ordered semantic/canonical timeline.
    pub fn timeline(&self, all_states: bool, controls: bool) -> Result<Vec<TimelineEntry>> {
        let log = self.completed_log()?;
        return Ok(build_timeline(&log, all_states, controls, None)?);
    }

    /// Compare two complete semantic endpoints.
    pub fn diff(&self, from: StatePoint, to: StatePoint) -> Result<SemanticDiff> {
        return Ok(diff_states(&self.completed_log()?, from, to)?);
    }

    /// Explain exact VT frames and transitions around one point.
    pub fn explain(&self, point: StatePoint, context_events: u64) -> Result<ExplainReport> {
        return Ok(explain(&self.completed_log()?, point, context_events)?);
    }

    /// Run conservative source-bound detectors.
    pub fn diagnose(
        &self,
        checks: Vec<DiagnosticCheck>,
        broad_change_percent: u8,
    ) -> Result<DiagnoseReport> {
        let log = self.completed_log()?;
        let visual_evidence = validated_visual_manifests(&self.session.path)?;
        return Ok(diagnose(
            &log,
            checks,
            broad_change_percent,
            &visual_evidence,
        )?);
    }

    fn completed_log(&self) -> Result<EventLog> {
        let log = EventLog::read(self.session.path.join("events.jsonl"))?;
        validate_completed_log(&self.session.path, &log)?;
        return Ok(log);
    }
}
