use super::{
    ACTION_RESPONSE_BYTES, MAX_AGENT_RESPONSE_BYTES, MAX_CAPTURE_BYTES, ResolvedResponseMode,
    ResponseField, ResponseState, START_RESPONSE_BYTES, STOP_RESPONSE_BYTES,
    WAIT_STATUS_RESPONSE_BYTES, bounded_agent_response, bounded_capture_projection,
    capture_raw_into, compact_success, compact_task_value, compact_value_with_paths,
    enforce_success_budget, human_screen_summary, human_summary, pagination_projection,
    response_size_without_screen, retry_for_applied,
};

#[test]
fn compact_fields_hide_forensic_tables_by_default() {
    let source = serde_json::json!({
        "state_hash": "abc",
        "cursor": { "column": 2, "row": 3 },
        "modes": { "dec_1049": true },
        "boundary": { "event_hash": "def", "next_event_seq": 9 },
        "observed_monotonic_ns": 42,
        "nullable": null,
    });
    let compact = compact_value_with_paths(source.clone(), &[], false).unwrap_or_default();
    assert_eq!(compact["state_hash"], "abc");
    assert!(compact.get("cursor").is_none());
    assert!(compact.get("modes").is_none());
    assert!(compact.get("boundary").is_none());
    assert!(compact.get("observed_monotonic_ns").is_none());
    assert!(compact.get("nullable").is_none());

    let fields = [
        ResponseField::Cursor,
        ResponseField::Modes,
        ResponseField::Boundary,
        ResponseField::Timing,
    ];
    let expanded = compact_value_with_paths(source, &fields, false).unwrap_or_default();
    assert!(expanded.get("cursor").is_some());
    assert!(expanded.get("modes").is_some());
    assert!(expanded.get("boundary").is_some());
    assert!(expanded.get("observed_monotonic_ns").is_some());
}

#[test]
fn oversized_agent_errors_keep_required_retry_contract() {
    let response = serde_json::json!({
        "v": 1,
        "ok": false,
        "type": "action",
        "code": "postcondition_failed",
        "applied": "all",
        "retry": "unsafe",
        "session_state": "live",
        "screen": "x".repeat(MAX_AGENT_RESPONSE_BYTES),
    });
    let encoded = bounded_agent_response(response).unwrap_or_default();
    assert!(encoded.len() <= MAX_AGENT_RESPONSE_BYTES);
    let compact: serde_json::Value =
        serde_json::from_slice(&encoded).unwrap_or(serde_json::Value::Null);
    assert_eq!(compact["ok"], false);
    assert_eq!(compact["applied"], "all");
    assert_eq!(compact["retry"], "unsafe");
    assert_eq!(compact["session_state"], "live");
    assert_eq!(compact["truncated"], true);
}
#[test]
fn oversized_agent_pages_keep_the_next_offset() {
    let response = serde_json::json!({
        "v": 1,
        "ok": true,
        "type": "events",
        "results": [
            {"kind": "event", "payload": "x".repeat(MAX_AGENT_RESPONSE_BYTES)},
            {
                "kind": "output_page",
                "page": {
                    "offset": 0,
                    "limit": 100,
                    "total": 250,
                    "returned": 100,
                    "truncated": true,
                    "next_offset": 100
                }
            }
        ]
    });
    let encoded = bounded_agent_response(response).unwrap_or_default();
    let compact: serde_json::Value =
        serde_json::from_slice(&encoded).unwrap_or(serde_json::Value::Null);
    assert_eq!(compact["truncated"], true);
    assert_eq!(compact["page"]["next_offset"], 100);
    assert_eq!(compact["page"]["returned"], 100);
}

#[test]
fn oversized_raw_pages_keep_the_trailing_resume_cursor() {
    let mut state = response_state("timeline", false, serde_json::Value::Null, Vec::new());
    let output = format!(
        "{}\npage: {{\"offset\":0,\"limit\":600,\"returned\":600,\"truncated\":true,\"next_offset\":580}}\n",
        "x".repeat(MAX_CAPTURE_BYTES)
    );
    capture_raw_into(&mut state, output.as_bytes());

    let encoded = bounded_agent_response(compact_success(&state)).expect("bounded agent response");
    let compact: serde_json::Value = serde_json::from_slice(&encoded).expect("agent response JSON");
    assert_eq!(compact["truncated"], true);
    assert_eq!(compact["page"]["next_offset"], 580);
    assert_eq!(compact["page"]["returned"], 600);
}

#[test]
fn binary_raw_output_is_base64_encoded_for_agents() {
    let mut state = response_state("output", false, serde_json::Value::Null, Vec::new());
    capture_raw_into(&mut state, b"\xff");

    let response = compact_success(&state);
    assert_eq!(response["output"]["encoding"], "base64");
    assert_eq!(response["output"]["data"], "/w==");
    assert_eq!(response["output"]["bytes"], 1);
    assert_eq!(response["output"]["truncated"], false);
}

#[test]
fn capture_budget_fallback_keeps_only_pagination_metadata() {
    let value = serde_json::json!({
        "kind": "timeline",
        "entries": ["x".repeat(MAX_CAPTURE_BYTES)],
        "output_page": {
            "offset": 40,
            "limit": 20,
            "returned": 20,
            "truncated": true,
            "next_offset": 60
        }
    });
    let projection = pagination_projection(&value).unwrap_or_default();
    assert_eq!(projection["kind"], "timeline");
    assert_eq!(projection["output_page"]["next_offset"], 60);
    assert!(projection.get("entries").is_none());
}

#[test]
fn oversized_capture_projection_keeps_state_identity_and_omits_bulk_state() {
    let value = serde_json::json!({
        "record": {
            "state_id": 13_000,
            "state_hash": "abc",
            "event_seq": 42,
            "presented": true,
            "delta": {
                "rows": ["x".repeat(MAX_CAPTURE_BYTES)]
            }
        },
        "state": {
            "grid": ["x".repeat(MAX_CAPTURE_BYTES)]
        }
    });
    let projection = bounded_capture_projection(&value).unwrap_or_default();
    assert_eq!(projection["record"]["state_id"], 13_000);
    assert_eq!(projection["record"]["state_hash"], "abc");
    assert_eq!(projection["record"]["event_seq"], 42);
    assert_eq!(projection["record"]["presented"], true);
    assert!(projection["record"].get("delta").is_none());
    assert!(projection.get("state").is_none());
}

#[test]
fn rich_wait_and_action_frames_keep_bounded_agent_identity_and_text() {
    let frame = serde_json::json!({
        "version": 1,
        "state_hash": "a".repeat(64),
        "frame_hash": "b".repeat(64),
        "presented": true,
        "capture_quality": "live",
        "geometry": { "cols": 80, "rows": 24 },
        "data": {
            "detail": "spans",
            "rows": [
                { "row": 0, "text": "READY" },
                { "row": 1, "text": "next" }
            ]
        }
    });
    let wait = serde_json::json!({
        "predicate": { "predicate": "contains", "text": "MISSING" },
        "matched": false,
        "quality": "deadline",
        "frame": frame,
    });
    let projected = bounded_capture_projection(&wait).expect("bounded wait projection");
    let compact = compact_task_value(projected, &[], "wait").expect("compact wait");
    assert_eq!(compact["scope"], "screen");
    assert_eq!(compact["matched"], false);
    assert_eq!(compact["state"], "a".repeat(64));
    assert_eq!(compact["screen"], "READY\nnext");

    let action = serde_json::json!({
        "input": {
            "applied": "all",
            "atoms": 2,
            "bytes": 4,
            "outcomes": [{ "large": "x".repeat(70_000) }]
        },
        "observation": frame,
        "postcondition": {
            "kind": "screen",
            "matched": true,
            "outcome": "matched_unsettled"
        }
    });
    let projected = bounded_capture_projection(&action).expect("bounded action projection");
    let compact = compact_task_value(projected, &[], "act").expect("compact action");
    assert_eq!(compact["input"]["applied"], "all");
    assert_eq!(compact["input"]["atoms"], 2);
    assert_eq!(compact["input"]["bytes"], 4);
    assert!(compact["input"].get("outcomes").is_none());
    assert_eq!(compact["observation"]["state"], "a".repeat(64));
    assert_eq!(compact["observation"]["screen"], "READY\nnext");
}

fn response_state(
    command_type: &str,
    mutating: bool,
    value: serde_json::Value,
    fields: Vec<ResponseField>,
) -> ResponseState {
    let captured_bytes = serde_json::to_vec(&value).expect("response fixture").len();
    return ResponseState {
        mode: ResolvedResponseMode::Agent,
        fields,
        command_type: command_type.to_owned(),
        mutating,
        values: vec![value],
        raw: Vec::new(),
        captured_bytes,
        omitted_items: 0,
    };
}

#[test]
fn task_projections_are_allowlisted_and_meet_default_byte_gates() {
    let start = response_state(
        "start",
        true,
        serde_json::json!({
            "kind": "started",
            "startup_state": "completed_success",
            "alias": "fast",
            "canonical_session": "/private/session",
            "canonical_session_display": "/private/session",
            "session_id": "018f47d2-7ad0-7000-8000-000000000000",
            "recorder_pid": 42,
            "recorder_process_start_ticks": 99,
            "stdout_log": "/private/stdout",
            "stderr_log": "/private/stderr",
            "ready": false,
            "completion": {
                "exit_code": 0,
                "signal_name": null,
                "event_count": 4,
                "final_event_hash": "a".repeat(64),
                "output_bytes": 12,
            },
            "postcondition": {
                "matched": true,
                "state": "b".repeat(64),
                "screen": "large viewport".repeat(100),
            },
        }),
        Vec::new(),
    );
    let start =
        enforce_success_budget(compact_success(&start), "start", &[]).expect("start budget");
    assert_eq!(start["session_state"], "ended");
    assert!(start.get("canonical_session").is_none());
    assert_eq!(start["path"], "/private/session");
    assert!(start.get("recorder_pid").is_none());
    assert!(start.get("ready").is_none());
    assert!(start.get("boundary").is_none());
    assert!(start["postcondition"].get("screen").is_none());
    assert!(response_size_without_screen(&start).expect("start size") <= START_RESPONSE_BYTES);

    let send = response_state(
        "send",
        true,
        serde_json::json!({
            "kind": "input_sequence_applied",
            "sequence_id": "018f47d2-7ad0-7000-8000-000000000001",
            "atom_count": 3,
            "bytes_written": 9,
            "pace_ns": 10,
            "boundary": {"event_hash": "c".repeat(64)},
        }),
        Vec::new(),
    );
    let send = enforce_success_budget(compact_success(&send), "send", &[]).expect("send budget");
    assert_eq!(send["atoms"], 3);
    assert_eq!(send["bytes"], 9);
    assert!(send.get("atom_count").is_none());
    assert!(send.get("pace_ns").is_none());
    assert!(response_size_without_screen(&send).expect("send size") <= ACTION_RESPONSE_BYTES);

    let act = response_state(
        "act",
        true,
        serde_json::json!({
            "type": "action",
            "request_id": "018f47d2-7ad0-7000-8000-000000000002",
            "input": {"applied": "all", "atoms": 1, "bytes": 1, "pace_ns": 0},
            "postcondition": {
                "kind": "screen",
                "matched": true,
                "outcome": "stable",
            },
            "observation": {
                "state": "d".repeat(64),
                "quality": "stable",
                "size": [80, 24],
                "screen": "viewport".repeat(100),
            },
            "ended": false,
        }),
        Vec::new(),
    );
    let act = enforce_success_budget(compact_success(&act), "act", &[]).expect("act budget");
    assert_eq!(act["input"]["applied"], "all");
    assert!(act.get("postcondition").is_none());
    assert!(act["observation"].get("screen").is_some());
    assert!(response_size_without_screen(&act).expect("act size") <= ACTION_RESPONSE_BYTES);

    for (command, mutating, limit, value) in [
        (
            "wait",
            false,
            WAIT_STATUS_RESPONSE_BYTES,
            serde_json::json!({
                "kind": "stable_snapshot",
                "quality": "completed",
                "state_hash": "e".repeat(64),
                "viewport_text": "large viewport".repeat(100),
                "geometry": {"cols": 80, "rows": 24},
                "modes": {"alternate_screen": false},
                "observed_monotonic_ns": 7,
                "boundary": {"event_hash": "f".repeat(64)},
            }),
        ),
        (
            "status",
            false,
            WAIT_STATUS_RESPONSE_BYTES,
            serde_json::json!({
                "kind": "status",
                "session": "app",
                "lifecycle": "live",
                "pid": 42,
                "boundary": {"event_hash": "f".repeat(64)},
            }),
        ),
        (
            "stop",
            true,
            STOP_RESPONSE_BYTES,
            serde_json::json!({
                "kind": "completed",
                "completion": {
                    "exit_code": 0,
                    "signal_name": null,
                    "event_count": 4,
                    "final_event_hash": "f".repeat(64),
                    "output_bytes": 12,
                },
            }),
        ),
    ] {
        let state = response_state(command, mutating, value, Vec::new());
        let response = enforce_success_budget(compact_success(&state), command, &[])
            .expect("task response budget");
        assert!(
            response_size_without_screen(&response).expect("task size") <= limit,
            "{command} response exceeded {limit} bytes: {response}"
        );
        let mut gated_response = response.clone();
        if command == "status" {
            assert_eq!(gated_response["pid"], 42);
            gated_response
                .as_object_mut()
                .expect("status response")
                .remove("pid");
        }
        assert!(!contains_forbidden_default_field(&gated_response));
        if command == "wait" {
            assert_eq!(response["session_state"], "ended");
            assert_eq!(response["kind"], "stable_snapshot");
            assert!(response.get("screen").is_some());
        }
        if command == "stop" {
            assert_eq!(response["kind"], "completed");
        }
    }
}

fn contains_forbidden_default_field(value: &serde_json::Value) -> bool {
    return match value {
        serde_json::Value::Array(values) => values.iter().any(contains_forbidden_default_field),
        serde_json::Value::Object(values) => {
            values.keys().any(|key| {
                return matches!(
                    key.as_str(),
                    "boundary"
                        | "boundaries"
                        | "modes"
                        | "terminal_modes"
                        | "timing"
                        | "pid"
                        | "recorder_pid"
                        | "recorder_process_start_ticks"
                        | "stdout_log"
                        | "stderr_log"
                        | "ready"
                );
            }) || values.values().any(contains_forbidden_default_field)
        }
        _ => false,
    };
}

#[test]
fn boundary_is_only_included_when_requested_and_partial_requires_inspection() {
    let value = serde_json::json!({
        "kind": "input_sequence_applied",
        "sequence_id": "018f47d2-7ad0-7000-8000-000000000003",
        "atom_count": 1,
        "bytes_written": 1,
        "boundary": {"event_hash": "a".repeat(64), "next_event_seq": 3},
    });
    let default = compact_success(&response_state("send", true, value.clone(), Vec::new()));
    assert!(default.get("boundary").is_none());
    let requested = compact_success(&response_state(
        "send",
        true,
        value,
        vec![ResponseField::Boundary],
    ));
    assert!(requested.get("boundary").is_some());
    assert_eq!(retry_for_applied("none"), "safe_new_request");
    assert_eq!(retry_for_applied("unknown"), "same_request_id_only");
    assert_eq!(retry_for_applied("partial"), "inspect_before_retry");
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
fn human_task_summaries_preserve_status_findings_mutation_and_paths() {
    let status = human_summary(&serde_json::json!({"kind": "status", "pid": 42}), "status");
    assert_eq!(status, "Status: session=live\n");

    let action = human_summary(
        &serde_json::json!({
            "type": "action",
            "input": {"applied": "all", "atoms": 2, "bytes": 6},
            "postcondition": {"kind": "screen", "matched": true, "outcome": "stable"},
            "ended": false,
        }),
        "act",
    );
    assert!(action.contains("mutation applied=all, atoms=2, bytes=6"));
    assert!(action.contains("postcondition screen matched=true, outcome=stable"));

    let inspect = human_summary(
        &serde_json::json!({
            "format": "termtrace-last-action",
            "action": {"status": "applied", "bytes_written": 4},
            "cells_changed": 7,
            "rows_changed": [1, 2],
            "warnings": [{"code": "broad_change"}],
        }),
        "inspect",
    );
    assert!(inspect.contains("status=applied"));
    assert!(inspect.contains("changed=7 cells across 2 rows, warnings=1"));

    let diagnose = human_summary(
        &serde_json::json!({
            "format": "termtrace-last-action-diagnostics",
            "findings": [{"code": "stale_cells"}, {"code": "cursor_jump"}],
        }),
        "diagnose",
    );
    assert!(diagnose.contains("2 finding(s): stale_cells, cursor_jump"));

    let screenshot = human_summary(
        &serde_json::json!({
            "kind": "semantic_screenshot",
            "file": "/tmp/screen.png",
            "state_hash": "abc",
        }),
        "screenshot",
    );
    assert!(screenshot.contains("/tmp/screen.png"));
    assert!(screenshot.contains("state abc"));

    let bundle = human_summary(
        &serde_json::json!({"kind": "bundle", "directory": "/tmp/report"}),
        "bundle",
    );
    assert_eq!(bundle, "Bundle written to /tmp/report\n");
    let bug_report = human_summary(
        &serde_json::json!({"kind": "bug_report", "directory": "/tmp/bug"}),
        "bug-report",
    );
    assert_eq!(bug_report, "Bug report written to /tmp/bug\n");
    let video = human_summary(
        &serde_json::json!({
            "kind": "video",
            "output_display": "/tmp/demo.mp4",
            "renderer": "semantic",
            "fps": 30,
            "width": 1280,
            "height": 720,
        }),
        "video",
    );
    assert_eq!(
        video,
        "Video written to /tmp/demo.mp4 (1280×720, 30 FPS, semantic)\n"
    );
}
