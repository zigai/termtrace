//! Resizes owned by an outer terminal: the recorder forwards each request to a supervisor and
//! reports the grid its own terminal then shows.

use super::*;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::{
    io::{BufRead as _, BufReader, Read as _, Write as _},
    os::linux::net::SocketAddrExt as _,
    sync::{Arc, Mutex},
};
use termtrace_model::protocol::{
    control::{ControlResponse, ControlResult},
    mutation::MutationEffect,
    outer_resize::{OuterResizeReply, OuterResizeRequest},
};
use uuid::Uuid;

#[derive(Clone, Copy)]
enum Outer {
    /// Configure the window and let the terminal show this grid.
    Apply(u16, u16),
    /// Configure the window, then show this grid only after a delay.
    ApplyLater(u16, u16, Duration),
    Reject,
    /// Configure the window, but the terminal never changes size.
    ConfigureOnly,
    /// Read the request and close without answering.
    Silent,
}

struct OuterTerminal {
    socket: String,
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    behaviour: Arc<Mutex<Outer>>,
    requests: Arc<Mutex<Vec<OuterResizeRequest>>>,
}

fn pty_size(cols: u16, rows: u16) -> PtySize {
    return PtySize {
        rows,
        cols,
        pixel_width: cols * 7,
        pixel_height: rows * 14,
    };
}

impl OuterTerminal {
    #[allow(
        clippy::expect_used,
        clippy::unwrap_used,
        reason = "test helper should stop when its outer-terminal fixture fails"
    )]
    fn start(master: Box<dyn MasterPty + Send>) -> Self {
        let socket = format!("termtrace-test-outer-{}", Uuid::now_v7().simple());
        let listener = SocketAddr::from_abstract_name(socket.as_bytes())
            .and_then(|address| return UnixListener::bind_addr(&address))
            .expect("outer resize socket");

        let outer = Self {
            socket,
            master: Arc::new(Mutex::new(master)),
            behaviour: Arc::new(Mutex::new(Outer::Apply(0, 0))),
            requests: Arc::new(Mutex::new(Vec::new())),
        };
        let (master, behaviour, requests) = (
            Arc::clone(&outer.master),
            Arc::clone(&outer.behaviour),
            Arc::clone(&outer.requests),
        );

        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.expect("outer resize connection");
                let mut line = String::new();
                BufReader::new(&stream)
                    .read_line(&mut line)
                    .expect("request line");
                let request: OuterResizeRequest = serde_json::from_str(&line).expect("request");
                requests.lock().unwrap().push(request.clone());

                let reply = match *behaviour.lock().unwrap() {
                    Outer::Silent => continue,
                    Outer::Reject => OuterResizeReply::Rejected {
                        request_id: request.request_id,
                        code: "exceeds_native_canvas".to_owned(),
                        message: "the window does not fit".to_owned(),
                    },
                    Outer::Apply(..) | Outer::ApplyLater(..) | Outer::ConfigureOnly => {
                        OuterResizeReply::Configured {
                            request_id: request.request_id,
                            window_width: 800,
                            window_height: 600,
                        }
                    }
                };

                let mut bytes = serde_json::to_vec(&reply).expect("reply");
                bytes.push(b'\n');
                stream.write_all(&bytes).expect("write reply");
                drop(stream);

                match *behaviour.lock().unwrap() {
                    Outer::Apply(cols, rows) => {
                        master
                            .lock()
                            .unwrap()
                            .resize(pty_size(cols, rows))
                            .expect("resize");
                    }
                    Outer::ApplyLater(cols, rows, delay) => {
                        let master = Arc::clone(&master);
                        thread::spawn(move || {
                            thread::sleep(delay);

                            master
                                .lock()
                                .unwrap()
                                .resize(pty_size(cols, rows))
                                .expect("resize");
                        });
                    }
                    Outer::Reject | Outer::ConfigureOnly | Outer::Silent => {}
                }
            }
        });

        return outer;
    }

    #[allow(
        clippy::unwrap_used,
        reason = "test helper should stop when its outer-terminal fixture fails"
    )]
    fn behave(&self, behaviour: Outer) {
        *self.behaviour.lock().unwrap() = behaviour;
    }
}

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when its outer-terminal fixture fails"
)]
fn start_recorder(temporary: &TempDir) -> (PathBuf, OuterTerminal, Box<dyn portable_pty::Child>) {
    let session = temporary.path().join("outer");
    let pty = native_pty_system()
        .openpty(pty_size(80, 24))
        .expect("recorder PTY");

    let mut reader = pty.master.try_clone_reader().expect("PTY reader");
    thread::spawn(move || {
        let mut buffer = [0_u8; 4096];

        while reader.read(&mut buffer).is_ok_and(|read| return read > 0) {}
    });

    let outer = OuterTerminal::start(pty.master);
    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_termtrace"));
    command.args([
        "record",
        "--geometry",
        "inherit",
        "--outer-resize-socket",
        &outer.socket,
    ]);
    command.args([
        "--max-duration",
        "60s",
        "--out",
        session.to_str().expect("UTF-8 path"),
    ]);
    command.args(["--", "/bin/sh", "-c", "while :; do sleep 0.05; done"]);
    let child = pty.slave.spawn_command(command).expect("spawn recorder");
    drop(pty.slave);

    wait_for_control(&session);

    return (session, outer, child);
}

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when its outer-terminal fixture fails"
)]
fn resize(session: &Path, request_id: Uuid, cols: u16, rows: u16) -> ControlResponse {
    let request = ControlRequest {
        request_id,
        ..ControlRequest::new(ControlCommand::Resize {
            cols,
            rows,
            pixel_width: cols * 7,
            pixel_height: rows * 14,
        })
    };

    return termtrace_sdk::send_request_response(session, &request).expect("resize exchange");
}

#[allow(
    clippy::panic,
    reason = "test helper should stop when its outer-terminal fixture fails"
)]
fn failure(response: &ControlResponse) -> (&str, MutationEffect) {
    let ControlResult::Error { code, effect, .. } = &response.result else {
        panic!("resize succeeded: {response:?}");
    };

    return (code, *effect);
}

#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test helper should stop when its outer-terminal fixture fails"
)]
fn resize_events(session: &Path, request_id: Uuid) -> Vec<serde_json::Value> {
    return fs::read_to_string(session.join("events.jsonl"))
        .expect("events")
        .lines()
        .skip(1)
        .map(|line| return serde_json::from_str::<serde_json::Value>(line).expect("event"))
        .filter(|event| {
            return event["event"]["type"]
                .as_str()
                .is_some_and(|kind| return kind.starts_with("resize"))
                && event["event"]["operation_id"] == request_id.to_string();
        })
        .collect();
}

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when its outer-terminal fixture fails"
)]
fn stop(session: &Path, mut child: Box<dyn portable_pty::Child>) {
    termtrace_sdk::send_request(session, &ControlRequest::new(ControlCommand::Stop)).expect("stop");

    child.wait().expect("recorder exit");
}

#[test]
fn observed_outer_grid_acknowledges_once_and_retries_replay_the_answer() {
    let temporary = TempDir::new().expect("temporary directory");
    let (session, outer, child) = start_recorder(&temporary);
    outer.behave(Outer::Apply(100, 30));
    let request_id = Uuid::now_v7();
    let applied = resize(&session, request_id, 100, 30);
    assert!(
        matches!(applied.result, ControlResult::Ok { .. }),
        "{applied:?}"
    );
    assert_eq!(resize_events(&session, request_id).len(), 2);

    let retried = resize(&session, request_id, 100, 30);
    assert_eq!(retried.result, applied.result);
    assert_eq!(
        outer.requests.lock().unwrap().len(),
        1,
        "a retry never reaches the window"
    );
    assert_eq!(resize_events(&session, request_id).len(), 2);

    outer.behave(Outer::Apply(90, 28));
    let different = Uuid::now_v7();
    assert_eq!(
        failure(&resize(&session, different, 120, 34)),
        ("resize_postcondition_unmet", MutationEffect::Applied)
    );

    let events = resize_events(&session, different);
    assert_eq!(events[0]["event"]["cols"], 90);
    stop(&session, child);
}

#[test]
fn rejected_outer_resize_has_no_effect() {
    let temporary = TempDir::new().expect("temporary directory");
    let (session, outer, child) = start_recorder(&temporary);
    outer.behave(Outer::Reject);
    let request_id = Uuid::now_v7();
    let rejected = resize(&session, request_id, 400, 200);
    assert_eq!(
        failure(&rejected),
        ("exceeds_native_canvas", MutationEffect::NotApplied)
    );
    assert!(resize_events(&session, request_id).is_empty());
    assert_eq!(
        resize(&session, request_id, 400, 200).result,
        rejected.result
    );

    stop(&session, child);
}

#[test]
fn unobserved_outer_resizes_are_unknown_and_late_grids_evolve_the_answer() {
    let temporary = TempDir::new().expect("temporary directory");
    let (session, outer, child) = start_recorder(&temporary);
    outer.behave(Outer::ConfigureOnly);
    let unchanged = Uuid::now_v7();
    assert_eq!(
        failure(&resize(&session, unchanged, 100, 30)),
        ("resize_outcome_unknown", MutationEffect::Unknown)
    );
    assert!(resize_events(&session, unchanged).is_empty());

    outer.behave(Outer::Silent);
    let silent = Uuid::now_v7();
    assert_eq!(
        failure(&resize(&session, silent, 100, 30)),
        ("resize_outcome_unknown", MutationEffect::Unknown)
    );

    outer.behave(Outer::ApplyLater(110, 32, Duration::from_secs(7)));
    let late = Uuid::now_v7();
    let pending = thread::scope(|scope| {
        let first = scope.spawn(|| return resize(&session, late, 110, 32));
        thread::sleep(Duration::from_millis(500));

        let retry = resize(&session, late, 110, 32);
        let busy = resize(&session, Uuid::now_v7(), 90, 20);

        return (first.join().expect("first request"), retry, busy);
    });
    assert_eq!(
        failure(&pending.0),
        ("resize_outcome_unknown", MutationEffect::Unknown)
    );
    assert_eq!(
        failure(&pending.1),
        ("resize_pending", MutationEffect::Unknown)
    );
    assert_eq!(
        failure(&pending.2),
        ("resize_busy", MutationEffect::NotApplied)
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    while resize_events(&session, late).is_empty() {
        assert!(Instant::now() < deadline, "late grid was never recorded");

        thread::sleep(Duration::from_millis(50));
    }

    let evolved = resize(&session, late, 110, 32);
    assert!(
        matches!(evolved.result, ControlResult::Ok { .. }),
        "{evolved:?}"
    );
    assert_eq!(resize_events(&session, late).len(), 2);
    assert_eq!(outer.requests.lock().unwrap().len(), 3);

    stop(&session, child);
}
