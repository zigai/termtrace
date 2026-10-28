mod support;

use std::{sync::mpsc, thread, time::Duration};
use support::{
    enter, events, screen_text, shell, stop, terminal, wait_text, Launch, Live, ECHO, TIMEOUT,
};
use termtrace_sdk::{
    events::Event,
    input::{ActRequest, ActionCondition, InputAtom, MouseButton, MutationGuard},
    screen::ScreenPredicate,
    session::{send_step_request_id, LaunchSpec, SendRequest, SendStep, Session},
    ByteString, ControlCommand, ControlPayload, ControlRequest, Uuid,
};

const REPORTER: &str = "import os, sys, tty
fd = sys.stdin.fileno()
tty.setraw(fd)
os.write(1, b'\\x1b[?1000h\\x1b[?1006h\\x1b[?1004hREPORTER\\r\\n')
while True:
    data = os.read(fd, 256)
    if not data:
        break
    os.write(1, b'EV=' + data.replace(b'\\x1b', b'^') + b'\\r\\n')
";

fn reporter(name: &str) -> Live {
    let session = terminal(name)
        .launch(LaunchSpec::new(["python3", "-c", REPORTER]))
        .expect("launch reporter");
    wait_text(&session, "REPORTER");

    return session;
}

fn echo(name: &str) -> Live {
    let session = terminal(name)
        .launch(LaunchSpec::new(shell(ECHO)))
        .expect("launch echo");
    wait_text(&session, "READY");

    return session;
}

fn applied_bytes(payload: ControlPayload) -> Vec<u8> {
    let ControlPayload::InputApplied { data, .. } = payload else {
        panic!("input was not applied: {payload:?}");
    };

    return data.as_bytes().to_vec();
}

fn line(session: &Session, text: &str) {
    session
        .act(ActRequest::new().text(text).key(enter()).until_screen(
            ScreenPredicate::Contains {
                text: format!("GOT:{text}"),
            },
            TIMEOUT,
        ))
        .expect("line reaches the app");
}

fn marker_events(session: &Session, label: &str) -> Vec<Uuid> {
    return events(session)
        .into_iter()
        .filter_map(|event| {
            return match event {
                Event::Marker {
                    operation_id,
                    label: found,
                } if found == label => Some(operation_id),
                _ => None,
            };
        })
        .collect();
}

#[test]
fn pointer_click_reaches_an_app_that_reports_the_mouse() {
    let mut session = reporter("input-click");
    let guard = MutationGuard::default();

    let bytes = applied_bytes(
        session
            .pointer()
            .click(4, 2, MouseButton::Left, true, &guard)
            .expect("click"),
    );

    assert_eq!(bytes, b"\x1b[<0;5;3M\x1b[<0;5;3m");
    wait_text(&session, "^[<0;5;3M");
    wait_text(&session, "^[<0;5;3m");
    stop(&mut session);

    let mut plain = echo("input-click-disabled");
    let noop = plain
        .pointer()
        .click(4, 2, MouseButton::Left, false, &guard)
        .expect("click without reporting");
    assert!(
        matches!(noop, ControlPayload::InputNoop { ref reason, .. } if reason == "pointer_reporting_disabled"),
        "{noop:?}"
    );
    assert!(plain
        .pointer()
        .click(4, 2, MouseButton::Left, true, &guard)
        .is_err());
    stop(&mut plain);
}

#[test]
fn pointer_scroll_sends_signed_wheel_ticks() {
    let mut session = reporter("input-scroll");
    let guard = MutationGuard::default();

    let up = applied_bytes(
        session
            .pointer()
            .scroll(1, 1, -2, true, &guard)
            .expect("scroll up"),
    );
    let down = applied_bytes(
        session
            .pointer()
            .scroll(7, 3, 1, true, &guard)
            .expect("scroll down"),
    );

    assert_eq!(up, b"\x1b[<64;2;2M\x1b[<64;2;2M");
    assert_eq!(down, b"\x1b[<65;8;4M");
    wait_text(&session, "^[<64;2;2M");
    wait_text(&session, "^[<65;8;4M");
    stop(&mut session);
}

#[test]
fn pointer_focus_reports_focus_changes_only_when_requested() {
    let mut session = reporter("input-focus");
    let guard = MutationGuard::default();

    let gained = applied_bytes(
        session
            .pointer()
            .focus(true, true, &guard)
            .expect("focus in"),
    );
    wait_text(&session, "EV=^[I");
    let lost = applied_bytes(
        session
            .pointer()
            .focus(false, true, &guard)
            .expect("focus out"),
    );
    wait_text(&session, "EV=^[O");

    assert_eq!(gained, b"\x1b[I");
    assert_eq!(lost, b"\x1b[O");
    stop(&mut session);

    let mut plain = echo("input-focus-disabled");
    let noop = plain
        .pointer()
        .focus(true, false, &guard)
        .expect("focus without reporting");
    assert!(matches!(noop, ControlPayload::InputNoop { .. }), "{noop:?}");
    stop(&mut plain);
}

#[test]
fn send_applies_steps_in_order_and_reports_the_failed_step() {
    let mut session = echo("input-send");
    let send_id = Uuid::now_v7();
    let request = SendRequest {
        steps: vec![
            SendStep::Input(vec![
                InputAtom::Text("first".to_owned()),
                InputAtom::Key(enter()),
            ]),
            SendStep::Command(ControlCommand::Mark {
                label: "between".to_owned(),
            }),
            SendStep::Input(vec![
                InputAtom::Text("second".to_owned()),
                InputAtom::Key(enter()),
            ]),
        ],
        guard: MutationGuard {
            request_id: Some(send_id),
            if_state: None,
        },
        postcondition: ActionCondition::Screen {
            predicate: ScreenPredicate::Contains {
                text: "GOT:second".to_owned(),
            },
            timeout: TIMEOUT,
        },
        ..SendRequest::default()
    };

    session.send(request).expect("send");

    let text = screen_text(&session);
    let first = text.find("GOT:first").expect("first line");
    let second = text.find("GOT:second").expect("second line");
    assert!(first < second, "{text}");
    let log = events(&session);
    let position = |wanted: &[u8]| {
        return log
            .iter()
            .position(|event| {
                return matches!(event, Event::InputIntent { data, .. } if data.as_bytes() == wanted);
            })
            .expect("input intent");
    };
    let marker = log
        .iter()
        .position(|event| return matches!(event, Event::Marker { label, .. } if label == "between"))
        .expect("marker");
    assert!(position(b"first") < marker && marker < position(b"second"));
    assert_eq!(
        marker_events(&session, "between"),
        [send_step_request_id(send_id, 1)]
    );

    let failing_id = Uuid::now_v7();
    let failure = session
        .send(SendRequest {
            steps: vec![
                SendStep::Input(vec![
                    InputAtom::Text("kept".to_owned()),
                    InputAtom::Key(enter()),
                ]),
                SendStep::Command(ControlCommand::Mark {
                    label: String::new(),
                }),
                SendStep::Input(vec![
                    InputAtom::Text("dropped".to_owned()),
                    InputAtom::Key(enter()),
                ]),
            ],
            guard: MutationGuard {
                request_id: Some(failing_id),
                if_state: None,
            },
            ..SendRequest::default()
        })
        .expect_err("an empty marker label fails");

    assert_eq!(failure.step, 1);
    assert_eq!(failure.request_id, send_step_request_id(failing_id, 1));
    wait_text(&session, "GOT:kept");
    line(&session, "sync");
    assert!(!screen_text(&session).contains("GOT:dropped"));
    stop(&mut session);
}

#[test]
fn replaying_a_request_id_returns_the_first_result_without_reapplying() {
    let mut session = echo("input-dedup");
    let request_id = Uuid::now_v7();
    let mut paste = ControlRequest::new(ControlCommand::Paste {
        data: ByteString::new(b"once-only\n".to_vec()),
    });
    paste.request_id = request_id;

    let first = session.control(&paste).expect("paste");
    let replay = session.control(&paste).expect("replayed paste");
    line(&session, "sync");

    assert_eq!(first, replay);
    let ControlPayload::InputApplied { operation_id, .. } = first else {
        panic!("paste was not applied: {first:?}");
    };
    assert_eq!(operation_id, request_id);
    assert_eq!(screen_text(&session).matches("GOT:once-only").count(), 1);

    let guard = MutationGuard {
        request_id: Some(Uuid::now_v7()),
        if_state: None,
    };
    let mark = session.mark("checkpoint", &guard).expect("mark");
    let again = session.mark("checkpoint", &guard).expect("replayed mark");
    assert_eq!(mark.event_seq, again.event_seq);
    assert_eq!(
        marker_events(&session, "checkpoint"),
        [guard.request_id.expect("request id")]
    );

    let collision = session
        .mark("different", &guard)
        .expect_err("a reused id with another command is rejected");
    assert_eq!(collision.code(), "request_id_collision");
    assert!(marker_events(&session, "different").is_empty());
    stop(&mut session);
}

#[test]
fn an_abandoned_act_keeps_its_admitted_input_and_a_retry_reports_it() {
    let mut session = echo("input-cancel");
    let request = ActRequest {
        guard: MutationGuard {
            request_id: Some(Uuid::now_v7()),
            if_state: None,
        },
        allow_incomplete: true,
        ..ActRequest::new().text("slow").key(enter()).until_screen(
            ScreenPredicate::Contains {
                text: "NEVER".to_owned(),
            },
            Duration::from_millis(800),
        )
    };
    let request_id = request.guard.request_id.expect("request id");
    let path = session.path().to_path_buf();
    let helper = session.helper().to_path_buf();
    let background = request.clone();
    let (sender, receiver) = mpsc::channel();
    let worker = thread::spawn(move || {
        let attached = Session::attach(path, helper).expect("attach");
        let _ = sender.send(attached.act(background));
    });

    assert!(
        receiver.recv_timeout(Duration::from_millis(50)).is_err(),
        "the act should still be waiting for its postcondition"
    );
    drop(receiver);
    worker.join().expect("abandoned act finishes");

    let retry = session.act(request).expect("retry");
    assert_eq!(retry.request_id, request_id);
    assert!(!retry.is_matched());
    line(&session, "sync");
    assert_eq!(screen_text(&session).matches("GOT:slow").count(), 1);
    stop(&mut session);
}
