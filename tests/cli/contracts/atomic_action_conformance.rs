//! Shared public-SDK atomic-action conformance matrix.

use crate::support::*;
use serde::Deserialize;
use std::thread;
use termtrace_model::{
    protocol::control::{ScreenPostconditionQuality, StableCaptureQuality},
    terminal::observation::CaptureQuality,
    terminal::screen::{
        FrameDetail, ScreenFrame, ScreenFrameData, ScreenFrameError, ScreenFrameLimitKind,
        ScreenFrameSource,
    },
};
use termtrace_sdk::{
    ActionCompletion, ActionCondition, ActionOptions, Error, InputAtom, RetrySafety,
    StableCaptureOptions,
};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct Fixture {
    command: Vec<String>,
    ready_text: String,
    request_id: Uuid,
    input: Vec<FixtureAtom>,
    predicate: ScreenPredicate,
    expected_text: String,
    expected_atom_count: u32,
    frame_detail: FrameDetail,
    region: FixtureRegion,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum FixtureAtom {
    Text { text: String },
    Key { key: String },
}

#[derive(Clone, Copy, Debug, Deserialize)]
struct FixtureRegion {
    top: u16,
    left: u16,
    width: u16,
    height: u16,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "This shared SDK conformance response has separate protocol fields for each verified action property"
)]
struct SdkSummary {
    state_hash_valid: bool,
    atom_count: u32,
    matched: bool,
    presented: bool,
    retry_exact: bool,
    stable_quality: bool,
    actor_boundaries: bool,
}

#[test]
fn shared_fixture_matches_through_every_public_sdk_facade() {
    let fixture_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/conformance/actions/atomic-action.json");

    let fixture: Fixture =
        serde_json::from_slice(&fs_err::read(&fixture_path).expect("shared atomic-action fixture"))
            .expect("shared atomic-action fixture JSON");

    let temporary = TempDir::new().expect("temporary directory");
    let rust = rust_summary(&fixture, temporary.path());
    let python_sync = python_summary(&fixture_path, temporary.path(), false);
    let python_async = python_summary(&fixture_path, temporary.path(), true);
    let typescript = typescript_summary(&fixture_path, temporary.path());

    let expected = SdkSummary {
        state_hash_valid: true,
        atom_count: fixture.expected_atom_count,
        matched: true,
        presented: true,
        retry_exact: true,
        stable_quality: true,
        actor_boundaries: true,
    };

    assert_eq!(rust, expected, "Rust conformance outcomes");
    assert_eq!(python_sync, expected, "Python sync conformance outcomes");
    assert_eq!(python_async, expected, "Python async conformance outcomes");
    assert_eq!(typescript, expected, "TypeScript conformance outcomes");
}

#[test]
fn oversized_action_projection_is_cached_without_replaying_input() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = ClientTerminal::new(
        env!("CARGO_BIN_EXE_termtrace"),
        temporary.path().join("resource-action-evidence"),
    );

    let mut session = terminal
        .launch([
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from(
                "trap 'exit 0' TERM; printf READY; count=0; while IFS= read -r line; do count=$((count + 1)); printf '\\r\\nCOUNT:%s:%s' \"$count\" \"$line\"; done",
            ),
        ])
        .geometry(300, 150)
        .start()
        .expect("launch oversized action fixture");

    wait_for_output(session.path(), b"READY", Duration::from_secs(5))
        .expect("oversized action fixture ready");
    let initial = session
        .screen()
        .frame(ScreenProjectionRequest::default())
        .expect("initial oversized action frame");

    let atoms = [
        InputAtom::Text("once".to_owned()),
        InputAtom::Key {
            key: NamedKey::Enter,
            modifiers: KeyModifiers::default(),
        },
    ];

    let options = ActionOptions {
        request_id: Some(
            Uuid::parse_str("019fddda-17c8-7000-87da-8785e71ab5e1")
                .expect("oversized action request UUID"),
        ),
        if_state: Some(initial.state_hash),
        postcondition: ActionCondition::Screen {
            predicate: ScreenPredicate::Contains {
                text: "COUNT:1:once".to_owned(),
            },
            timeout: Duration::from_secs(5),
        },
        projection: ScreenProjectionRequest {
            detail: FrameDetail::Cells,
            region: Some(ScreenRegion {
                top: 0,
                left: 0,
                right: 300,
                bottom: 150,
            }),
        },
        ..ActionOptions::default()
    };
    let error = session
        .act(&atoms, options.clone())
        .expect_err("oversized action projection must fail loudly");

    let Error::ActionTerminatingFrame {
        request_id,
        retry_safety,
        source: ScreenFrameError::ResourceLimit { limit, .. },
        ..
    } = &error
    else {
        panic!("unexpected oversized action error: {error:?}");
    };

    assert_eq!(
        *request_id,
        options.request_id.expect("fixed action request ID")
    );
    assert_eq!(*retry_safety, RetrySafety::SameRequestOnly);
    assert_eq!(*limit, ScreenFrameLimitKind::SerializedBytes);

    let retry = session
        .act(&atoms, options)
        .expect_err("cached oversized action retry must retain its failure");

    let Error::ActionTerminatingFrame {
        request_id: retry_request_id,
        retry_safety: retry_safety_retry,
        source: ScreenFrameError::ResourceLimit {
            limit: retry_limit, ..
        },
        ..
    } = &retry
    else {
        panic!("unexpected retry action error: {retry:?}");
    };

    assert_eq!(retry_request_id, request_id);
    assert_eq!(retry_safety_retry, retry_safety);
    assert_eq!(retry_limit, limit);

    wait_for_output(session.path(), b"COUNT:1:once", Duration::from_secs(5))
        .expect("first action input reached the fixture");
    let frame = session
        .screen()
        .frame(ScreenProjectionRequest::default())
        .expect("post-error screen frame");
    assert!(frame_contains(&frame, "COUNT:1:once"));
    assert!(!frame_contains(&frame, "COUNT:2:once"));

    session
        .stop(Some(Duration::from_secs(5)))
        .expect("stop oversized action fixture");
}

#[test]
fn action_termination_frame_remains_live_actor_evidence() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = ClientTerminal::new(
        env!("CARGO_BIN_EXE_termtrace"),
        temporary.path().join("action-quality-evidence"),
    );

    let mut session = terminal
        .launch([
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from("printf READY; IFS= read -r line; printf '\\r\\nDONE:%s' \"$line\""),
        ])
        .start()
        .expect("launch action quality fixture");

    wait_for_output(session.path(), b"READY", Duration::from_secs(5))
        .expect("action quality fixture ready");
    let result = session
        .act(
            &[
                InputAtom::Text("once".to_owned()),
                InputAtom::Key {
                    key: NamedKey::Enter,
                    modifiers: KeyModifiers::default(),
                },
            ],
            ActionOptions {
                postcondition: ActionCondition::Screen {
                    predicate: ScreenPredicate::Contains {
                        text: "never-matches".to_owned(),
                    },
                    timeout: Duration::from_secs(5),
                },
                allow_incomplete: true,
                ..ActionOptions::default()
            },
        )
        .expect("child-terminated action result");

    let ActionCompletion::Screen {
        quality:
            ScreenPostconditionQuality::ChildCompleted | ScreenPostconditionQuality::OutputClosed,
        ..
    } = &result.completion
    else {
        panic!("unexpected action completion: {:?}", result.completion);
    };

    let frame = result.frame.as_ref().expect("frame");
    assert_eq!(
        frame.capture_quality,
        CaptureQuality::Live,
        "actor-owned action frame is not validated completed evidence"
    );

    session
        .complete(Duration::from_secs(5))
        .expect("complete action quality fixture");
}

#[test]
fn force_close_terminates_the_recorded_process_group() {
    let temporary = TempDir::new().expect("temporary directory");
    let pid_path = temporary.path().join("recorded.pid");
    let terminal = ClientTerminal::new(
        env!("CARGO_BIN_EXE_termtrace"),
        temporary.path().join("force-close-evidence"),
    );

    let command = format!(
        "trap '' HUP TERM; printf '%s' $$ > '{}'; exec sleep 600",
        pid_path.display()
    );

    let mut session = terminal
        .launch([
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from(command),
        ])
        .start()
        .expect("launch force-close fixture");

    let deadline = Instant::now() + Duration::from_secs(5);

    while !pid_path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }

    let recorded_pid: u32 = fs_err::read_to_string(&pid_path)
        .expect("recorded process PID")
        .parse()
        .expect("numeric recorded process PID");

    session.force_close().expect("force-close process group");

    let process_path = PathBuf::from(format!("/proc/{recorded_pid}"));
    let deadline = Instant::now() + Duration::from_secs(2);

    while process_path.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }

    assert!(
        !process_path.exists(),
        "recorded process remained alive after force-close"
    );
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test-only conformance helper reports fixture and SDK failures"
)]
fn rust_summary(fixture: &Fixture, temporary: &Path) -> SdkSummary {
    let terminal = ClientTerminal::new(
        env!("CARGO_BIN_EXE_termtrace"),
        temporary.join("rust-action-evidence"),
    );

    let command: Vec<OsString> = fixture.command.iter().map(OsString::from).collect();
    let mut session = terminal
        .launch(command)
        .geometry(fixture.region.width, fixture.region.height)
        .start()
        .expect("launch Rust action fixture");

    wait_for_output(
        session.path(),
        fixture.ready_text.as_bytes(),
        Duration::from_secs(5),
    )
    .expect("Rust fixture ready");
    let initial = session
        .screen()
        .frame(ScreenProjectionRequest::default())
        .expect("initial Rust frame");

    let atoms: Vec<InputAtom> = fixture
        .input
        .iter()
        .map(|atom| {
            return match atom {
                FixtureAtom::Text { text } => InputAtom::Text(text.clone()),
                FixtureAtom::Key { key } if key == "enter" => InputAtom::Key {
                    key: NamedKey::Enter,
                    modifiers: KeyModifiers::default(),
                },
                FixtureAtom::Key { key } => panic!("unsupported fixture key {key}"),
            };
        })
        .collect();

    let options = ActionOptions {
        request_id: Some(fixture.request_id),
        if_state: Some(initial.state_hash),
        postcondition: ActionCondition::Screen {
            predicate: fixture.predicate.clone(),
            timeout: Duration::from_secs(5),
        },
        projection: ScreenProjectionRequest {
            detail: fixture.frame_detail,
            region: Some(ScreenRegion {
                top: fixture.region.top,
                left: fixture.region.left,
                right: fixture.region.left + fixture.region.width,
                bottom: fixture.region.top + fixture.region.height,
            }),
        },
        ..ActionOptions::default()
    };
    let result = session
        .act(&atoms, options.clone())
        .expect("Rust atomic action");
    let retry = session
        .act(&atoms, options)
        .expect("idempotent Rust action retry");
    let stable = session
        .screen()
        .capture_stable(StableCaptureOptions {
            stable_for: Duration::from_millis(20),
            timeout: Duration::from_secs(2),
            projection: ScreenProjectionRequest::default(),
            allow_incomplete: false,
        })
        .expect("Rust stable capture");

    let frame = result.frame.as_ref().expect("frame");
    let matched = matches!(
        result.completion,
        ActionCompletion::Screen {
            matched: true,
            quality: ScreenPostconditionQuality::Matched
                | ScreenPostconditionQuality::MatchedUnsettled,
        }
    ) && frame_contains(frame, &fixture.expected_text);

    let actor_boundaries = matches!(
        &frame.source,
        ScreenFrameSource::Actor { boundaries, .. }
            if boundaries == &result.boundaries
    );

    let summary = SdkSummary {
        state_hash_valid: frame.state_hash.len() == 64,
        atom_count: result.input.atom_count,
        matched,
        presented: frame.presented,
        retry_exact: retry == result,
        stable_quality: stable.quality == StableCaptureQuality::Stable,
        actor_boundaries,
    };

    session
        .stop(Some(Duration::from_secs(5)))
        .expect("stop Rust fixture");

    return summary;
}

#[allow(
    clippy::expect_used,
    reason = "test-only conformance helper reports subprocess and JSON failures"
)]
fn python_summary(fixture: &Path, temporary: &Path, asynchronous: bool) -> SdkSummary {
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/python");
    let code = if asynchronous { PYTHON_ASYNC_CONFORMANCE } else { PYTHON_SYNC_CONFORMANCE };

    let output = StdCommand::new("uv")
        .args(["run", "--isolated", "--project"])
        .arg(package)
        .arg("python")
        .arg("-c")
        .arg(code)
        .arg(env!("CARGO_BIN_EXE_termtrace"))
        .arg(fixture)
        .arg(temporary.join(if asynchronous {
            "python-async-action-evidence"
        } else {
            "python-sync-action-evidence"
        }))
        .output()
        .expect("run Python action conformance");
    assert!(
        output.status.success(),
        "Python action conformance failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    return serde_json::from_slice(&output.stdout).expect("Python action summary JSON");
}

#[allow(
    clippy::expect_used,
    reason = "test-only conformance helper reports subprocess and JSON failures"
)]
fn typescript_summary(fixture: &Path, temporary: &Path) -> SdkSummary {
    let package = Path::new(env!("CARGO_MANIFEST_DIR")).join("packages/typescript/src/index.ts");

    let output = StdCommand::new("bun")
        .arg("-e")
        .arg(TYPESCRIPT_CONFORMANCE)
        .arg(package)
        .arg(env!("CARGO_BIN_EXE_termtrace"))
        .arg(fixture)
        .arg(temporary.join("typescript-action-evidence"))
        .output()
        .expect("run TypeScript action conformance");
    assert!(
        output.status.success(),
        "TypeScript action conformance failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    return serde_json::from_slice(&output.stdout).expect("TypeScript action summary JSON");
}

fn frame_contains(frame: &ScreenFrame, expected: &str) -> bool {
    match &frame.data {
        ScreenFrameData::Text { rows } => {
            return rows.iter().any(|row| return row.text.contains(expected));
        }
        ScreenFrameData::Spans { rows } => {
            return rows.iter().any(|row| return row.text.contains(expected));
        }
        ScreenFrameData::Cells { rows } => {
            return rows.iter().any(|row| return row.text.contains(expected));
        }
        ScreenFrameData::None => false,
    }
}

const PYTHON_SYNC_CONFORMANCE: &str = r#"import json, sys
from pathlib import Path
from termtrace import ActionOptions, Duration, FrameRequest, KeyAtom, Region, ScreenPostcondition, StableOptions, Terminal, TextAtom
fixture = json.loads(Path(sys.argv[2]).read_text())
terminal = Terminal(binary_path=sys.argv[1], evidence_root=sys.argv[3])
session = terminal.launch(fixture["command"], cols=fixture["region"]["width"], rows=fixture["region"]["height"])
session.output.wait_for_text(fixture["ready_text"])
initial = session.screen.frame()
region = Region(**fixture["region"])
atoms = [TextAtom(item["text"]) if item["kind"] == "text" else KeyAtom(item["key"]) for item in fixture["input"]]
options = ActionOptions(request_id=fixture["request_id"], if_state=initial.state_hash, postcondition=ScreenPostcondition(fixture["predicate"], Duration.seconds(5)), frame=FrameRequest(fixture["frame_detail"], region))
result = session.act(atoms, options)
retry = session.act(atoms, options)
stable = session.screen.capture_stable(StableOptions(Duration.milliseconds(20), Duration.seconds(2)))
summary = {"state_hash_valid": len(result.frame.state_hash) == 64, "atom_count": result.input.completed_atoms, "matched": result.postcondition.quality in ("matched", "matched_unsettled") and fixture["expected_text"] in result.frame.viewport_text, "presented": result.frame.presented, "retry_exact": retry == result, "stable_quality": stable.quality == "stable", "actor_boundaries": result.frame.boundaries is not None and result.frame.boundaries == result.frame.source.boundaries}
session.stop()
session.evidence.complete()
session.close()
terminal.close()
print(json.dumps(summary, sort_keys=True))
"#;
const PYTHON_ASYNC_CONFORMANCE: &str = r#"import asyncio, json, sys
from pathlib import Path
from termtrace import ActionOptions, AsyncTerminal, Duration, FrameRequest, KeyAtom, Region, ScreenPostcondition, StableOptions, TextAtom
async def main():
    fixture = json.loads(Path(sys.argv[2]).read_text())
    terminal = await AsyncTerminal.create(binary_path=sys.argv[1], evidence_root=sys.argv[3])
    session = await terminal.launch(fixture["command"], cols=fixture["region"]["width"], rows=fixture["region"]["height"])
    await session.output.wait_for_text(fixture["ready_text"])
    initial = await session.screen.frame()
    region = Region(**fixture["region"])
    atoms = [TextAtom(item["text"]) if item["kind"] == "text" else KeyAtom(item["key"]) for item in fixture["input"]]
    options = ActionOptions(request_id=fixture["request_id"], if_state=initial.state_hash, postcondition=ScreenPostcondition(fixture["predicate"], Duration.seconds(5)), frame=FrameRequest(fixture["frame_detail"], region))
    result = await session.act(atoms, options)
    retry = await session.act(atoms, options)
    stable = await session.screen.capture_stable(StableOptions(Duration.milliseconds(20), Duration.seconds(2)))
    summary = {"state_hash_valid": len(result.frame.state_hash) == 64, "atom_count": result.input.completed_atoms, "matched": result.postcondition.quality in ("matched", "matched_unsettled") and fixture["expected_text"] in result.frame.viewport_text, "presented": result.frame.presented, "retry_exact": retry == result, "stable_quality": stable.quality == "stable", "actor_boundaries": result.frame.boundaries is not None and result.frame.boundaries == result.frame.source.boundaries}
    await session.stop()
    await session.evidence.complete()
    await session.close()
    await terminal.close()
    print(json.dumps(summary, sort_keys=True))
asyncio.run(main())
"#;
const TYPESCRIPT_CONFORMANCE: &str = r#"const { readFileSync } = await import("node:fs")
const { Duration, Termtrace } = await import(process.argv[1])
const fixture = JSON.parse(readFileSync(process.argv[3], "utf8"))
const terminal = await Termtrace.create({ binaryPath: process.argv[2] })
const session = await terminal.launch({ command: fixture.command, evidenceRoot: process.argv[4], cols: fixture.region.width, rows: fixture.region.height })
await session.output.waitForText(fixture.ready_text)
const initial = await session.screen.frame()
const atoms = fixture.input.map((item) => item.kind === "text" ? { kind: "text", text: item.text } : { kind: "key", key: item.key })
const options = { requestId: fixture.request_id, ifState: initial.stateHash, postcondition: { kind: "screen", predicate: fixture.predicate, timeout: Duration.seconds(5) }, frame: { detail: fixture.frame_detail, region: fixture.region } }
const result = await session.act(atoms, options)
const retry = await session.act(atoms, options)
const stable = await session.screen.captureStable({ stableFor: Duration.milliseconds(20), timeout: Duration.seconds(2) })
const retryExact = retry.requestId === result.requestId && retry.frame.frameHash === result.frame.frameHash && retry.input.completedAtoms === result.input.completedAtoms && retry.input.firstIntentEventSeq === result.input.firstIntentEventSeq && retry.input.lastResultEventSeq === result.input.lastResultEventSeq && retry.completion.kind === result.completion.kind && retry.completion.quality === result.completion.quality
const summary = { state_hash_valid: result.frame.stateHash.length === 64, atom_count: result.input.completedAtoms, matched: result.completion.kind === "screen" && (result.completion.quality === "matched" || result.completion.quality === "matched_unsettled") && result.frame.viewportText.includes(fixture.expected_text), presented: result.frame.presented, retry_exact: retryExact, stable_quality: stable.quality === "stable", actor_boundaries: result.frame.source.boundaries !== undefined }
await session.stop()
await session.complete(10000)
await session.close()
await terminal.close()
console.log(JSON.stringify(summary))
"#;
