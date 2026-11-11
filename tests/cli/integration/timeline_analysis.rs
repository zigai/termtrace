//! timeline analysis CLI integration tests.

use crate::support::*;
use std::sync::atomic::AtomicBool;
use termtrace_engine::analysis::diagnostics::{
    diagnose, diagnose_session, diagnose_snapshot, explain, explain_session,
};
use termtrace_engine::replay::visit_session_states_while;
use termtrace_engine::timeline::visit_snapshot_timeline_while;
use termtrace_engine::timeline::{build_session_timeline_page, build_timeline};
use termtrace_evidence::session::SessionSnapshot;
use termtrace_model::terminal::analysis::{DiagnosticCheck, OmittedFindings};

#[test]
fn streamed_diagnostic_detectors_match_materialized_reports() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("detectors");
    let mut record = Command::cargo_bin("termtrace").expect("binary");
    record.args(["record", "--no-input", "--out"]).arg(&session)
        .args(["--", "/bin/sh", "-c", "printf '\\033[?2026hhidden\\033[?2026l\\033[2J\\033[HAAA\\033[HBBB\\033[HAAA\\033[20;30Hjump\\033[3J\\033[?25l\\033[?25h\\033[?25l\\033[6n\\033[6n\\033[6n\\033]7400;x\\007\\033_Zcustom;y\\033\\\\'"])
        .assert().success();

    let log = EventLog::read(session.join("events.jsonl")).expect("canonical log");
    let mut selections = vec![Vec::new()];
    selections.extend(DiagnosticCheck::all().into_iter().map(|check| vec![check]));

    for checks in selections {
        let expected = diagnose(&log, checks.clone(), 1, &[]).expect("materialized diagnostics");
        let actual = diagnose_session(&session, checks, 1, &[]).expect("streamed diagnostics");
        assert_eq!(
            serde_json::to_value(actual).expect("streamed report"),
            serde_json::to_value(expected).expect("materialized report")
        );
    }

    for count in [0, log.events.len() / 2, log.events.len()] {
        let count = u64::try_from(count).expect("fixture count");
        let snapshot = SessionSnapshot::capture(&session, Some(count)).expect("finite snapshot");
        assert!(!snapshot.path().join("completion.json").exists());
        let prefix =
            EventLog::read_prefix(session.join("events.jsonl"), count).expect("legacy prefix");

        let expected =
            diagnose(&prefix, DiagnosticCheck::all(), 1, &[]).expect("legacy prefix diagnosis");
        let actual =
            diagnose_snapshot(&snapshot, DiagnosticCheck::all(), 1).expect("snapshot diagnosis");
        assert_eq!(
            serde_json::to_value(actual).expect("snapshot JSON"),
            serde_json::to_value(expected).expect("legacy JSON")
        );

        let expected = build_timeline(&prefix, true, true, None).expect("legacy prefix timeline");
        let mut actual = Vec::new();
        visit_snapshot_timeline_while(&snapshot, true, true, None, |entry| {
            actual.push(entry);

            return Ok(true);
        })
        .expect("snapshot timeline");

        assert_eq!(actual, expected);
    }
}

#[test]
fn streamed_diagnosis_checks_late_states_and_counts_unlisted_findings() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("long");
    let script = concat!(
        "printf '%*s' 3000 '' | tr ' ' X;",
        "i=0; while [ $i -lt 150 ]; do printf '\\033[2J'; i=$((i+1)); done;",
        "printf '\\033]7400;late\\007'",
    );

    let mut record = Command::cargo_bin("termtrace").expect("binary");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", script])
        .assert()
        .success();

    let mut final_state_id = 0_u64;
    visit_session_states_while(&session, &AtomicBool::new(false), |record, _| {
        final_state_id = record.state_id;

        return Ok(true);
    })
    .expect("replay states");

    assert!(final_state_id > 3000, "only {final_state_id} positions");

    let report = diagnose_session(
        &session,
        vec![DiagnosticCheck::Effects, DiagnosticCheck::FullRedraw],
        100,
        &[],
    )
    .expect("streamed diagnostics");

    let codes = report
        .findings
        .iter()
        .map(|finding| return finding.code.as_str())
        .collect::<Vec<_>>();
    assert!(codes.contains(&"ignored_control_strings"), "{codes:?}");
    assert_eq!(
        codes
            .iter()
            .filter(|code| return **code == "full_viewport_clear")
            .count(),
        100
    );
    assert!(report.omitted_findings.contains(&OmittedFindings {
        code: "full_viewport_clear".to_owned(),
        count: 50,
    }));
}

#[test]
fn terminal_effects_show_in_timeline_and_diagnose_without_payloads() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("effects");
    // `c2VjcmV0LXZhbHVl` is base64 for `secret-value`.
    let script = concat!(
        "printf '\\033]133;A\\007$ \\033]133;B\\007\\033]133;C\\007out\\033]133;D;3\\007';",
        "printf '\\033]52;c;c2VjcmV0LXZhbHVl\\007\\033]777;notify;Build;hunter2-body\\007';",
        "printf '\\033]7400;token=abc\\007\\033]7400;b\\007\\007'",
    );

    let mut record = Command::cargo_bin("termtrace").expect("binary");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", script])
        .assert()
        .success();

    let run = |arguments: &[&str]| {
        let mut command = Command::cargo_bin("termtrace").expect("binary");
        let output = command
            .args(arguments)
            .arg(&session)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        return String::from_utf8(output).expect("UTF-8 output");
    };
    let timeline = run(&["analyze", "timeline", "--all"]);
    let jsonl = run(&["analyze", "timeline", "--all", "--jsonl"]);
    let diagnose_text = run(&["analyze", "diagnose", "--check", "effects"]);
    let diagnose_json = run(&["analyze", "diagnose", "--check", "effects", "--json"]);

    for output in [&timeline, &jsonl, &diagnose_text, &diagnose_json] {
        for secret in [
            "secret-value",
            "c2VjcmV0LXZhbHVl",
            "hunter2",
            "aHVudGVyMi1ib2R5",
            "token=abc",
        ] {
            assert!(!output.contains(secret), "{secret} leaked into {output}");
        }
    }

    let effects = timeline
        .lines()
        .filter_map(|line| return line.split_once("effect ").map(|(_, effect)| return effect))
        .map(|effect| {
            return effect
                .split_once(" byte ")
                .map_or(effect, |(kind, _)| return kind);
        })
        .collect::<Vec<_>>();
    assert_eq!(
        effects,
        [
            "prompt_start",
            "input_start",
            "command_start",
            "command_end",
            "clipboard_write",
            "desktop_notification",
            "unknown_sequence",
            "unknown_sequence",
            "bell",
        ]
    );
    assert!(timeline.contains(" exit=3"));
    assert!(timeline.contains("target=standard data=[text/plain=12B:"));
    assert!(timeline.contains(r#"osc id="7400""#));

    let entries = jsonl
        .lines()
        .map(|line| return serde_json::from_str::<serde_json::Value>(line).expect("JSON line"))
        .filter(|entry| {
            return entry.pointer("/item/kind") == Some(&serde_json::json!("terminal_effect"));
        })
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), effects.len());
    assert!(entries.iter().all(|entry| {
        return entry["event_seq"].is_u64()
            && entry["monotonic_ns"].is_u64()
            && entry
                .pointer("/item/event_byte_offset")
                .is_some_and(serde_json::Value::is_u64);
    }));
    assert_eq!(
        entries[4].pointer("/item/effect/representations/0/data/byte_length"),
        Some(&serde_json::json!(12))
    );

    let report: serde_json::Value = serde_json::from_str(&diagnose_json).expect("diagnose JSON");
    let findings = report["findings"].as_array().expect("findings");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["code"], "ignored_control_strings");
    assert!(findings[0]["facts"][0]
        .as_str()
        .is_some_and(|fact| return fact.starts_with("the program sent 2 APC or OSC strings")));
    assert_eq!(findings[0]["facts"][1], "first kinds: OSC 7400");
    assert!(diagnose_text.contains("ignored_control_strings"));
}

#[test]
fn timeline_streams_presented_states_and_canonical_operations() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("timeline-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "printf '\\033[?2026hHIDDEN'; sleep 0.08; printf '\\033[?2026lVISIBLE'",
        ])
        .assert()
        .success();

    let mut concise = Command::cargo_bin("termtrace").expect("binary should build");
    let concise_output = concise
        .args(["analyze", "timeline"])
        .arg(&session)
        .arg("--jsonl")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let concise_entries = concise_output
        .split(|byte| return *byte == b'\n')
        .filter(|line| return !line.is_empty())
        .map(|line| {
            return serde_json::from_slice::<serde_json::Value>(line).expect("timeline JSON");
        })
        .collect::<Vec<_>>();
    assert!(concise_entries.iter().any(|entry| {
        return entry.pointer("/item/kind") == Some(&serde_json::json!("canonical_event"));
    }));
    assert!(concise_entries
        .iter()
        .filter(|entry| {
            return entry.pointer("/item/kind") == Some(&serde_json::json!("semantic_state"));
        })
        .all(|entry| return entry.pointer("/item/presented") == Some(&serde_json::json!(true))));

    let mut complete = Command::cargo_bin("termtrace").expect("binary should build");
    complete
        .args(["analyze", "timeline"])
        .arg(&session)
        .args(["--jsonl", "--all-states"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""presented":false"#));

    let mut controls = Command::cargo_bin("termtrace").expect("binary should build");
    controls
        .args(["analyze", "timeline"])
        .arg(&session)
        .args(["--jsonl", "--controls"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""kind":"vt_token""#))
        .stdout(predicate::str::contains("set synchronized output"))
        .stdout(predicate::str::contains("reset synchronized output"));

    let mut human = Command::cargo_bin("termtrace").expect("binary should build");
    let human = human
        .args(["analyze", "timeline"])
        .arg(&session)
        .args(["--controls", "--all-states"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let human = String::from_utf8(human).expect("timeline text");
    assert!(human.contains("lifecycle spawned pid="), "{human}");
    assert!(human.contains("exit code=0 signal=-"), "{human}");
    assert!(human.contains(" csi bytes="), "{human}");

    for leaked in ["Some(", "None", " Csi ", "Spawned"] {
        assert!(!human.contains(leaked), "leaked `{leaked}` in:\n{human}");
    }

    let log = EventLog::read(session.join("events.jsonl")).expect("comparison source");

    for (hidden, controls) in [(false, false), (false, true), (true, true)] {
        let expected = build_timeline(&log, hidden, controls, None).expect("reference timeline");
        let mut entries = Vec::new();
        let mut offset = 0;

        loop {
            let page = build_session_timeline_page(&session, hidden, controls, None, offset, 3)
                .expect("streamed page");
            entries.extend(page.entries);

            let Some(next) = page.next_offset else {
                break;
            };
            offset = next;
        }

        assert_eq!(entries, expected, "hidden={hidden}, controls={controls}");
    }
}

#[test]
fn agent_facing_event_and_timeline_defaults_are_bounded_and_resumable() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("paged-session");
    let binary = env!("CARGO_BIN_EXE_termtrace");
    let recorder = StdCommand::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; while :; do sleep .1; done",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn recorder");

    wait_for_control(&session);

    for index in 0..205 {
        send_request(
            &session,
            &ControlRequest::new(ControlCommand::Mark {
                label: format!("page-{index:03}"),
            }),
        )
        .expect("append page marker");
    }

    send_request(&session, &ControlRequest::new(ControlCommand::Stop)).expect("stop paged session");

    let recorded = recorder.wait_with_output().expect("wait for recorder");
    assert!(recorded.status.success(), "{recorded:?}");

    let mut timeline = Command::cargo_bin("termtrace").expect("binary should build");
    let timeline = timeline
        .args(["analyze", "timeline"])
        .arg(&session)
        .arg("--jsonl")
        .assert()
        .success();

    let timeline_lines = json_lines(&timeline.get_output().stdout);
    assert_eq!(timeline_lines.len(), 101);

    let timeline_page = timeline_lines.last().expect("timeline page marker");

    assert_eq!(timeline_page["kind"], "output_page");
    assert_eq!(timeline_page["page"]["returned"], 100);
    assert_eq!(timeline_page["page"]["next_offset"], 100);

    let mut events = Command::cargo_bin("termtrace").expect("binary should build");
    let events = events
        .args(["analyze", "events"])
        .arg(&session)
        .args(["--type", "marker", "--metadata-only"])
        .assert()
        .success();

    let event_lines = json_lines(&events.get_output().stdout);
    assert_eq!(event_lines.len(), 101);

    let event_page = event_lines.last().expect("event page marker");

    assert_eq!(event_page["kind"], "output_page");
    assert_eq!(event_page["page"]["total"], 205);
    assert_eq!(event_page["page"]["next_offset"], 100);
    assert!(event_page["page"]["next_event_seq"].is_u64());

    let mut resume = Command::cargo_bin("termtrace").expect("binary should build");
    let resume = resume
        .args(["analyze", "events"])
        .arg(&session)
        .args([
            "--type",
            "marker",
            "--metadata-only",
            "--offset",
            "200",
            "--limit",
            "10",
        ])
        .assert()
        .success();

    let resumed_lines = json_lines(&resume.get_output().stdout);
    assert_eq!(resumed_lines.len(), 6);
    assert_eq!(
        resumed_lines.last().expect("resumed page")["kind"],
        "output_page"
    );
    assert_eq!(
        resumed_lines.last().expect("resumed page")["page"]["returned"],
        5
    );
}

#[test]
fn diff_reports_complete_cell_changes_and_enforces_expectations() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("diff-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf 'A界'"])
        .assert()
        .success();

    let final_state = inspect_session(&session, StateSelector::Final)
        .expect("final state")
        .record
        .state_id;

    let mut changed = Command::cargo_bin("termtrace").expect("binary should build");
    changed
        .args(["analyze", "diff"])
        .arg(&session)
        .args([
            "--from-state",
            "0",
            "--to-state",
            &final_state.to_string(),
            "--format",
            "json-compact",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""changed":true"#))
        .stdout(predicate::str::contains(r#""grapheme":"A""#))
        .stdout(predicate::str::contains("界"));

    let mut visual = Command::cargo_bin("termtrace").expect("binary should build");
    let png = visual
        .args(["analyze", "diff"])
        .arg(&session)
        .args([
            "--from-state",
            "0",
            "--to-state",
            &final_state.to_string(),
            "--format",
            "png",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));

    let mut visual_svg = Command::cargo_bin("termtrace").expect("binary should build");
    visual_svg
        .args(["analyze", "diff"])
        .arg(&session)
        .args([
            "--from-state",
            "0",
            "--to-state",
            &final_state.to_string(),
            "--format",
            "svg",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("termtrace-semantic-diff-svg-v4"))
        .stdout(predicate::str::contains("#ff3b30"));

    let mut rejected = Command::cargo_bin("termtrace").expect("binary should build");
    rejected
        .args(["analyze", "diff"])
        .arg(&session)
        .args([
            "--from-state",
            "0",
            "--to-state",
            &final_state.to_string(),
            "--expect-no-change",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("changed=true"));

    let mut unchanged = Command::cargo_bin("termtrace").expect("binary should build");
    unchanged
        .args(["analyze", "diff"])
        .arg(&session)
        .args([
            "--from-state",
            &final_state.to_string(),
            "--to-state",
            &final_state.to_string(),
            "--expect-no-change",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("changed=false"));
}

#[test]
fn explain_attributes_controls_without_claiming_pixel_flicker() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("explain-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "printf 'before\\033[2J\\033[Hafter\\033[3J'",
        ])
        .assert()
        .success();

    let final_state = inspect_session(&session, StateSelector::Final)
        .expect("final state")
        .record
        .state_id;

    let log = EventLog::read(session.join("events.jsonl")).expect("canonical log");

    for target in [
        StatePoint::LatestPresented,
        StatePoint::StateId { state_id: 0 },
        StatePoint::StateId {
            state_id: final_state,
        },
        StatePoint::EventSeq { event_seq: 1 },
    ] {
        for context in [0, 1, 8] {
            let expected =
                explain(&log, target.clone(), context).expect("materialized explanation");
            let actual =
                explain_session(&session, target.clone(), context).expect("streamed explanation");
            assert_eq!(
                serde_json::to_value(actual).expect("streamed JSON"),
                serde_json::to_value(expected).expect("materialized JSON"),
            );
        }
    }

    let mut explain = Command::cargo_bin("termtrace").expect("binary should build");
    explain
        .args(["analyze", "explain"])
        .arg(&session)
        .args([
            "--state",
            &final_state.to_string(),
            "--context",
            "8",
            "--format",
            "json-compact",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""code":"full_viewport_clear""#))
        .stdout(predicate::str::contains(r#""code":"clear_scrollback""#))
        .stdout(predicate::str::contains(
            r#""code":"clear_then_repaint_candidate""#,
        ))
        .stdout(predicate::str::contains(
            "this does not establish that pixels flickered",
        ));

    let mut human_explain = Command::cargo_bin("termtrace").expect("binary should build");
    let human = human_explain
        .args(["analyze", "explain"])
        .arg(&session)
        .args(["--state", &final_state.to_string(), "--context", "8"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let human = String::from_utf8(human).expect("explain text");
    assert!(
        human.contains(&format!("state {final_state} event=")),
        "{human}"
    );
    assert!(human.contains(" csi:"), "{human}");
    assert!(human.contains("[high]"), "{human}");

    for leaked in ["Some(", "None", " Csi", "[High]"] {
        assert!(!human.contains(leaked), "leaked `{leaked}` in:\n{human}");
    }

    let mut initial = Command::cargo_bin("termtrace").expect("binary should build");
    initial
        .args(["analyze", "explain"])
        .arg(&session)
        .args(["--state", "0", "--context", "1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("state 0 event=-"));

    let mut verify = Command::cargo_bin("termtrace").expect("binary should build");
    verify
        .args(["integrity", "verify"])
        .arg(&session)
        .args(["--level", "structure"])
        .assert()
        .success()
        .stdout(predicate::str::contains("(structure)"))
        .stdout(predicate::str::contains("(Structure)").not());

    let diagnostic_session = temporary.path().join("diagnostic-session");
    let mut diagnostic_record = Command::cargo_bin("termtrace").expect("binary should build");
    diagnostic_record
        .args(["record", "--no-input", "--out"])
        .arg(&diagnostic_session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "printf '\\033[2J\\033[H'; printf '%*s' 1200 '' | tr ' ' X; printf '\\033[3J'",
        ])
        .assert()
        .success();

    let diagnostic_log =
        EventLog::read(diagnostic_session.join("events.jsonl")).expect("diagnostic log");
    let expected = diagnose(&diagnostic_log, Vec::new(), 1, &[]).expect("materialized diagnostics");
    let actual =
        diagnose_session(&diagnostic_session, Vec::new(), 1, &[]).expect("streamed diagnostics");
    assert_eq!(
        serde_json::to_value(actual).expect("streamed report"),
        serde_json::to_value(expected).expect("materialized report")
    );

    let mut diagnose = Command::cargo_bin("termtrace").expect("binary should build");
    diagnose
        .args(["analyze", "diagnose"])
        .arg(&diagnostic_session)
        .args([
            "--check",
            "full-redraw,flicker,stale-cells",
            "--broad-change-percent",
            "1",
            "--json",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            r#""format": "termtrace-diagnose""#,
        ))
        .stdout(predicate::str::contains(r#""code": "full_viewport_clear""#))
        .stdout(predicate::str::contains(r#""code": "clear_then_repaint""#))
        .stdout(predicate::str::contains(
            "semantic evidence does not by itself prove a visible pixel clear",
        ));

    let mut human_diagnose = Command::cargo_bin("termtrace").expect("binary should build");
    let human = human_diagnose
        .args(["analyze", "diagnose"])
        .arg(&diagnostic_session)
        .args([
            "--check",
            "full-redraw,flicker,stale-cells",
            "--broad-change-percent",
            "1",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let human = String::from_utf8(human).expect("diagnose text");
    assert!(human.contains("states="), "{human}");
    assert!(human.contains("tokens="), "{human}");
    assert!(
        human.contains("states=-") || human.contains("tokens=-"),
        "empty diagnostic references were not rendered as '-':\n{human}"
    );

    for leaked in ["states=[", "tokens=[", "Some(", "None", "[High]"] {
        assert!(!human.contains(leaked), "leaked `{leaked}` in:\n{human}");
    }

    let mut gate = Command::cargo_bin("termtrace").expect("binary should build");
    gate.args(["analyze", "diagnose"])
        .arg(&diagnostic_session)
        .args(["--check", "full-redraw", "--fail-on-finding"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("finding: full_viewport_clear"));
}

#[allow(
    clippy::expect_used,
    reason = "test helper failures should stop at the invalid fixture"
)]
fn json_lines(bytes: &[u8]) -> Vec<serde_json::Value> {
    return bytes
        .split(|byte| return *byte == b'\n')
        .filter(|line| return !line.is_empty())
        .map(|line| return serde_json::from_slice(line).expect("JSONL record"))
        .collect();
}
