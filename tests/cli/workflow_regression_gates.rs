use crate::support::*;
use serde::Deserialize;

const WORKFLOW_FIXTURES: [&str; 3] = [
    include_str!("../fixtures/regression-workflows/pi-extension-audit.json"),
    include_str!("../fixtures/regression-workflows/pi-extension-input-survival.json"),
    include_str!("../fixtures/regression-workflows/omp-extensions-tmux.json"),
];

#[derive(Debug, Deserialize)]
struct WorkflowFixture {
    format: String,
    version: u8,
    id: String,
    program: String,
    ready: String,
    done: String,
    accepted_input: String,
    reject_before_accept: bool,
    run_as_scenario: bool,
    generate_index: bool,
}

#[test]
fn converted_workflows_execute_with_surviving_evidence_and_compact_aggregate_output() {
    let temporary = TempDir::new().expect("temporary directory");
    let fixtures = WORKFLOW_FIXTURES
        .into_iter()
        .map(|fixture| {
            return serde_json::from_str::<WorkflowFixture>(fixture)
                .expect("executable workflow fixture JSON");
        })
        .collect::<Vec<_>>();
    assert_eq!(fixtures.len(), 3);

    let mut full_bytes = 0_usize;
    let mut agent_bytes = 0_usize;
    for fixture in &fixtures {
        assert_eq!(fixture.format, "termtrace-executable-regression-workflow");
        assert_eq!(fixture.version, 1);
        let session = temporary.path().join(&fixture.id);
        let child = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
            .args(["record", "--no-input", "--out"])
            .arg(&session)
            .args(["--", "/usr/bin/python3", "-c", fixture.program.as_str()])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn executable regression workflow");
        wait_for_control(&session);
        wait_for_output(&session, fixture.ready.as_bytes(), Duration::from_secs(15))
            .expect("workflow ready output");

        let full = wait_idle_output(&session, "full");
        assert!(full.status.success(), "full wait-idle failed: {full:?}");
        let full_value: serde_json::Value =
            serde_json::from_slice(&full.stdout).expect("full wait-idle JSON");
        assert!(contains_key(&full_value, "modes"));
        full_bytes = full_bytes.saturating_add(full.stdout.len());

        let agent = wait_idle_output(&session, "agent");
        assert!(agent.status.success(), "agent wait-idle failed: {agent:?}");
        let agent_value = compact_json_line(&agent.stdout);
        assert_eq!(agent_value["ok"], true);
        assert!(!contains_key(&agent_value, "modes"));
        assert!(!contains_key(&agent_value, "terminal_modes"));
        agent_bytes = agent_bytes.saturating_add(agent.stdout.len());

        if fixture.reject_before_accept {
            let rejected = send_request_response(
                &session,
                &ControlRequest::new(ControlCommand::Key {
                    key: NamedKey::Function(0),
                    modifiers: KeyModifiers::default(),
                }),
            )
            .expect("structured rejected input");
            assert!(matches!(
                &rejected.result,
                ControlResult::Error { code, .. } if code == "key_encoding_failed"
            ));
            send_request(&session, &ControlRequest::new(ControlCommand::Ping))
                .expect("ping after rejected input");
            assert!(matches!(
                send_request(&session, &ControlRequest::new(ControlCommand::Status)),
                Ok(ControlData::Status { .. })
            ));
        }

        let mut send = Command::cargo_bin("termtrace").expect("binary should build");
        let sent = send
            .args(["--response", "agent", "send"])
            .arg(&session)
            .arg(format!("text:{}", fixture.accepted_input))
            .output()
            .expect("send accepted fixture input");
        assert!(sent.status.success(), "accepted input failed: {sent:?}");
        let sent_value = compact_json_line(&sent.stdout);
        assert_eq!(sent_value["ok"], true);
        assert_eq!(sent_value["applied"], "all");

        let output = child.wait_with_output().expect("workflow completion");
        assert!(output.status.success(), "workflow child failed: {output:?}");
        wait_for_output(&session, fixture.done.as_bytes(), Duration::from_secs(5))
            .expect("workflow completion output");
        wait_for_path(&session.join("completion.json"));
        let log = EventLog::read(session.join("events.jsonl")).expect("completed event log");
        validate_completed_log(&session, &log).expect("completion evidence");

        let selected_hash = agent_value["state"]
            .as_str()
            .expect("agent selected state hash")
            .to_owned();
        inspect_session(&session, StateSelector::StateHash(selected_hash))
            .expect("selected state inspection");
        inspect_session(&session, StateSelector::Final).expect("final state inspection");

        let mut timeline = Command::cargo_bin("termtrace").expect("binary should build");
        timeline
            .args(["analyze", "timeline"])
            .arg(&session)
            .args(["--limit", "1", "--jsonl"])
            .assert()
            .success();
        if fixture.generate_index {
            assert_late_checkpoint_inspection(&session);
        }

        if fixture.run_as_scenario {
            assert_scenario_replayability(&temporary, fixture);
        }
    }

    assert!(
        agent_bytes.saturating_mul(100) <= full_bytes.saturating_mul(40),
        "representative agent output must be at least 60% smaller than full output: agent={agent_bytes} full={full_bytes}"
    );
}

#[allow(
    clippy::expect_used,
    reason = "integration helper must stop when the CLI cannot execute"
)]
fn wait_idle_output(session: &Path, response: &str) -> Output {
    return Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["--response", response, "wait-idle"])
        .arg(session)
        .args(["--stable-for", "10ms", "--timeout-ms", "5000"])
        .output()
        .expect("run wait-idle");
}

#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "integration helper must stop when the scenario report violates its asserted schema"
)]
fn assert_scenario_replayability(temporary: &TempDir, fixture: &WorkflowFixture) {
    let scenario = temporary.path().join("converted-pi-audit.termtrace.json");
    let artifacts = temporary.path().join("scenario-artifacts");
    let launch_program = format!("{}; import time; time.sleep(30)", fixture.program);
    fs_err::write(
        &scenario,
        serde_json::to_vec_pretty(&serde_json::json!({
            "version": 2,
            "name": "converted Pi audit workflow",
            "launch": {
                "command": ["/usr/bin/python3", "-c", launch_program],
                "timeoutMs": 5000
            },
            "steps": [
                {"waitScreen": {"contains": fixture.ready}},
                {"send": [{"text": fixture.accepted_input}]},
                {"waitScreen": {"contains": fixture.done}},
                {"stop": {}}
            ]
        }))
        .expect("scenario fixture JSON"),
    )
    .expect("write converted scenario");
    let mut test = Command::cargo_bin("termtrace").expect("binary should build");
    let output = test
        .args(["--response", "full", "test"])
        .arg(&scenario)
        .arg("--artifacts")
        .arg(&artifacts)
        .output()
        .expect("run converted scenario");
    assert!(
        output.status.success(),
        "converted scenario failed: {output:?}"
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("scenario report JSON");
    assert_eq!(report["status"], "passed");
    let attempt = &report["cases"][0]["attempts"][0]["report"];
    let canonical = PathBuf::from(
        attempt["canonical_session"]
            .as_str()
            .expect("scenario canonical path"),
    );
    let log = EventLog::read(canonical.join("events.jsonl")).expect("scenario event log");
    validate_completed_log(&canonical, &log).expect("scenario replay closure");
    inspect_session(&canonical, StateSelector::Final).expect("scenario final state");
    for step in attempt["steps"].as_array().expect("scenario steps") {
        let Some(state_hash) = step["state_hash"].as_str() else {
            continue;
        };
        inspect_session(&canonical, StateSelector::StateHash(state_hash.to_owned()))
            .expect("scenario selected state");
    }
}

fn contains_key(value: &serde_json::Value, expected: &str) -> bool {
    return match value {
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| return contains_key(value, expected)),
        serde_json::Value::Object(values) => {
            values.contains_key(expected)
                || values
                    .values()
                    .any(|value| return contains_key(value, expected))
        }
        _ => false,
    };
}
