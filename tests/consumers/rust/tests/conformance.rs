mod support;

use std::{
    fs,
    path::Path,
    thread,
    time::{Duration, Instant},
};
use support::{screen_text, stop, terminal, wait_text};
use termtrace_sdk::{
    output::OutputService,
    screen::{FrameDetail, ScreenProjectionRequest},
    session::LaunchSpec,
    ActRequest, ErrorKind, MutationEffect, Retry,
};

const FIXTURE: &str = include_str!("../../../../fixtures/conformance/sdk/cases/foundation.json");

fn process_is_gone(pid_file: &Path) -> bool {
    let pid: u32 = fs::read_to_string(pid_file)
        .expect("pid file")
        .parse()
        .expect("pid");

    return pid > 1 && !Path::new(&format!("/proc/{pid}")).exists();
}

#[test]
fn foundation_case_matches_the_shared_fixture() {
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture");
    let terminal = terminal("conformance-foundation");
    let root = terminal
        .evidence_root()
        .parent()
        .expect("scratch")
        .to_owned();
    let mut command: Vec<String> = fixture["command"]
        .as_array()
        .expect("command")
        .iter()
        .map(|part| return part.as_str().expect("command part").to_owned())
        .collect();
    command.push(root.join("child.pid").display().to_string());
    let rows = u16::try_from(fixture["rows"].as_u64().expect("rows")).expect("rows");
    let cols = u16::try_from(fixture["cols"].as_u64().expect("cols")).expect("cols");
    let mut session = terminal
        .start(LaunchSpec::new(command).geometry(cols, rows))
        .expect("start");

    wait_text(&session, fixture["text"].as_str().expect("text"));
    let frame = session
        .frame(ScreenProjectionRequest {
            detail: FrameDetail::Text,
            region: None,
        })
        .expect("frame");
    assert_eq!((frame.geometry.rows, frame.geometry.cols), (rows, cols));
    assert!(screen_text(&session).contains(fixture["text"].as_str().expect("text")));

    let error = session
        .act(
            ActRequest::new()
                .text("FORBIDDEN\n")
                .if_state(fixture["guard"].as_str().expect("guard")),
        )
        .expect_err("stale guard must fail");
    assert_eq!(error.kind(), ErrorKind::StaleState);
    assert_eq!(error.code(), "stale_observation");
    assert_eq!(error.effect(), MutationEffect::NotApplied);
    assert_eq!(error.retry(), Retry::NewRequest);

    // The live screen can show output before the recorder has appended it to the event log.
    let expected_hex = fixture["output_hex"].as_str().expect("output_hex");
    let deadline = Instant::now() + Duration::from_secs(5);
    let (output, output_hex) = loop {
        let output = OutputService::read_page(session.path(), None, 4096).expect("output page");
        let output_hex: String = output
            .bytes
            .iter()
            .map(|b| return format!("{b:02x}"))
            .collect();
        if output_hex == expected_hex || Instant::now() >= deadline {
            break (output, output_hex);
        }

        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(output_hex, expected_hex);

    let completion = stop(&mut session);
    assert_eq!(completion.output_bytes, output.bytes.len() as u64);
    assert!(completion.failure.is_none());

    let events = fs::read_to_string(session.path().join("events.jsonl")).expect("events");
    for line in events.lines().skip(1) {
        let record: serde_json::Value = serde_json::from_str(line).expect("event record");
        let kind = record["event"]["type"].as_str().unwrap_or_default();
        assert!(
            !matches!(kind, "input" | "input_intent" | "input_result"),
            "stale guard emitted {kind}"
        );
    }
    assert!(
        process_is_gone(&root.join("child.pid")),
        "child was not reaped"
    );
    assert!(
        process_is_gone(&root.join("child.pid.recorder")),
        "recorder was not reaped"
    );
}
