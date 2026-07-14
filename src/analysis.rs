//! Evidence-backed explanations of canonical semantic transitions.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use crate::{
    Error, Result,
    diff::inspect_point,
    event_log::EventLog,
    observation::StatePoint,
    replay::{StateRecord, replay},
    vt_framer::{VtToken, frame_event_log},
};

/// Conservative confidence label for a derived finding.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FindingConfidence {
    /// Directly encoded control and semantic outcome agree.
    High,
    /// Ordered facts support a candidate but do not prove user perception.
    Medium,
}

/// One deterministic evidence-backed diagnostic finding.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExplainFinding {
    /// Stable machine-readable finding code.
    pub code: String,
    /// Conservative confidence label.
    pub confidence: FindingConfidence,
    /// VT token IDs supporting the finding.
    pub token_ids: Vec<u64>,
    /// Exact neutral facts, without pixel-level inference.
    pub facts: Vec<String>,
}

/// Causal evidence around one canonical state point.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExplainReport {
    /// Canonical session identity.
    pub session_id: Uuid,
    /// Final canonical event hash anchoring this report's source.
    pub source_final_event_hash: String,
    /// Canonical event count anchoring this report's source.
    pub source_event_count: u64,
    /// Requested target selector.
    pub target: StatePoint,
    /// Resolved target semantic state.
    pub target_state: StateRecord,
    /// Inclusive event-sequence context radius.
    pub context_events: u64,
    /// Lossless tokens in the selected event context.
    pub tokens: Vec<VtToken>,
    /// Semantic transitions in the selected event context.
    pub states: Vec<StateRecord>,
    /// Conservative deterministic findings.
    pub findings: Vec<ExplainFinding>,
}

/// Explain exact VT frames and semantic transitions around one state point.
///
/// # Errors
///
/// Returns an error when the target cannot resolve or replay/framing fails.
pub fn explain(log: &EventLog, target: StatePoint, context_events: u64) -> Result<ExplainReport> {
    let target_inspection = inspect_point(log, &target)?;
    let target_event = target_inspection.record.event_seq.unwrap_or(0);
    let start_event = target_event.saturating_sub(context_events);
    let end_event = target_event.saturating_add(context_events);
    let tokens = frame_event_log(log)?
        .into_iter()
        .filter(|token| {
            return token.source.iter().any(|segment| {
                return segment.event_seq >= start_event && segment.event_seq <= end_event;
            });
        })
        .collect::<Vec<_>>();
    let states = replay(log)?
        .states
        .into_iter()
        .filter(|state| {
            return state
                .event_seq
                .is_none_or(|sequence| return sequence >= start_event && sequence <= end_event);
        })
        .collect::<Vec<_>>();
    let findings = findings(&tokens, &states);
    let source_event_count = u64::try_from(log.events.len()).unwrap_or(u64::MAX);
    let source_final_event_hash = match log.events.last() {
        Some(record) => record.hash.clone(),
        None => {
            let header = serde_json::to_vec(&log.header).map_err(|source| {
                return Error::InvalidJson {
                    path: PathBuf::from("<event-log-header>"),
                    json_path: "$".to_owned(),
                    source,
                };
            })?;
            blake3::hash(&header).to_hex().to_string()
        }
    };
    return Ok(ExplainReport {
        session_id: log.header.session_id,
        source_final_event_hash,
        source_event_count,
        target,
        target_state: target_inspection.record,
        context_events,
        tokens,
        states,
        findings,
    });
}

fn findings(tokens: &[VtToken], states: &[StateRecord]) -> Vec<ExplainFinding> {
    let mut findings = Vec::new();
    for (description, code, fact) in [
        (
            "erase display (all)",
            "full_viewport_clear",
            "ED 2 occurred in canonical output",
        ),
        (
            "erase saved lines",
            "clear_scrollback",
            "ED 3 occurred in canonical output",
        ),
    ] {
        let token_ids = tokens
            .iter()
            .filter(|token| return token.description == description)
            .map(|token| return token.token_id)
            .collect::<Vec<_>>();
        if !token_ids.is_empty() {
            findings.push(ExplainFinding {
                code: code.to_owned(),
                confidence: FindingConfidence::High,
                token_ids,
                facts: vec![fact.to_owned()],
            });
        }
    }
    let hidden = states
        .iter()
        .filter(|state| return !state.presented && !state.delta.rows.is_empty())
        .collect::<Vec<_>>();
    if !hidden.is_empty() {
        findings.push(ExplainFinding {
            code: "hidden_transient".to_owned(),
            confidence: FindingConfidence::High,
            token_ids: Vec::new(),
            facts: vec![format!(
                "{} cell-changing semantic states were not presentation eligible",
                hidden.len()
            )],
        });
    }
    let clear = findings
        .iter()
        .find(|finding| return finding.code == "full_viewport_clear");
    let printable_after_clear = clear.is_some_and(|finding| {
        let last_clear = finding.token_ids.iter().copied().max().unwrap_or(0);
        return tokens.iter().any(|token| {
            return token.token_id > last_clear
                && token.description == "printable bytes"
                && !token.raw.as_bytes().is_empty();
        });
    });
    if printable_after_clear {
        findings.push(ExplainFinding {
            code: "clear_then_repaint_candidate".to_owned(),
            confidence: FindingConfidence::Medium,
            token_ids: tokens
                .iter()
                .filter(|token| {
                    return token.description == "erase display (all)"
                        || token.description == "printable bytes";
                })
                .map(|token| return token.token_id)
                .collect(),
            facts: vec![
                "printable output followed a full semantic erase".to_owned(),
                "this does not establish that pixels flickered".to_owned(),
            ],
        });
    }
    return findings;
}
