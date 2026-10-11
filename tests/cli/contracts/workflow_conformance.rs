use crate::support::*;
use serde::Deserialize;

const WORKFLOW_FIXTURES: [&str; 3] = [
    include_str!("../../fixtures/regression-workflows/pi-extension-audit.json"),
    include_str!("../../fixtures/regression-workflows/pi-extension-input-survival.json"),
    include_str!("../../fixtures/regression-workflows/omp-extensions-tmux.json"),
];

#[derive(Debug, Deserialize)]
#[expect(dead_code, reason = "Format identifier parsed from fixture JSON")]
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

    for fixture in &fixtures {
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

        let agent = wait_idle_output(&session);
        assert!(agent.status.success(), "agent idle wait failed: {agent:?}");
        let agent_value = compact_json_line(&agent.stdout);
        assert_eq!(agent_value["kind"], "stable_snapshot");

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
                Ok(ControlPayload::Status { .. })
            ));
        }

        let mut act = Command::cargo_bin("termtrace").expect("binary should build");
        let action = act
            .args(["send", "--json"])
            .arg(&session)
            .arg(format!("text:{}", fixture.accepted_input))
            .output()
            .expect("act on accepted fixture input");
        assert!(action.status.success(), "accepted input failed: {action:?}");
        let action_value = compact_json_line(&action.stdout);
        assert_eq!(action_value["ok"], true);
        assert_eq!(
            action_value["input"]["effect"]["kind"], "applied",
            "unexpected compact action response: {action_value}"
        );
        let output = child.wait_with_output().expect("workflow completion");
        assert!(output.status.success(), "workflow child failed: {output:?}");
        wait_for_output(&session, fixture.done.as_bytes(), Duration::from_secs(5))
            .expect("workflow completion output");

        wait_for_path(&session.join("completion.json"));
        validate_completed_session(&session).expect("completion evidence");

        let selected_hash = agent_value["state_hash"]
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
}

#[allow(
    clippy::expect_used,
    reason = "integration helper must stop when the CLI cannot execute"
)]
fn wait_idle_output(session: &Path) -> Output {
    return Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["wait", "--json"])
        .arg(session)
        .args(["--idle=10ms", "--timeout", "5s"])
        .output()
        .expect("run idle wait");
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
        .args(["test"])
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

    validate_completed_session(&canonical).expect("scenario replay closure");
    inspect_session(&canonical, StateSelector::Final).expect("scenario final state");

    for step in attempt["steps"].as_array().expect("scenario steps") {
        let Some(state_hash) = step["state_hash"].as_str() else {
            continue;
        };
        inspect_session(&canonical, StateSelector::StateHash(state_hash.to_owned()))
            .expect("scenario selected state");
    }
}
