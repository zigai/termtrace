//! Shared public-SDK atomic-action conformance matrix.

use crate::support::*;
use serde::Deserialize;
use std::{collections::BTreeMap, error::Error, thread, time::Instant};
use termtrace_model::{
    execution::event::EventMetadata,
    execution::session::ResourceLimitKind,
    protocol::control::{
        ActionPostconditionResult, ProcessSignal, ScreenPostconditionQuality, StableCaptureQuality,
    },
    terminal::observation::CaptureQuality,
    terminal::screen::{FrameDetail, ScreenFrame, ScreenFrameContent, ScreenFrameSource},
    terminal::state::MouseButton,
};
use termtrace_sdk::{
    contracts::ActionDraft,
    events::EventsService,
    session::{CommandPostcondition, SendLastStep, SendRequest, SendStep},
    MutationEffect, MutationGuard, PointerOutcome, Reaction, Retry,
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
            settle: Duration::ZERO,
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

fn marker_labels(session: &termtrace_sdk::Session) -> termtrace_sdk::Result<Vec<String>> {
    let (events, _) = session.events().page(0, 4096, true)?;

    return Ok(events
        .into_iter()
        .filter(|event| return event.event_type == "marker")
        .filter_map(|event| {
            let Some(EventMetadata::Marker { label, .. }) = event.metadata else {
                return None;
            };
            return Some(label);
        })
        .collect());
}

#[test]
fn send_applies_mixed_steps_once_and_replays_them_on_retry() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = ClientTerminal::new(
        env!("CARGO_BIN_EXE_termtrace"),
        temporary.path().join("send-evidence"),
    );

    let mut session = terminal
        .start(LaunchSpec::new([
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from(
                "trap 'exit 0' TERM; printf READY; count=0; while IFS= read -r line; do count=$((count + 1)); printf '\\r\\nCOUNT:%s:%s' \"$count\" \"$line\"; done",
            ),
        ]))
        .expect("launch send fixture");

    wait_for_output(session.path(), b"READY", Duration::from_secs(5)).expect("send fixture ready");
    let initial = session
        .frame(ScreenProjectionRequest::default())
        .expect("initial send frame");

    let request = SendRequest {
        steps: vec![
            SendStep::Command(ControlCommand::Mark {
                label: "before".to_owned(),
            }),
            SendStep::Input(vec![
                InputAtom::Text("first".to_owned()),
                InputAtom::Key(ENTER),
            ]),
            SendStep::Command(ControlCommand::Mark {
                label: "between".to_owned(),
            }),
            SendStep::Input(vec![
                InputAtom::Text("second".to_owned()),
                InputAtom::Key(ENTER),
            ]),
        ],
        guard: MutationGuard {
            request_id: Some(
                Uuid::parse_str("019fddda-17c8-7000-87da-8785e71ab5e2").expect("send request UUID"),
            ),
            if_state: Some(initial.state_hash),
        },
        postcondition: ActionCondition::Screen {
            predicate: ScreenPredicate::Contains {
                text: "COUNT:2:second".to_owned(),
            },
            timeout: Duration::from_secs(5),
            settle: Duration::ZERO,
        },
        ..SendRequest::default()
    };
    let SendLastStep::Action(first) = session.send(request.clone()).expect("first send").last
    else {
        panic!("a send ending in input reports its action outcome");
    };

    assert!(first.is_matched());

    let SendLastStep::Action(retry) = session.send(request).expect("retried send").last else {
        panic!("a retried send ending in input reports its action outcome");
    };

    assert_eq!(retry.request_id, first.request_id);
    assert_eq!(retry.input, first.input);

    let frame = session
        .frame(ScreenProjectionRequest::default())
        .expect("post-retry frame");
    assert!(frame_contains(&frame, "COUNT:2:second"));
    assert!(!frame_contains(&frame, "COUNT:3"));
    assert_eq!(
        marker_labels(&session).expect("read markers"),
        ["before", "between"]
    );

    session
        .stop(StopRequest::default(), Some(Duration::from_secs(5)))
        .expect("stop send fixture");
}

#[test]
fn send_reports_the_failed_step_after_applying_earlier_ones() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = ClientTerminal::new(
        env!("CARGO_BIN_EXE_termtrace"),
        temporary.path().join("send-failure-evidence"),
    );

    let mut session = terminal
        .start(LaunchSpec::new([
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from("trap 'exit 0' TERM; printf READY; while :; do sleep 1; done"),
        ]))
        .expect("launch send failure fixture");

    wait_for_output(session.path(), b"READY", Duration::from_secs(5))
        .expect("send failure fixture ready");
    let failure = session
        .send(SendRequest {
            steps: vec![
                SendStep::Command(ControlCommand::Mark {
                    label: "applied".to_owned(),
                }),
                SendStep::Command(ControlCommand::Mark {
                    label: "   ".to_owned(),
                }),
            ],
            ..SendRequest::default()
        })
        .expect_err("a blank marker label is rejected");

    assert_eq!(failure.step, 1);
    assert_eq!(failure.error.code(), "invalid_marker");
    assert_eq!(failure.error.effect(), MutationEffect::NotApplied);
    assert_eq!(marker_labels(&session).expect("read markers"), ["applied"]);

    session
        .stop(StopRequest::default(), Some(Duration::from_secs(5)))
        .expect("stop send failure fixture");
}

fn start_delayed_reaction(
    temporary: &Path,
    name: &str,
) -> Result<termtrace_sdk::Session, Box<dyn Error>> {
    let terminal = ClientTerminal::new(env!("CARGO_BIN_EXE_termtrace"), temporary.join(name));
    let session = terminal
        .start(LaunchSpec::new([
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from(
                "trap 'exit 0' TERM; stty -echo; printf READY; IFS= read -r line; sleep 0.7; printf '\\r\\nDONE:%s' \"$line\"; while :; do sleep 1; done",
            ),
        ]))?;

    wait_for_output(session.path(), b"READY", Duration::from_secs(5))?;
    return Ok(session);
}

#[test]
fn idle_action_waits_for_a_reaction_slower_than_its_stable_interval() {
    let temporary = TempDir::new().expect("temporary directory");
    let mut session = start_delayed_reaction(temporary.path(), "slow-idle-evidence")
        .expect("start delayed reaction fixture");

    let outcome = session
        .act(
            ActRequest::new()
                .text("go")
                .key(ENTER)
                .until_idle(Duration::from_millis(200), Duration::from_secs(5))
                .projection(ScreenProjectionRequest::default()),
        )
        .expect("idle action");
    assert!(outcome.is_matched());

    let frame = outcome.frame().expect("terminating frame");
    assert!(
        frame_contains(frame, "DONE:go"),
        "stale idle frame: {frame:?}"
    );

    session
        .stop(StopRequest::default(), Some(Duration::from_secs(5)))
        .expect("stop slow idle fixture");
}

fn idle_after(atoms: Vec<InputAtom>, reaction_timeout: Duration) -> ActRequest {
    return ActRequest {
        atoms,
        postcondition: ActionCondition::Idle {
            stable_for: Duration::from_millis(100),
            timeout: Duration::from_secs(10),
            reaction: Reaction {
                changed_from: None,
                timeout: reaction_timeout,
            },
        },
        projection: ScreenProjectionRequest::default(),
        allow_incomplete: true,
        ..ActRequest::default()
    };
}

#[test]
fn idle_action_settles_as_unchanged_after_the_reaction_window() {
    let temporary = TempDir::new().expect("temporary directory");
    let mut session = start_delayed_reaction(temporary.path(), "unchanged-idle-evidence")
        .expect("start delayed reaction fixture");

    let started = Instant::now();
    let outcome = session
        .act(idle_after(
            vec![InputAtom::Text("x".to_owned())],
            Duration::from_millis(300),
        ))
        .expect("unchanged idle action");

    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(outcome.is_matched());
    assert!(matches!(
        outcome.postcondition,
        ActionPostconditionResult::Idle {
            quality: StableCaptureQuality::Unchanged,
            ..
        }
    ));

    session
        .stop(StopRequest::default(), Some(Duration::from_secs(5)))
        .expect("stop unchanged idle fixture");
}

#[test]
fn idle_action_counts_output_that_redraws_an_identical_screen_as_a_reaction() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = ClientTerminal::new(
        env!("CARGO_BIN_EXE_termtrace"),
        temporary.path().join("redraw-idle-evidence"),
    );

    let mut session = terminal
        .start(LaunchSpec::new([
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from(
                "trap 'exit 0' TERM; stty -echo -icanon min 1; printf READY; while :; do dd bs=1 count=1 >/dev/null 2>&1; printf '\\rREADY'; done",
            ),
        ]))
        .expect("launch redraw fixture");

    wait_for_output(session.path(), b"READY", Duration::from_secs(5))
        .expect("redraw fixture ready");
    let before = session
        .frame(ScreenProjectionRequest::default())
        .expect("frame before redraw");
    let outcome = session
        .act(idle_after(
            vec![InputAtom::Text("x".to_owned())],
            Duration::from_secs(5),
        ))
        .expect("redraw idle action");
    assert_eq!(outcome.state_hash(), Some(before.state_hash.as_str()));
    assert!(matches!(
        outcome.postcondition,
        ActionPostconditionResult::Idle {
            quality: StableCaptureQuality::Stable,
            ..
        }
    ));

    session
        .stop(StopRequest::default(), Some(Duration::from_secs(5)))
        .expect("stop redraw fixture");
}

#[test]
fn idle_send_ending_in_a_command_waits_for_a_change_since_the_send_began() {
    let temporary = TempDir::new().expect("temporary directory");
    let mut session = start_delayed_reaction(temporary.path(), "command-idle-evidence")
        .expect("start delayed reaction fixture");

    let outcome = session
        .send(SendRequest {
            steps: vec![
                SendStep::Input(vec![
                    InputAtom::Text("go".to_owned()),
                    InputAtom::Key(ENTER),
                ]),
                SendStep::Command(ControlCommand::Mark {
                    label: "sent".to_owned(),
                }),
            ],
            postcondition: ActionCondition::Idle {
                stable_for: Duration::from_millis(200),
                timeout: Duration::from_secs(5),
                reaction: Reaction::new(),
            },
            projection: ScreenProjectionRequest::default(),
            ..SendRequest::default()
        })
        .expect("send ending in a command");

    let SendLastStep::Command { postcondition, .. } = outcome.last else {
        panic!("a send ending in a marker reports its command");
    };
    let Some(CommandPostcondition::Idle(Ok(capture))) = postcondition.as_deref() else {
        panic!("unexpected command postcondition: {postcondition:?}");
    };

    assert_eq!(capture.quality, StableCaptureQuality::Stable);
    assert!(frame_contains(&capture.frame, "DONE:go"));

    session
        .stop(StopRequest::default(), Some(Duration::from_secs(5)))
        .expect("stop command idle fixture");
}

#[test]
fn send_reports_screen_changes_for_input_and_command_endings() {
    let temporary = TempDir::new().expect("temporary directory");
    let mut session = start_delayed_reaction(temporary.path(), "changes-evidence")
        .expect("start delayed reaction fixture");

    let send = |steps| {
        return session
            .send(SendRequest {
                steps,
                postcondition: ActionCondition::Screen {
                    predicate: ScreenPredicate::Contains {
                        text: "DONE:go".to_owned(),
                    },
                    timeout: Duration::from_secs(5),
                    settle: Duration::ZERO,
                },
                report_changes: true,
                ..SendRequest::default()
            })
            .expect("send with changes");
    };
    let input_ending = send(vec![SendStep::Input(vec![
        InputAtom::Text("go".to_owned()),
        InputAtom::Key(ENTER),
    ])]);
    let changes = input_ending.changes.expect("changes for an input ending");

    assert_ne!(changes.before_state_hash, changes.after_state_hash);
    assert!(changes.moves.is_empty());
    assert!(
        changes
            .rows
            .iter()
            .any(|row| return row.row == 1 && row.after.as_deref() == Some("DONE:go")),
        "unexpected changes: {changes:?}"
    );

    let command_ending = send(vec![SendStep::Command(ControlCommand::Mark {
        label: "after".to_owned(),
    })]);
    let changes = command_ending
        .changes
        .expect("changes for a command ending");

    assert!(changes.is_empty(), "a marker changes nothing: {changes:?}");

    session
        .stop(StopRequest::default(), Some(Duration::from_secs(5)))
        .expect("stop changes fixture");
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
                settle: Duration::ZERO,
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
            settle: Duration::ZERO,
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
            reaction: None,
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
        ScreenFrameContent::Text { rows } => {
            return rows.iter().any(|row| return row.text.contains(expected));
        }
        ScreenFrameContent::Spans { rows } => {
            return rows.iter().any(|row| return row.text.contains(expected));
        }
        ScreenFrameContent::Cells { rows } => {
            return rows.iter().any(|row| return row.text.contains(expected));
        }
        ScreenFrameContent::None => return false,
    }
}

#[derive(Debug, Deserialize)]
struct InteractionFixture {
    echo_command: Vec<String>,
    raw_command: Vec<String>,
    mouse_command: Vec<String>,
    environment_command: Vec<String>,
    ready_text: String,
    raw_bytes_hex: String,
    raw_bytes_echo: String,
    raw_bytes_base64: String,
    set_bytes_hex: String,
    set_bytes_echo: String,
    activation_text: String,
    errors: BTreeMap<String, ErrorFacts>,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
struct ErrorFacts {
    kind: String,
    code: String,
    effect: serde_json::Value,
    retry: String,
}

impl ErrorFacts {
    #[expect(
        clippy::expect_used,
        reason = "test-only conformance helper reports fixture and SDK failures"
    )]
    fn of(error: &termtrace_sdk::Error) -> Self {
        return Self {
            kind: error.kind().as_str().to_owned(),
            code: error.code().to_owned(),
            effect: serde_json::to_value(error.effect()).expect("effect JSON"),
            retry: serde_json::to_value(error.retry())
                .expect("retry JSON")
                .as_str()
                .expect("retry name")
                .to_owned(),
        };
    }
}

#[expect(
    clippy::expect_used,
    reason = "test-only conformance helper reports fixture and SDK failures"
)]
fn interaction_fixture() -> InteractionFixture {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/conformance/actions/interaction-control.json");
    return serde_json::from_slice(&fs_err::read(&path).expect("interaction fixture"))
        .expect("interaction fixture JSON");
}

#[expect(
    clippy::expect_used,
    reason = "test-only conformance helper reports fixture and SDK failures"
)]
fn hex_bytes(hex: &str) -> Vec<u8> {
    return (0..hex.len())
        .step_by(2)
        .map(|index| {
            let pair = hex.get(index..index + 2).expect("hex pair");
            return u8::from_str_radix(pair, 16).expect("hex byte");
        })
        .collect();
}

#[expect(
    clippy::expect_used,
    reason = "test-only conformance helper reports fixture and SDK failures"
)]
fn start_fixture(
    temporary: &Path,
    name: &str,
    spec: LaunchSpec,
    ready: &str,
) -> termtrace_sdk::Session {
    let terminal = ClientTerminal::new(env!("CARGO_BIN_EXE_termtrace"), temporary.join(name));
    let session = terminal.start(spec).expect("launch interaction fixture");
    wait_for_output(session.path(), ready.as_bytes(), Duration::from_secs(5))
        .expect("interaction fixture ready");
    return session;
}

#[expect(
    clippy::expect_used,
    reason = "test-only conformance helper reports fixture and SDK failures"
)]
fn wait_for_frame_text(session: &termtrace_sdk::Session, text: &str) -> ScreenFrame {
    let wait = session
        .wait_screen(
            ScreenPredicate::Contains {
                text: text.to_owned(),
            },
            Duration::ZERO,
            Duration::from_secs(5),
        )
        .expect("screen wait");
    assert!(wait.matched, "screen shows {text:?}");

    return wait.frame;
}

#[expect(
    clippy::expect_used,
    reason = "test-only conformance helper reports fixture and SDK failures"
)]
fn stop_fixture(mut session: termtrace_sdk::Session) {
    session
        .stop(StopRequest::default(), Some(Duration::from_secs(5)))
        .expect("stop interaction fixture");
}

#[test]
fn interaction_failures_report_the_shared_fixture_facts_without_side_effects() {
    let fixture = interaction_fixture();
    let temporary = TempDir::new().expect("temporary directory");
    let session = start_fixture(
        temporary.path(),
        "errors",
        LaunchSpec::new(&fixture.echo_command),
        &fixture.ready_text,
    );

    let stale = session
        .act(ActRequest {
            guard: MutationGuard {
                request_id: Some(Uuid::now_v7()),
                if_state: Some("0".repeat(64)),
            },
            ..ActRequest::new().text("stale\n")
        })
        .expect_err("a stale guard is rejected");
    assert_eq!(ErrorFacts::of(&stale), fixture.errors["stale"]);

    let request_id = Uuid::now_v7();
    let applied = session
        .act(ActRequest {
            guard: MutationGuard {
                request_id: Some(request_id),
                if_state: None,
            },
            ..ActRequest::new().text("first\n")
        })
        .expect("first action");
    let collision = session
        .act(ActRequest {
            guard: MutationGuard {
                request_id: Some(request_id),
                if_state: None,
            },
            ..ActRequest::new().text("other\n")
        })
        .expect_err("a reused identity with other input is rejected");
    assert_eq!(ErrorFacts::of(&collision), fixture.errors["collision"]);

    let strict = session
        .pointer()
        .click(0, 0, MouseButton::Left, true, &MutationGuard::default())
        .expect_err("strict pointer input without mouse reporting is rejected");
    assert_eq!(ErrorFacts::of(&strict), fixture.errors["strict_pointer"]);

    let incomplete = session
        .act(ActRequest {
            postcondition: ActionCondition::Screen {
                predicate: ScreenPredicate::Contains {
                    text: "NEVER".to_owned(),
                },
                timeout: Duration::from_millis(200),
                settle: Duration::ZERO,
            },
            ..ActRequest::new().text("late\n")
        })
        .expect_err("an unmet postcondition is incomplete");
    assert_eq!(ErrorFacts::of(&incomplete), fixture.errors["incomplete"]);

    let termtrace_sdk::Error::PostconditionIncomplete { result } = &incomplete else {
        panic!("incomplete failures carry the applied result");
    };

    assert_eq!(result.input.atom_count, 1);

    let mut draft = ActionDraft::new();
    let _sealed = draft.seal();
    let sealed = draft
        .edit(|request| request.atoms.push(InputAtom::Text("x".to_owned())))
        .expect_err("a sealed draft rejects changes");
    assert_eq!(ErrorFacts::of(&sealed), fixture.errors["sealed"]);

    let frame = wait_for_frame_text(&session, "GOT:late");
    assert!(frame_contains(&frame, "GOT:first"));
    assert!(!frame_contains(&frame, "GOT:stale"));
    assert!(!frame_contains(&frame, "GOT:other"));
    assert_eq!(applied.input.atom_count, 1);

    stop_fixture(session);
}

#[test]
fn raw_bytes_reach_the_child_and_canonical_events_exactly() {
    let fixture = interaction_fixture();
    let temporary = TempDir::new().expect("temporary directory");
    let session = start_fixture(
        temporary.path(),
        "raw",
        LaunchSpec::new(&fixture.raw_command),
        &fixture.ready_text,
    );

    let result = session
        .act(ActRequest::new().bytes(hex_bytes(&fixture.raw_bytes_hex)))
        .expect("raw byte action");

    assert_eq!(result.input.bytes_written, 4);
    wait_for_frame_text(&session, &fixture.raw_bytes_echo);

    let page = EventsService::read_page(session.path(), None, 4096).expect("canonical events");
    let written = serde_json::to_string(&page.events).expect("events JSON");
    assert!(written.contains(&fixture.raw_bytes_base64));

    stop_fixture(session);
}

#[test]
fn typed_pointer_operations_report_applied_noop_and_activated_outcomes() {
    let fixture = interaction_fixture();
    let temporary = TempDir::new().expect("temporary directory");
    let session = start_fixture(
        temporary.path(),
        "mouse",
        LaunchSpec::new(&fixture.mouse_command),
        &fixture.ready_text,
    );

    wait_for_frame_text(&session, &fixture.ready_text);

    let PointerOutcome::Applied {
        bytes: multi_bytes, ..
    } = session
        .pointer()
        .multi_click(3, 0, MouseButton::Left, 2, true, &MutationGuard::default())
        .expect("multi-click")
    else {
        panic!("reported multi-click input is applied");
    };
    let multi_bytes = multi_bytes.as_bytes().to_vec();
    assert_eq!(
        multi_bytes
            .windows(3)
            .filter(|window| return window == b"\x1b[<")
            .count(),
        4,
        "two presses and two releases"
    );

    let PointerOutcome::Applied { bytes: drag, .. } = session
        .pointer()
        .drag(
            1,
            0,
            5,
            0,
            MouseButton::Left,
            3,
            true,
            &MutationGuard::default(),
        )
        .expect("drag")
    else {
        panic!("reported drag input is applied");
    };

    assert!(drag
        .as_bytes()
        .windows(4)
        .any(|window| return window == b"<32;"));

    let PointerOutcome::Activated {
        text,
        start_column,
        end_column,
        row,
        ..
    } = session
        .pointer()
        .activate(
            fixture.activation_text.clone(),
            true,
            true,
            &MutationGuard::default(),
        )
        .expect("activate")
    else {
        panic!("visible text activates");
    };

    assert_eq!(
        (text.as_str(), start_column, end_column, row),
        (fixture.activation_text.as_str(), 0, 4, 0)
    );
    stop_fixture(session);

    let echo = start_fixture(
        temporary.path(),
        "no-mouse",
        LaunchSpec::new(&fixture.echo_command),
        &fixture.ready_text,
    );

    let request_id = Uuid::now_v7();
    let payload = echo
        .pointer()
        .click(
            0,
            0,
            MouseButton::Left,
            false,
            &MutationGuard {
                request_id: Some(request_id),
                if_state: None,
            },
        )
        .expect("non-strict click");

    let PointerOutcome::Noop { reason, .. } =
        PointerOutcome::from_payload(request_id, payload).expect("typed outcome")
    else {
        panic!("unreported pointer input is a no-op");
    };

    assert_eq!(reason, "pointer_reporting_disabled");

    stop_fixture(echo);
}

#[test]
fn session_controls_resize_signal_and_mark_the_live_session() {
    let fixture = interaction_fixture();
    let temporary = TempDir::new().expect("temporary directory");
    let session = start_fixture(
        temporary.path(),
        "controls",
        LaunchSpec::new(&fixture.echo_command),
        &fixture.ready_text,
    );

    session
        .resize(100, 30, &MutationGuard::default())
        .expect("resize");
    let frame = session
        .frame(ScreenProjectionRequest::default())
        .expect("resized frame");
    assert_eq!((frame.geometry.cols, frame.geometry.rows), (100, 30));

    session
        .signal(ProcessSignal::Interrupt, &MutationGuard::default())
        .expect("signal");

    wait_for_frame_text(&session, "SIGNAL:INT");

    let marked = session
        .mark("checkpoint", &MutationGuard::default())
        .expect("mark");

    assert_eq!(marked.label, "checkpoint");
    assert_eq!(
        marker_labels(&session).expect("read markers"),
        ["checkpoint"]
    );

    stop_fixture(session);
}

#[test]
fn cleared_launch_environment_keeps_exact_bytes_and_reserved_terminal_names() {
    let fixture = interaction_fixture();
    let temporary = TempDir::new().expect("temporary directory");
    let spec = LaunchSpec::new(&fixture.environment_command)
        .clear_environment()
        .environment_variable("TT_SET", hex_bytes(&fixture.set_bytes_hex));

    let session = start_fixture(temporary.path(), "environment", spec, "DONE");
    let frame = wait_for_frame_text(&session, "DONE");
    assert!(frame_contains(
        &frame,
        &format!("SET= {}", fixture.set_bytes_echo)
    ));
    assert!(frame_contains(&frame, "SECRET=absent"));
    assert!(frame_contains(&frame, "TERM=xterm"));

    stop_fixture(session);
}

#[test]
fn launch_limits_end_the_session_with_their_resource_limit() {
    let temporary = TempDir::new().expect("temporary directory");

    for (name, spec, kind) in [
        (
            "max-duration",
            LaunchSpec::new(["/bin/sh", "-c", "printf READY; sleep 30"])
                .max_duration(Duration::from_millis(400)),
            ResourceLimitKind::MaxDuration,
        ),
        (
            "idle-timeout",
            LaunchSpec::new(["/bin/sh", "-c", "printf READY; sleep 30"])
                .idle_timeout(Duration::from_millis(400)),
            ResourceLimitKind::IdleTimeout,
        ),
    ] {
        let mut session = start_fixture(temporary.path(), name, spec, "READY");
        let completion = session
            .complete(Duration::from_secs(10))
            .expect("limited session completes");
        let limit = completion.resource_limit.expect("resource limit outcome");
        assert_eq!(limit.kind, kind, "{name}");
        assert_eq!(limit.limit, 400, "{name} limit in milliseconds");
    }
}
