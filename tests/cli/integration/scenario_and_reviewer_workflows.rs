//! scenario and reviewer workflows CLI integration tests.

use crate::support::*;

#[test]
fn scenario_runner_executes_typed_steps_in_fresh_evidence() {
    let temporary = TempDir::new().expect("temporary directory");
    let scenario = temporary.path().join("ready.termtrace.json");
    let artifacts = temporary.path().join("artifacts");
    fs_err::write(
        &scenario,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 2,
            "name": "ready workflow",
            "launch": {
                "command": ["/bin/sh", "-c", "test \"$SCENARIO_ID\" = ready; printf Ready; IFS= read -r line; printf Done; sleep 30"],
                "environment": {
                    "base": "clear",
                    "set": { "SCENARIO_ID": ByteString::from(b"ready".as_slice()) },
                    "copy": ["PATH"],
                    "unset": [],
                    "color": "never"
                },
                "geometry": { "cols": 90, "rows": 25 },
                "timeoutMs": 3000
            },
            "steps": [
                { "waitScreen": { "contains": "Ready" } },
                { "mark": "ready-visible" },
                { "send": [{ "text": "go" }, { "key": "enter" }] },
                { "waitScreen": { "contains": "Done" } },
                { "mark": "done-visible" },
                { "assertTimeline": {
                    "betweenMarkers": ["ready-visible", "done-visible"],
                    "noPresentedScreenContains": "Missing"
                } },
                { "matchScreenSnapshot": {
                    "name": "ready-workflow-final",
                    "stableForMs": 10
                } },
                { "stop": {} }
            ]
        }))
        .expect("scenario JSON"),
    )
    .expect("write scenario");
    let second = temporary.path().join("simple.termtrace.json");
    fs_err::write(
        &second,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 2,
            "name": "simple workflow",
            "launch": {
                "command": ["/bin/sh", "-c", "test \"$SCENARIO_ID\" = simple; printf SIMPLE; sleep 30"],
                "environment": {
                    "base": "clear",
                    "set": { "SCENARIO_ID": ByteString::from(b"simple".as_slice()) },
                    "copy": ["PATH"],
                    "unset": [],
                    "color": "auto"
                },
                "timeoutMs": 3000
            },
            "steps": [
                { "waitScreen": { "contains": "SIMPLE" } },
                { "stop": {} }
            ]
        }))
        .expect("second scenario JSON"),
    )
    .expect("write second scenario");

    let junit = temporary.path().join("junit.xml");
    let mut update = Command::cargo_bin("termtrace").expect("binary should build");
    update
        .arg("test")
        .arg(&scenario)
        .arg(&second)
        .arg("--artifacts")
        .arg(&artifacts)
        .arg("--update-snapshots")
        .args(["--jobs", "2"])
        .arg("--junit")
        .arg(&junit)
        .assert()
        .success();
    assert!(
        artifacts
            .join("snapshots/ready-workflow-final.frame.json")
            .is_file()
    );
    assert!(
        fs_err::read_to_string(&junit)
            .expect("JUnit XML")
            .contains("failures=\"0\"")
    );

    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    let assertion = command
        .arg("test")
        .arg(&scenario)
        .arg(&second)
        .arg("--artifacts")
        .arg(&artifacts)
        .args(["--jobs", "2"])
        .assert()
        .success();
    let report: serde_json::Value =
        serde_json::from_slice(&assertion.get_output().stdout).expect("scenario report");
    assert_eq!(report["status"], "passed");
    assert_eq!(report["cases"].as_array().expect("cases").len(), 2);
    let attempt = &report["cases"][0]["attempts"][0]["report"];
    assert_eq!(attempt["steps"].as_array().expect("steps").len(), 8);
    let canonical = PathBuf::from(attempt["canonical_session"].as_str().expect("path"));
    assert!(canonical.is_dir());
    let log = EventLog::read(canonical.join("events.jsonl")).expect("scenario event log");
    validate_completed_log(&canonical, &log).expect("scenario completion closure");
    let replayed = replay(&log).expect("scenario final replay");
    assert!(
        replayed.states.last().is_some(),
        "final state must be inspectable"
    );
    for step in attempt["steps"].as_array().expect("steps") {
        let Some(state_hash) = step["state_hash"].as_str() else {
            continue;
        };
        let mut show = Command::cargo_bin("termtrace").expect("binary should build");
        show.arg("show")
            .arg(&canonical)
            .args(["--state-hash", state_hash, "--format", "json-compact"])
            .assert()
            .success();
    }
    let mut timeline = Command::cargo_bin("termtrace").expect("binary should build");
    timeline
        .args(["analyze", "timeline"])
        .arg(&canonical)
        .args(["--limit", "1", "--jsonl"])
        .assert()
        .success();
    assert!(
        fs_err::read_dir(canonical.join("derived-cache"))
            .expect("scenario replay checkpoints")
            .next()
            .is_some(),
        "closure must checkpoint an asserted endpoint"
    );
}

#[test]
fn bundled_agent_skill_scenario_is_directly_executable() {
    let temporary = TempDir::new().expect("temporary directory");
    let scenario = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("skills/termtrace/examples/ready.termtrace.json");
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    command
        .arg("test")
        .arg(scenario)
        .arg("--artifacts")
        .arg(temporary.path().join("skill-example-artifacts"))
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""status":"passed""#));
}

#[test]
fn scenario_failure_publishes_atomic_diagnostic_directory() {
    let temporary = TempDir::new().expect("temporary directory");
    let scenario = temporary.path().join("failure.termtrace.json");
    let artifacts = temporary.path().join("artifacts");
    fs_err::write(
        &scenario,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 2,
            "name": "missing screen",
            "launch": {
                "command": ["/bin/sh", "-c", "printf Ready; sleep 30"],
                "environment": {
                    "base": "clear",
                    "set": { "SCENARIO_SECRET": ByteString::from(b"scenario-private".as_slice()) },
                    "copy": ["PATH"],
                    "unset": [],
                    "color": "never"
                },
                "geometry": { "cols": 200, "rows": 100 },
                "outputByteBudget": 1_000_000,
                "timeoutMs": 1000
            },
            "steps": [{ "waitScreen": { "contains": "Missing", "timeoutMs": 50 } }]
        }))
        .expect("scenario JSON"),
    )
    .expect("write scenario");

    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    let assertion = command
        .arg("test")
        .arg(&scenario)
        .arg("--artifacts")
        .arg(&artifacts)
        .args(["--reruns", "1"])
        .assert()
        .failure();
    let report: serde_json::Value =
        serde_json::from_slice(&assertion.get_output().stdout).expect("failure report");
    assert_eq!(report["status"], "failed");
    assert_eq!(
        report["cases"][0]["attempts"]
            .as_array()
            .expect("attempts")
            .len(),
        2
    );
    let attempt = &report["cases"][0]["attempts"][1]["report"];
    assert_eq!(attempt["failure"]["step_index"], 0);
    let failure = Path::new(attempt["failure_artifacts"].as_str().expect("failure path"));
    assert!(failure.join("scenario.json").is_file());
    assert!(failure.join("result.json").is_file());
    assert!(failure.join("final-screen.json").is_file());
    assert!(failure.join("final-screen.txt").is_file());
    assert!(failure.join("final-screen.png").is_file());
    let final_snapshot: ControlData = serde_json::from_slice(
        &fs_err::read(failure.join("final-screen.json")).expect("final screen snapshot"),
    )
    .expect("final screen snapshot JSON");
    let ControlData::Snapshot {
        viewport_text,
        cols,
        rows,
        ..
    } = final_snapshot
    else {
        panic!("final screen artifact must retain the actor snapshot payload");
    };
    assert_eq!((cols, rows), (200, 100));
    assert!(viewport_text.contains("Ready"));
    assert_eq!(
        fs_err::read_to_string(failure.join("final-screen.txt")).expect("final screen text"),
        viewport_text
    );
    assert!(failure.join("timeline.json").is_file());
    assert!(failure.join("diagnose.json").is_file());
    let canonical = Path::new(
        attempt["canonical_session"]
            .as_str()
            .expect("canonical session"),
    );
    assert_eq!(
        read_manifest(canonical)
            .expect("scenario manifest")
            .resource_limits
            .output_bytes,
        Some(1_000_000)
    );
    let copied_scenario = fs_err::read(failure.join("scenario.json")).expect("copied scenario");
    let encoded_secret = serde_json::to_vec(&ByteString::from(b"scenario-private".as_slice()))
        .expect("encoded secret");
    assert!(
        !copied_scenario
            .windows(encoded_secret.len())
            .any(|window| return window == encoded_secret)
    );
    let encoded_secret = &encoded_secret[1..encoded_secret.len() - 1];
    for entry in fs_err::read_dir(failure).expect("failure artifacts") {
        let path = entry.expect("failure artifact entry").path();
        if path.is_file() {
            let bytes = fs_err::read(&path).expect("failure artifact");
            assert!(
                !bytes
                    .windows(encoded_secret.len())
                    .any(|window| return window == encoded_secret),
                "scenario secret leaked into {}",
                path.display()
            );
        }
    }
}

#[test]
fn scenario_wait_requires_the_requested_stability_interval() {
    let temporary = TempDir::new().expect("temporary directory");
    let scenario = temporary.path().join("unstable.termtrace.json");
    let artifacts = temporary.path().join("artifacts");
    fs_err::write(
        &scenario,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 2,
            "name": "unstable matching screen",
            "launch": {
                "command": [
                    "/bin/sh",
                    "-c",
                    "printf 'READY\\n'; i=0; while :; do printf '\\r%04d' \"$i\"; i=$((i + 1)); sleep .01; done"
                ],
                "environment": {
                    "base": "clear",
                    "set": {},
                    "copy": ["PATH"],
                    "unset": [],
                    "color": "never"
                },
                "timeoutMs": 1000
            },
            "steps": [{
                "waitScreen": {
                    "contains": "READY",
                    "stableForMs": 100,
                    "timeoutMs": 250
                }
            }]
        }))
        .expect("scenario JSON"),
    )
    .expect("write scenario");

    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    let assertion = command
        .arg("test")
        .arg(&scenario)
        .arg("--artifacts")
        .arg(&artifacts)
        .assert()
        .failure();
    let report: serde_json::Value =
        serde_json::from_slice(&assertion.get_output().stdout).expect("scenario report");
    assert_eq!(report["status"], "failed");
    let attempt = &report["cases"][0]["attempts"][0]["report"];
    assert_eq!(attempt["failure"]["step_index"], 0);
    assert!(
        attempt["failure"]["message"]
            .as_str()
            .is_some_and(|message| return message.contains("requested stability before Deadline"))
    );
}

#[test]
fn reviewer_summary_is_read_only_and_never_replays_recorded_controls() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("review-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            r"printf '\033]52;c;hostile-payload\007SAFE'",
        ])
        .assert()
        .success();
    let event_path = session.join("events.jsonl");
    let before = fs_err::read(&event_path).expect("event log before review");

    let mut review = Command::cargo_bin("termtrace").expect("binary should build");
    let assertion = review
        .args(["analyze", "review"])
        .arg(&session)
        .arg("--json")
        .assert()
        .success();
    let summary: serde_json::Value =
        serde_json::from_slice(&assertion.get_output().stdout).expect("review summary");
    assert_eq!(
        summary["rendering_policy"],
        "semantic_cells_only_no_raw_replay"
    );
    assert!(summary["state_count"].as_u64().expect("state count") > 0);
    assert!(
        !assertion
            .get_output()
            .stdout
            .windows("hostile-payload".len())
            .any(|window| return window == b"hostile-payload")
    );
    let mut search = Command::cargo_bin("termtrace").expect("binary should build");
    let searched = search
        .args(["analyze", "review"])
        .arg(&session)
        .args(["--json", "--search", "SAFE"])
        .assert()
        .success();
    let result: serde_json::Value =
        serde_json::from_slice(&searched.get_output().stdout).expect("review search");
    assert_eq!(result["query"], "SAFE");
    assert!(!result["matches"].as_array().expect("matches").is_empty());
    assert!(
        result["matches"][0]["selector"]
            .as_str()
            .is_some_and(|selector| return selector.starts_with("--state "))
    );
    let state_id = result["matches"][0]["state_id"]
        .as_u64()
        .expect("matched state ID");
    let exported = temporary.path().join("review-export");
    let mut export = Command::cargo_bin("termtrace").expect("binary should build");
    export
        .args(["analyze", "review"])
        .arg(&session)
        .arg("--export-state")
        .arg(&exported)
        .arg("--state")
        .arg(state_id.to_string())
        .assert()
        .success();
    assert!(exported.join("manifest.json").is_file());
    assert!(exported.join("state.json").is_file());
    assert!(exported.join("screen.txt").is_file());
    assert_eq!(
        fs_err::read(event_path).expect("event log after review"),
        before
    );
}

#[test]
fn reviewer_exposes_scriptable_commands_without_a_session() {
    let mut review = Command::cargo_bin("termtrace").expect("binary should build");
    review
        .args(["analyze", "review", "--commands"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "semantic_cells_only_no_raw_replay",
        ))
        .stdout(predicate::str::contains("next_state"))
        .stdout(predicate::str::contains("next_marker"));
}
