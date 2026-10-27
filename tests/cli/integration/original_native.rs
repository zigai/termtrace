//! Original execution capture against the real native toolchain.

use crate::support::*;
use std::sync::atomic::AtomicBool;
use termtrace_engine::{
    analysis::diagnostics::native_correlations_for_state, observation::replay_endpoint_for_prefix,
};
use termtrace_evidence::{
    artifact::{verify_artifact_members, verify_session_artifacts},
    session::validate_completed_session,
};
use termtrace_media::native_capture::{
    validate_replay_screenshot, validated_visual_manifests_for_state,
};
use termtrace_model::artifact::visual::VisualManifest;
use termtrace_model::terminal::replay::ReplayEndpoint;

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick and FFprobe"]
fn native_live_prefix_excludes_hidden_bytes_and_remains_verifiable_after_completion() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("source");
    let release = temporary.path().join("release");
    let mut recorder = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["record", "--no-input", "--max-duration", "30s", "--out"]).arg(&session)
        .args(["--", "/bin/sh", "-c", "printf 'OLD\\033[?2026hHIDDEN'; while [ ! -f \"$1\" ]; do sleep 0.05; done; printf '\\033[?2026lNEW'", "source"])
        .arg(&release).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("recorder");

    wait_for_control(&session);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let status = termtrace_sdk::request(&session, ControlCommand::Status).expect("status");
        if let ControlPayload::Status { boundaries, .. } = status
            && boundaries.durable.output_bytes >= 17
        {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "initial output did not become durable"
        );

        thread::sleep(Duration::from_millis(10));
    }

    let destination = session.join("live");
    let capture = Command::cargo_bin("termtrace")
        .expect("binary")
        .arg("screenshot")
        .arg(&session)
        .args(["--renderer", "native", "--out"])
        .arg(&destination)
        .timeout(Duration::from_secs(25))
        .output()
        .expect("capture process");

    fs::write(&release, b"release").expect("release recorder");

    assert!(recorder.wait().expect("recording status").success());
    assert!(
        capture.status.success(),
        "{}",
        String::from_utf8_lossy(&capture.stderr)
    );

    let endpoint: ReplayEndpoint = serde_json::from_slice(
        &fs::read(destination.join("replay-endpoint.json")).expect("endpoint"),
    )
    .expect("endpoint JSON");
    assert!(!endpoint.source_complete);
    assert_eq!(endpoint.event_byte_offset, Some(2));

    let validated =
        replay_endpoint_for_prefix(&session, &endpoint.boundary, &AtomicBool::new(false))
            .expect("prefix still validates after completion");
    assert_eq!(endpoint, validated);
    let completion = validate_completed_session(&session).expect("completed source");
    assert_eq!(
        verify_session_artifacts(
            &session,
            &completion.session_manifest_hash,
            Some(&completion.final_event_hash)
        )
        .expect("live artifact catalog remains valid"),
        1
    );
    let native = validated_visual_manifests_for_state(&session, 3, &AtomicBool::new(false))
        .expect("live screenshot discovered for completed state");
    assert_eq!(
        native_correlations_for_state(endpoint.session_id, 3, &native).len(),
        1
    );
    assert_eq!(
        endpoint,
        validate_replay_screenshot(&session, &destination, &AtomicBool::new(false))
            .expect("source-bound native screenshot")
    );

    let manifest: VisualManifest =
        serde_json::from_slice(&fs::read(destination.join("manifest.json")).expect("manifest"))
            .expect("visual manifest");

    verify_artifact_members(&destination, &manifest.members)
        .expect("verified pixels and prefix commitment");

    assert_eq!(
        manifest.selected_state_hash.as_deref(),
        Some(endpoint.state_hash.as_str())
    );

    fs::write(destination.join("final.png"), b"tampered pixels").expect("tamper fixture");

    assert!(validate_replay_screenshot(&session, &destination, &AtomicBool::new(false)).is_err());
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick and FFprobe"]
fn native_replay_batch_orders_endpoints_and_reuses_one_terminal() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("source");
    let profile = temporary.path().join("ghostty.conf");
    fs::write(&profile, b"cursor-style-blink = false\n").expect("non-blinking cursor profile");

    Command::cargo_bin("termtrace")
        .expect("binary")
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .arg("--ghostty-config")
        .arg(&profile)
        .args(["--", "/bin/sh", "-c", "printf 'ABC'"])
        .timeout(Duration::from_secs(20))
        .assert()
        .success();

    let destination = session.join("batch");
    Command::cargo_bin("termtrace")
        .expect("binary")
        .arg("screenshot")
        .arg(&session)
        .args([
            "--renderer",
            "native",
            "--state",
            "3",
            "--state",
            "0",
            "--state",
            "3",
            "--out",
        ])
        .arg(&destination)
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    let mut terminal_pid = None;
    let mut images = Vec::new();

    for (index, expected_state) in [3_u64, 0, 3].into_iter().enumerate() {
        let artifact = destination.join(format!("capture-{index:04}"));
        let manifest: VisualManifest =
            serde_json::from_slice(&fs::read(artifact.join("manifest.json")).expect("manifest"))
                .expect("visual manifest");

        verify_artifact_members(&artifact, &manifest.members).expect("verified screenshot payload");

        assert_eq!(manifest.selected_state_id, Some(expected_state));

        let endpoint: ReplayEndpoint = serde_json::from_slice(
            &fs::read(artifact.join("replay-endpoint.json")).expect("endpoint"),
        )
        .expect("endpoint JSON");
        assert!(endpoint.source_complete);
        assert_eq!(
            endpoint,
            validate_replay_screenshot(&session, &artifact, &AtomicBool::new(false))
                .expect("source-bound native screenshot")
        );
        assert_eq!(endpoint.state_id, Some(expected_state));

        let resources: serde_json::Value = serde_json::from_slice(
            &fs::read(artifact.join("native-resources.json")).expect("resources"),
        )
        .expect("resources JSON");

        let pid = resources["terminal_pid"]
            .as_u64()
            .expect("owned terminal PID");

        if let Some(expected) = terminal_pid {
            assert_eq!(pid, expected);
        } else {
            terminal_pid = Some(pid);
        }

        images.push(fs::read(artifact.join("final.png")).expect("PNG"));
    }

    assert_eq!(images[0], images[2]);
    assert_ne!(images[0], images[1]);

    let completion = validate_completed_session(&session).expect("completed source");
    assert_eq!(
        verify_session_artifacts(
            &session,
            &completion.session_manifest_hash,
            Some(&completion.final_event_hash)
        )
        .expect("batch catalog"),
        1
    );
    let native = validated_visual_manifests_for_state(&session, 3, &AtomicBool::new(false))
        .expect("batch screenshots discovered");
    assert_eq!(
        native_correlations_for_state(completion.session_id, 3, &native).len(),
        2
    );
    assert!(!Path::new(&format!("/proc/{}", terminal_pid.expect("terminal PID"))).exists());
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, FFmpeg and FFprobe"]
fn original_native_acquires_before_execution_and_preserves_exit_status() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("original with spaces");
    let mut record = Command::cargo_bin("termtrace").expect("binary");
    record
        .args(["record", "--original-native", "--out"])
        .arg(&session)
        .args([
            "--font-size",
            "18",
            "--max-duration",
            "10s",
            "--",
            "/bin/sh",
            "-c",
            "printf '\\033[31mORIGINAL\\033[0m\\n'; sleep 0.3; printf 'DONE\\n'; exit 7",
        ])
        .timeout(Duration::from_secs(40))
        .assert()
        .code(7);

    let completion = validate_completed_session(&session).expect("valid completed recording");
    assert_eq!(completion.exit_code, Some(7));
    let log = EventLog::read(session.join("events.jsonl")).expect("canonical log");
    assert_eq!(
        canonical_output_bytes(&log),
        b"\x1b[31mORIGINAL\x1b[0m\r\nDONE\r\n"
    );

    let visual = session.join("visual-original");
    let manifest: VisualManifest =
        serde_json::from_slice(&fs::read(visual.join("manifest.json")).expect("manifest bytes"))
            .expect("visual manifest");

    verify_artifact_members(&visual, &manifest.members).expect("all native members verified");

    let resources: serde_json::Value = serde_json::from_slice(
        &fs::read(visual.join("native-resources.json")).expect("resource report bytes"),
    )
    .expect("resource report");
    assert!(resources["terminal_pid"].is_number());
    assert!(resources["resources"]
        .as_array()
        .expect("resource identities")
        .iter()
        .any(|resource| return resource["kind"] == "terminal_executable"
            && resource["blake3"]
                .as_str()
                .is_some_and(|hash| return hash.len() == 64)
            && resource["byte_length"]
                .as_u64()
                .is_some_and(|bytes| return bytes > 0)));
    assert_eq!(manifest.capture_kind, "original_execution");
    assert_eq!(
        manifest.source_event_hash,
        Some(completion.final_event_hash)
    );
    assert!(manifest.capture_started_unix_ns <= log.header.started_unix_ns);
    assert_eq!(manifest.replay_origin_unix_ns, 0);
    assert!(manifest.selected_state_id.is_none());
    assert!(!visual.join("video-map.jsonl").exists());

    let frames: Vec<serde_json::Value> =
        BufReader::new(File::open(visual.join("acquisition-map.jsonl")).expect("frame map"))
            .lines()
            .map(|line| {
                return serde_json::from_str(&line.expect("frame line")).expect("frame JSON");
            })
            .collect();

    let first = frames.first().expect("first acquired frame");
    assert_eq!(
        first["acquisition_unix_ns"].as_u64().map(u128::from),
        Some(manifest.capture_started_unix_ns)
    );
    assert!(first["elapsed_since_execution_start_ns"].is_null());
    assert!(frames
        .iter()
        .any(|frame| return frame["elapsed_since_execution_start_ns"].is_number()));

    let packet_count = manifest
        .video_probe
        .as_ref()
        .and_then(|probe| return probe.pointer("/streams/0/nb_read_packets"))
        .and_then(serde_json::Value::as_str)
        .and_then(|value| return value.parse::<usize>().ok())
        .expect("packet count");
    assert_eq!(frames.len(), packet_count);
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, FFmpeg and FFprobe"]
fn original_native_cancellation_stops_the_managed_command() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("cancelled");
    let marker = temporary.path().join("child-survived");
    let log_path = temporary.path().join("capture.log");
    let mut capture = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["record", "--original-native", "--out"])
        .arg(&session)
        .args([
            "--max-duration",
            "15s",
            "--",
            "/bin/sh",
            "-c",
            "printf 'READY\\n'; sleep 2; printf survived > \"$1\"",
            "child",
        ])
        .arg(&marker)
        .stdout(Stdio::null())
        .stderr(File::create(&log_path).expect("capture log"))
        .spawn()
        .expect("capture child");

    wait_for_control(&session);

    assert!(ProcessCommand::new("kill")
        .args(["-TERM", &capture.id().to_string()])
        .status()
        .expect("signal capture")
        .success());

    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if let Some(status) = capture.try_wait().expect("capture status") {
            assert!(!status.success());

            break;
        }

        if Instant::now() >= deadline {
            // Best-effort cleanup before failing the test with the cancellation assertion.
            drop(capture.kill());
            drop(capture.wait());
            panic!("capture did not stop on cancellation");
        }

        thread::sleep(Duration::from_millis(20));
    }

    thread::sleep(Duration::from_secs(2));

    assert!(
        !marker.exists(),
        "managed command survived capture cancellation"
    );

    let diagnostic = fs::read_to_string(log_path).expect("capture diagnostic");
    assert!(diagnostic.contains("capture cancelled"), "{diagnostic}");
    assert!(!session.join("visual-original").exists());
}

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when the capture status fixture is invalid"
)]
fn original_status(session: &Path) -> serde_json::Value {
    let path = PathBuf::from(format!("{}.original-capture.json", session.display()));

    return serde_json::from_slice(&fs::read(path).expect("status sidecar")).expect("status JSON");
}

#[allow(
    clippy::indexing_slicing,
    reason = "test helper should stop when the capture status fixture is invalid"
)]
fn await_original_state(session: &Path, states: &[&str]) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(60);

    loop {
        let status = original_status(session);
        if states.iter().any(|state| return status["state"] == *state) {
            return status;
        }

        assert!(
            Instant::now() < deadline,
            "capture stayed {}",
            status["state"]
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when the capture status fixture is invalid"
)]
fn termtrace(state: &Path) -> Command {
    let mut command = Command::cargo_bin("termtrace").expect("binary");
    command.env("TERMTRACE_STATE_DIR", state);

    return command;
}

fn start_detached(state: &Path, session: &Path, alias: &str, script: &str) {
    termtrace(state)
        .args(["record", "--original-native", "--original-grid", "100x30"])
        .args(["--original-canvas", "1600x900", "-d", alias, "--out"])
        .arg(session)
        .args(["--", "/bin/sh", "-c", script])
        .timeout(Duration::from_secs(60))
        .assert()
        .success();
}

const WINCH_SCRIPT: &str =
    "trap 'echo winch $(stty size)' WINCH; echo ready $(stty size); while :; do sleep 0.05; done";

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, FFmpeg and FFprobe"]
fn detached_original_resizes_its_window_inside_a_constant_canvas() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let session = temporary.path().join("resized");
    start_detached(&state, &session, "resized", WINCH_SCRIPT);

    let live = original_status(&session);
    assert_eq!(live["state"], "live");
    assert_eq!(live["grid"], serde_json::json!({"cols": 100, "rows": 30}));

    validate_json_value("original-capture-status.schema.json", &live);

    termtrace(&state)
        .args(["send", "resized", "--resize", "120x34", "--json"])
        .assert()
        .success();

    termtrace(&state)
        .args(["send", "resized", "--resize", "240x80", "--json"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("exceeds_native_canvas"))
        .stdout(predicate::str::contains("not_applied"));

    termtrace(&state)
        .args(["status", "resized", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""original_capture""#));

    termtrace(&state)
        .args(["stop", "resized", "--wait"])
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    let published = await_original_state(&session, &["published", "failed"]);
    assert_eq!(published["state"], "published", "{published}");

    validate_json_value("original-capture-status.schema.json", &published);

    let log = EventLog::read(session.join("events.jsonl")).expect("canonical log");
    let output = String::from_utf8(canonical_output_bytes(&log)).expect("text output");
    assert!(output.contains("ready 30 100"), "{output}");
    assert!(output.contains("winch 34 120"), "{output}");
    assert!(!output.contains("winch 80 240"), "{output}");

    let visual = session.join("visual-original");
    let resizes = visual.join("window-resizes.jsonl");
    validate_json_lines("window-resize-record.schema.json", &resizes);
    let records: Vec<serde_json::Value> = fs::read_to_string(&resizes)
        .expect("resize timeline")
        .lines()
        .map(|line| return serde_json::from_str(line).expect("resize record"))
        .collect();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["decision"], "configured");
    assert_eq!(
        records[0]["observed"],
        serde_json::json!({"cols": 120, "rows": 34})
    );
    assert_eq!(records[1]["decision"], "rejected");
    assert_eq!(records[1]["code"], "exceeds_native_canvas");

    let probe = ProcessCommand::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .args(["stream=width,height,r_frame_rate", "-of", "csv=p=0"])
        .arg(visual.join("session.mkv"))
        .output()
        .expect("ffprobe");
    assert_eq!(
        String::from_utf8_lossy(&probe.stdout).trim(),
        "1600,900,24/1"
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, FFmpeg and FFprobe"]
fn killed_original_supervisor_reads_as_abandoned_and_leaves_no_processes() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let session = temporary.path().join("abandoned");
    start_detached(&state, &session, "abandoned", WINCH_SCRIPT);
    let supervisor: i32 = original_status(&session)["supervisor_pid"]
        .as_str()
        .expect("supervisor pid")
        .parse()
        .expect("decimal pid");

    let children = ProcessCommand::new("pgrep")
        .args(["-P", &supervisor.to_string()])
        .output()
        .expect("pgrep");

    let children: Vec<String> = String::from_utf8_lossy(&children.stdout)
        .lines()
        .map(str::to_owned)
        .collect();
    assert!(
        children.len() >= 3,
        "display, encoder and terminal are owned: {children:?}"
    );
    assert!(ProcessCommand::new("kill")
        .args(["-KILL", &supervisor.to_string()])
        .status()
        .expect("kill supervisor")
        .success());

    let deadline = Instant::now() + Duration::from_secs(10);
    while children
        .iter()
        .any(|pid| return Path::new(&format!("/proc/{pid}")).exists())
    {
        assert!(
            Instant::now() < deadline,
            "owned processes outlived their supervisor"
        );

        thread::sleep(Duration::from_millis(50));
    }

    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        let status = termtrace(&state)
            .args(["status", "abandoned", "--json"])
            .output()
            .expect("status");

        let text = String::from_utf8_lossy(&status.stdout);
        if text.contains(r#""state": "abandoned""#) {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "status never derived abandoned: {text}"
        );

        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, FFmpeg and FFprobe"]
fn original_restart_waits_for_the_predecessor_capture_and_keeps_its_grid() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let session = temporary.path().join("first");
    start_detached(&state, &session, "restarted", WINCH_SCRIPT);

    let restarted = termtrace(&state)
        .args(["restart", "restarted", "--json"])
        .timeout(Duration::from_secs(120))
        .assert()
        .success();

    assert_eq!(original_status(&session)["state"], "published");

    let report: serde_json::Value =
        serde_json::from_slice(&restarted.get_output().stdout).expect("restart report");

    let successor = PathBuf::from(
        report["successor"]["canonical_session_display"]
            .as_str()
            .expect("successor path"),
    );

    let live = original_status(&successor);
    assert_eq!(live["state"], "live");
    assert_eq!(live["grid"], serde_json::json!({"cols": 100, "rows": 30}));
    assert_eq!(
        live["canvas"],
        serde_json::json!({"width": 1600, "height": 900})
    );

    let manifest = read_manifest(&successor).expect("successor manifest");
    assert!(manifest.lineage.is_some());

    termtrace(&state)
        .args(["restart", "restarted", "--cols", "90"])
        .timeout(Duration::from_secs(60))
        .assert()
        .failure()
        .stderr(predicate::str::contains("overrides are not supported"));

    termtrace(&state)
        .args(["stop", "restarted", "--wait"])
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    assert_eq!(
        await_original_state(&successor, &["published", "failed"])["state"],
        "published"
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, FFmpeg and FFprobe"]
fn original_native_warns_about_ignored_geometry_flags() {
    let temporary = TempDir::new().expect("temporary directory");

    Command::cargo_bin("termtrace")
        .expect("binary")
        .args(["record", "--original-native", "--cols", "100", "--out"])
        .arg(temporary.path().join("warned"))
        .args(["--", "/bin/true"])
        .timeout(Duration::from_secs(60))
        .assert()
        .success()
        .stderr(predicate::str::contains("--cols is ignored"))
        .stderr(predicate::str::contains("--original-grid"));
    let quiet = Command::cargo_bin("termtrace")
        .expect("binary")
        .args(["record", "--original-native", "--out"])
        .arg(temporary.path().join("quiet"))
        .args(["--", "/bin/true"])
        .timeout(Duration::from_secs(60))
        .assert()
        .success();
    assert!(!String::from_utf8_lossy(&quiet.get_output().stderr).contains("ignored"));
}
