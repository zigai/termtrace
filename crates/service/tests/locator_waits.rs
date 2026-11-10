//! Locator wait cancellation against a live recorder whose capture exchange is stalled.

use assert_cmd::cargo::cargo_bin;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use termtrace_model::{
    protocol::control::ControlCommand, terminal::screen::ScreenProjectionRequest,
};
use termtrace_service::{
    defaults::COMPLETION_GRACE,
    locator::{
        LocatorCondition, LocatorError, LocatorWaitOptions, WaitCancellation, CAPTURE_QUANTUM,
    },
    Session,
};

/// How long one unanswered quantum capture can hold a wait: the client's response bound, then
/// the grace in which a completed session would be answered from its evidence.
#[allow(clippy::expect_used, reason = "test setup helper")]
fn busy_capture_bound() -> Duration {
    let capture = ControlCommand::CaptureStable {
        stable_for_ns: 1,
        timeout_ns: u64::try_from(CAPTURE_QUANTUM.as_nanos()).expect("quantum fits u64"),
        projection: ScreenProjectionRequest::default(),
        reaction: None,
    };

    return capture.client_response_timeout() + COMPLETION_GRACE;
}

struct OwnedRecorder(Child);

impl OwnedRecorder {
    #[allow(clippy::expect_used, reason = "test setup helper")]
    fn pid(&self) -> Pid {
        return Pid::from_raw(i32::try_from(self.0.id()).expect("pid fits i32"));
    }
}

impl Drop for OwnedRecorder {
    fn drop(&mut self) {
        let pid = self.pid();
        let _resume = kill(pid, Signal::SIGCONT);
        let _stop = kill(pid, Signal::SIGTERM);
        let _reaped = self.0.wait();
    }
}

#[allow(clippy::expect_used, reason = "test setup helper")]
fn start_recorder(binary: &Path, out: &Path) -> (OwnedRecorder, Session) {
    let child = Command::new(binary)
        .args(["record", "--no-input", "--out"])
        .arg(out)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; printf 'locator ready\\n'; while :; do sleep 1; done",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn recorder");

    let recorder = OwnedRecorder(child);
    let started = Instant::now();
    let session = loop {
        if let Ok(session) = Session::attach(out, binary)
            && session.status().is_ok()
        {
            break session;
        }

        assert!(
            started.elapsed() < Duration::from_secs(20),
            "recorder never answered"
        );
        thread::sleep(Duration::from_millis(20));
    };

    return (recorder, session);
}

#[test]
#[allow(clippy::expect_used, reason = "test assertion boundary")]
fn cancelling_during_a_stalled_capture_reports_cancelled_with_the_last_frame() {
    let binary = cargo_bin("termtrace");
    let evidence = tempfile::tempdir().expect("evidence dir");
    let out = evidence.path().join("session");
    let (recorder, session) = start_recorder(&binary, &out);
    session
        .get_by_text("locator ready")
        .expect("literal")
        .wait(Duration::from_secs(10), Duration::from_millis(20))
        .expect("fixture ready");

    let token = WaitCancellation::new();
    let waiter_token = token.clone();
    let (outcome, cancelled_at, stopped_frame) = thread::scope(|scope| {
        let waiter = scope.spawn(|| {
            return session
                .get_by_text("NEVER-SHOWN")
                .expect("literal")
                .wait_for(LocatorWaitOptions {
                    condition: LocatorCondition::Exists,
                    timeout: Duration::from_secs(30),
                    interval: Duration::from_millis(20),
                    stable_for: Duration::ZERO,
                    cancel: Some(waiter_token),
                });
        });

        // Let a few polls succeed, then freeze the recorder so the next capture cannot be answered.
        // Slow machines need well over one 20 ms poll interval for the first poll to finish.
        thread::sleep(Duration::from_secs(1));

        kill(recorder.pid(), Signal::SIGSTOP).expect("stop recorder");

        // Polls run every 20 ms, so after this pause the waiter is blocked inside an exchange.
        thread::sleep(CAPTURE_QUANTUM + Duration::from_millis(100));

        let cancelled_at = Instant::now();
        token.cancel();

        let outcome = waiter.join().expect("waiter thread");
        kill(recorder.pid(), Signal::SIGCONT).expect("resume recorder");

        let resumed = session
            .frame(ScreenProjectionRequest::default())
            .expect("capture after resume");

        return (outcome, cancelled_at, resumed);
    });
    let settled_after = cancelled_at.elapsed();
    println!("stalled-capture cancellation settled after {settled_after:?}");

    match outcome {
        Err(LocatorError::Cancelled { frame, .. }) => {
            let frame = frame.expect("frames captured before the stall are kept");

            assert_eq!(frame.state_hash, stopped_frame.state_hash);
        }
        other => panic!(
            "expected a cancelled wait, got {:?}",
            other.map(drop).map_err(|error| return error.kind())
        ),
    }

    // An exchange already sent cannot be interrupted; the wait returns once the client abandons it.
    assert!(
        settled_after <= busy_capture_bound() + Duration::from_secs(1),
        "cancellation settled after {settled_after:?}"
    );
}
