//! Shared public-SDK atomic-action conformance matrix.

use crate::support::*;
use serde::Deserialize;
use std::thread;
use termtrace_model::{
    protocol::control::{
        ActionPostconditionResult, ScreenPostconditionQuality, StableCaptureQuality,
    },
    terminal::observation::CaptureQuality,
    terminal::screen::{FrameDetail, ScreenFrame, ScreenFrameData, ScreenFrameSource},
};
use termtrace_sdk::{MutationEffect, MutationGuard, Retry};
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
}

#[test]
fn oversized_action_projection_is_cached_without_replaying_input() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = ClientTerminal::new(
        env!("CARGO_BIN_EXE_termtrace"),
        temporary.path().join("resource-action-evidence"),
    );

    let mut session = terminal
        .start(LaunchSpec::new([
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from(
                "trap 'exit 0' TERM; printf READY; count=0; while IFS= read -r line; do count=$((count + 1)); printf '\\r\\nCOUNT:%s:%s' \"$count\" \"$line\"; done",
            ),
        ]).geometry(300, 150))
        .expect("launch oversized action fixture");

    wait_for_output(session.path(), b"READY", Duration::from_secs(5))
        .expect("oversized action fixture ready");
    let initial = session
        .frame(ScreenProjectionRequest::default())
        .expect("initial oversized action frame");

    let request = ActRequest {
        atoms: vec![InputAtom::Text("once".to_owned()), InputAtom::Key(ENTER)],
        guard: MutationGuard {
            request_id: Some(
                Uuid::parse_str("019fddda-17c8-7000-87da-8785e71ab5e1")
                    .expect("oversized action request UUID"),
            ),
            if_state: Some(initial.state_hash),
        },
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
        ..ActRequest::default()
    };
    let error = session
        .act(request.clone())
        .expect_err("oversized action projection must fail loudly");
    let details = error.details().expect("terminating-frame failure details");
    let limit = details
        .screen_frame
        .as_ref()
        .and_then(|frame| return frame.limit_kind.clone());

    assert_eq!(error.code(), "screen_frame_resource_limit");
    assert_eq!(details.scope, "action_terminating_screen_frame");
    assert_eq!(limit.as_deref(), Some("serialized_bytes"));
    assert_eq!(error.effect(), MutationEffect::Applied);
    assert_eq!(error.retry(), Retry::DoNotRetry);

    let retry = session
        .act(request)
        .expect_err("cached oversized action retry must retain its failure");

    assert_eq!(retry.code(), error.code());
    assert_eq!(retry.effect(), error.effect());
    assert_eq!(retry.details(), error.details());

    wait_for_output(session.path(), b"COUNT:1:once", Duration::from_secs(5))
        .expect("first action input reached the fixture");
    let frame = session
        .frame(ScreenProjectionRequest::default())
        .expect("post-error screen frame");
    assert!(frame_contains(&frame, "COUNT:1:once"));
    assert!(!frame_contains(&frame, "COUNT:2:once"));

    session
        .stop(StopRequest::default(), Some(Duration::from_secs(5)))
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
        .start(LaunchSpec::new([
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from("printf READY; IFS= read -r line; printf '\\r\\nDONE:%s' \"$line\""),
        ]))
        .expect("launch action quality fixture");

    wait_for_output(session.path(), b"READY", Duration::from_secs(5))
        .expect("action quality fixture ready");
    let result = session
        .act(ActRequest {
            atoms: vec![InputAtom::Text("once".to_owned()), InputAtom::Key(ENTER)],
            postcondition: ActionCondition::Screen {
                predicate: ScreenPredicate::Contains {
                    text: "never-matches".to_owned(),
                },
                timeout: Duration::from_secs(5),
            },
            allow_incomplete: true,
            ..ActRequest::default()
        })
        .expect("child-terminated action result");

    let ActionPostconditionResult::Screen {
        quality:
            ScreenPostconditionQuality::ChildCompleted | ScreenPostconditionQuality::OutputClosed,
        ..
    } = &result.postcondition
    else {
        panic!("unexpected action completion: {:?}", result.postcondition);
    };

    let frame = result.frame().expect("frame");
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
        .start(LaunchSpec::new([
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from(command),
        ]))
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
        .start(LaunchSpec::new(command).geometry(fixture.region.width, fixture.region.height))
        .expect("launch Rust action fixture");

    wait_for_output(
        session.path(),
        fixture.ready_text.as_bytes(),
        Duration::from_secs(5),
    )
    .expect("Rust fixture ready");
    let initial = session
        .frame(ScreenProjectionRequest::default())
        .expect("initial Rust frame");

    let atoms: Vec<InputAtom> = fixture
        .input
        .iter()
        .map(|atom| {
            return match atom {
                FixtureAtom::Text { text } => InputAtom::Text(text.clone()),
                FixtureAtom::Key { key } => InputAtom::Key(
                    KeyChord::parse(key)
                        .expect("valid fixture key")
                        .expect("named fixture key"),
                ),
            };
        })
        .collect();

    let request = ActRequest {
        atoms,
        guard: MutationGuard {
            request_id: Some(fixture.request_id),
            if_state: Some(initial.state_hash),
        },
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
        ..ActRequest::default()
    };
    let result = session.act(request.clone()).expect("Rust atomic action");
    let retry = session.act(request).expect("idempotent Rust action retry");
    let stable = session
        .capture_stable(StableCaptureOptions {
            stable_for: Duration::from_millis(20),
            timeout: Duration::from_secs(2),
            projection: ScreenProjectionRequest::default(),
            allow_incomplete: false,
        })
        .expect("Rust stable capture");

    let frame = result.frame().expect("frame");
    let matched = matches!(
        result.postcondition,
        ActionPostconditionResult::Screen {
            matched: true,
            quality: ScreenPostconditionQuality::Matched
                | ScreenPostconditionQuality::MatchedUnsettled,
            ..
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
        .stop(StopRequest::default(), Some(Duration::from_secs(5)))
        .expect("stop Rust fixture");

    return summary;
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
        ScreenFrameData::None => return false,
    }
}
