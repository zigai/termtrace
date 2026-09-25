use super::{
    human_action_summary, human_diagnostics_summary, human_last_action_summary,
    human_screen_summary,
};
use termtrace_model::{
    execution::event::OperationStatus,
    terminal::analysis::{
        ActionOutputSummary, ActionWarning, DiagnosticFinding, FindingConfidence,
        LastActionBoundary, LastActionDiagnosticsReport, LastActionReport,
    },
};
use uuid::Uuid;

fn boundary() -> LastActionBoundary {
    return LastActionBoundary {
        request_id: None,
        operation_ids: Vec::new(),
        atom_count: 1,
        request_event_start_seq: 1,
        request_event_end_seq: 4,
        intent_event_seq: 2,
        result_event_seq: 3,
        intended_bytes: 4,
        status: OperationStatus::Applied,
        bytes_written: 4,
        error_code: None,
    };
}

fn finding(code: &str) -> DiagnosticFinding {
    return DiagnosticFinding {
        code: code.to_owned(),
        confidence: FindingConfidence::High,
        state_ids: Vec::new(),
        token_ids: Vec::new(),
        facts: Vec::new(),
        native_correlations: Vec::new(),
    };
}

#[test]
fn human_screen_renders_text_sparse_and_diff_content() {
    let base = serde_json::json!({
        "type": "screen",
        "state": "current",
        "quality": "stable",
        "size": [80, 24],
    });

    let mut text = base.clone();
    text["screen"] = serde_json::json!("hello\nworld");
    let text = human_screen_summary(&text);
    assert!(text.contains("Viewport state current"));
    assert!(text.contains("hello\nworld"));

    let mut sparse = base.clone();
    sparse["screen"] = serde_json::json!({
        "encoding": "rows",
        "rows": [[3, "visible"]],
        "blank_rows_omitted": 23,
    });
    let sparse = human_screen_summary(&sparse);
    assert!(sparse.contains("Sparse viewport"));
    assert!(sparse.contains("row 3: visible"));

    let mut diff = base;
    diff["screen"] = serde_json::json!({
        "encoding": "diff_rows",
        "since": "prior",
        "rows": [[4, "changed"]],
    });
    let diff = human_screen_summary(&diff);
    assert!(diff.contains("Viewport diff from prior to current"));
    assert!(diff.contains("row 4: changed"));
}

#[test]
fn human_action_summary_reports_mutation_and_postcondition() {
    let action = human_action_summary(&serde_json::json!({
        "type": "action",
        "input": {"applied": "all", "atoms": 2, "bytes": 6},
        "postcondition": {"kind": "screen", "matched": true, "outcome": "stable"},
        "ended": false,
    }));
    assert!(action.contains("mutation applied=all, atoms=2, bytes=6"));
    assert!(action.contains("postcondition screen matched=true, outcome=stable"));
    assert!(action.contains("session=live"));
}

#[test]
fn human_last_action_and_diagnostics_summaries_report_counts() {
    let inspection = LastActionReport {
        format: "termtrace-last-action".to_owned(),
        version: 1,
        session_id: Uuid::nil(),
        source_final_event_hash: String::new(),
        action: boundary(),
        from_state_id: 1,
        to_state_id: 2,
        rows_changed: vec![1, 2],
        cells_changed: 7,
        cursor_movement: None,
        mode_changes: Vec::new(),
        output_generated: ActionOutputSummary {
            event_count: 1,
            byte_count: 4,
        },
        presentation_eligible: true,
        presentation_eligible_states: 1,
        warnings: vec![ActionWarning {
            code: "broad_change".to_owned(),
            confidence: FindingConfidence::Medium,
            facts: Vec::new(),
        }],
        warnings_truncated: false,
    };
    let inspect = human_last_action_summary(&inspection).unwrap_or_default();
    assert!(inspect.contains("status=applied, bytes=4"), "{inspect}");
    assert!(
        inspect.contains("changed=7 cells across 2 rows, warnings=1"),
        "{inspect}"
    );

    let mut diagnostics = LastActionDiagnosticsReport {
        format: "termtrace-last-action-diagnostics".to_owned(),
        version: 1,
        session_id: Uuid::nil(),
        source_final_event_hash: String::new(),
        action: boundary(),
        from_state_id: 1,
        to_state_id: 2,
        findings: Vec::new(),
        findings_truncated: false,
    };

    assert_eq!(
        human_diagnostics_summary(&diagnostics),
        "Diagnostics: no findings for the last action\n"
    );

    diagnostics.findings = vec![finding("stale_cells"), finding("cursor_jump")];

    assert!(
        human_diagnostics_summary(&diagnostics).contains("2 finding(s): stale_cells, cursor_jump")
    );
}
