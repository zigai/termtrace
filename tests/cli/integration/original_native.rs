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
        .args(["send", "resized", "--resize", "120x34"])
        .args(["--until-screen", "winch 34 120", "--json"])
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

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when the tool directory cannot be built"
)]
fn path_without(directory: &Path, hidden: &str) -> OsString {
    fs::create_dir_all(directory).expect("tool directory");

    for source in env::split_paths(&env::var_os("PATH").expect("PATH")) {
        let Ok(entries) = fs::read_dir(&source) else {
            continue;
        };

        for entry in entries {
            let name = entry.expect("PATH entry").file_name();
            let link = directory.join(&name);

            if name != OsStr::new(hidden) && link.symlink_metadata().is_err() {
                symlink(source.join(&name), link).expect("link tool");
            }
        }
    }

    return directory.as_os_str().to_owned();
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, FFmpeg and FFprobe"]
fn original_native_names_a_missing_tool_before_creating_a_session() {
    let cases: [(&str, &str, Option<&str>, &str); 3] = [
        ("no-ffmpeg", "ffmpeg", None, "ffmpeg"),
        ("no-xvfb", "Xvfb", None, "Xvfb"),
        (
            "no-libx264rgb",
            "ffmpeg",
            Some("#!/bin/sh\nprintf ' V....D libx264 H.264\\n'\n"),
            "libx264rgb",
        ),
    ];

    for (case, hidden, replacement, named) in cases {
        let temporary = TempDir::new().expect("temporary directory");
        let tools = temporary.path().join("tools");
        let path = path_without(&tools, hidden);

        if let Some(script) = replacement {
            let stand_in = tools.join(hidden);
            fs::write(&stand_in, script).expect("stand-in tool");
            fs::set_permissions(&stand_in, Permissions::from_mode(0o755)).expect("executable");
        }

        let output = temporary.path().join("output");
        let scratch = temporary.path().join("scratch");
        fs::create_dir_all(&output).expect("output directory");

        fs::create_dir_all(&scratch).expect("scratch directory");
        let session = output.join("missing-tool.termtrace");

        Command::cargo_bin("termtrace")
            .expect("binary")
            .env("PATH", &path)
            .env("TMPDIR", &scratch)
            .args([
                "record",
                "--original-native",
                "--original-grid",
                "80x24",
                "--out",
            ])
            .arg(&session)
            .args(["--", "/bin/true"])
            .timeout(Duration::from_secs(60))
            .assert()
            .failure()
            .stderr(predicate::str::contains(named));
        assert!(!session.exists(), "{case}: a session was created");
        let created: Vec<OsString> = fs::read_dir(&output)
            .expect("output entries")
            .map(|entry| return entry.expect("entry").file_name())
            .filter(|name| return name != "missing-tool.termtrace.original-capture.json")
            .collect();
        assert_eq!(created, Vec::<OsString>::new(), "{case}");
    }
}

fn native_profile(fields: &str) -> String {
    return format!(r#"{{"version":1,"native":{fields},"semantic":{{"preset":"bundled-v1"}}}}"#);
}

#[test]
#[ignore = "requires Ghostty, Xvfb and dbus-run-session"]
fn visual_profile_native_failures_stop_before_launch_without_fallback() {
    let temporary = TempDir::new().expect("temporary directory");
    let marker = temporary.path().join("started");
    let invalid_config = temporary.path().join("invalid.ghostty");
    fs::write(&invalid_config, "font-size = 12\nnot-a-ghostty-key = 1\n").expect("config");

    for (profile, extra, code) in [
        (
            native_profile(r#"{"font_family":"Termtrace Missing Family 7c1d"}"#),
            Vec::new(),
            "font_unavailable",
        ),
        (
            native_profile(&format!(
                r#"{{"config":"{}"}}"#,
                invalid_config.to_str().expect("UTF-8 path")
            )),
            Vec::new(),
            "ghostty_config_invalid",
        ),
        (
            native_profile("{}"),
            vec!["--cols", "80", "--rows", "24", "--viewport", "400x300"],
            "viewport_mismatch",
        ),
    ] {
        let session = temporary.path().join("rejected");
        let mut command = Command::cargo_bin("termtrace").expect("binary");
        command
            .args(["record", "--no-input", "--visual-profile", &profile])
            .args(&extra)
            .arg("--out")
            .arg(&session)
            .args(["--", "/bin/sh", "-c", "printf started > \"$1\"", "probe"])
            .arg(&marker)
            .timeout(Duration::from_secs(60))
            .assert()
            .failure()
            .stderr(predicate::str::contains(code));

        assert!(!marker.exists(), "{code}: the application started");
        assert!(!session.exists(), "{code}: a session was created");
    }
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, FFmpeg, JetBrains Mono and DejaVu Sans Mono"]
fn visual_profile_native_replay_rejects_a_host_that_substitutes_the_font() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("session");
    Command::cargo_bin("termtrace")
        .expect("binary")
        .args(["record", "--no-input", "--visual-profile"])
        .arg(native_profile(
            r#"{"font_family":"JetBrains Mono","font_size_points":"12"}"#,
        ))
        .arg("--out")
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "echo profiled"])
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    Command::cargo_bin("termtrace")
        .expect("binary")
        .args(["analyze", "index"])
        .arg(&session)
        .assert()
        .success();

    // A host whose only font is DejaVu Sans Mono: Ghostty substitutes it for JetBrains Mono
    // and still reports the recorded configuration unchanged.
    let substitute = StdCommand::new("fc-match")
        .args(["--format=%{file}", "DejaVu Sans Mono"])
        .output()
        .expect("fc-match");

    let fonts = temporary.path().join("fonts");
    fs::create_dir_all(&fonts).expect("font directory");
    fs::copy(
        String::from_utf8(substitute.stdout).expect("UTF-8 font path"),
        fonts.join("substitute.ttf"),
    )
    .expect("substitute font");
    let fontconfig = temporary.path().join("fonts.conf");
    fs::write(
        &fontconfig,
        format!(
            "<?xml version=\"1.0\"?>\n<fontconfig><dir>{}</dir><cachedir>{}</cachedir></fontconfig>\n",
            fonts.display(),
            temporary.path().join("font-cache").display()
        ),
    )
    .expect("fontconfig");
    let screenshot = temporary.path().join("screenshot");
    let mut worker = Command::cargo_bin("termtrace").expect("binary");
    worker
        .arg("screenshot")
        .arg(&session)
        .args(["--renderer", "native", "--out"])
        .arg(&screenshot);

    let mut direct = Command::cargo_bin("termtrace").expect("binary");
    direct.args(["artifact", "capture"]).arg(&session);

    for command in [&mut worker, &mut direct] {
        command
            .env("FONTCONFIG_FILE", &fontconfig)
            .timeout(Duration::from_secs(120))
            .assert()
            .failure()
            .stderr(predicate::str::contains("font_unavailable"));
    }

    assert!(!screenshot.exists());
}

#[test]
#[ignore = "requires Ghostty"]
fn visual_profile_native_standard_base_ignores_ambient_user_config() {
    let temporary = TempDir::new().expect("temporary directory");
    let profile = native_profile(r#"{"font_size_points":"13.5"}"#);
    let mut replay_hashes = Vec::new();

    for (name, user_config) in [
        ("plain", ""),
        (
            "styled",
            "font-size = 31\nbackground = #123456\nfont-feature = +liga\n",
        ),
    ] {
        let home = temporary.path().join(format!("{name}-config"));
        fs::create_dir_all(home.join("ghostty")).expect("user config directory");
        fs::write(home.join("ghostty/config"), user_config).expect("user config");
        let session = temporary.path().join(name);
        let marker = temporary.path().join(format!("{name}-started"));

        Command::cargo_bin("termtrace")
            .expect("binary")
            .args([
                "record",
                "--no-input",
                "--visual-profile",
                &profile,
                "--out",
            ])
            .arg(&session)
            .args(["--", "/bin/sh", "-c", "printf started > \"$1\"", "probe"])
            .arg(&marker)
            .env("XDG_CONFIG_HOME", &home)
            .timeout(Duration::from_secs(60))
            .assert()
            .success();

        assert!(marker.exists());

        let manifest = read_manifest(&session).expect("profiled manifest");
        let provenance = manifest.ghostty_visual.expect("native provenance");
        let replay =
            fs::read_to_string(session.join(&provenance.replay_profile)).expect("replay profile");
        assert!(replay.contains("font-size = 13.5\n"));
        assert!(replay.contains("font-feature = -liga\n"));
        assert!(!replay.contains("31"), "{replay}");
        assert!(!replay.contains("#123456"), "{replay}");

        replay_hashes.push(provenance.replay_profile_hash);
    }

    assert_eq!(replay_hashes.first(), replay_hashes.get(1));
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn visual_profile_native_original_and_replay_share_profile_grid_and_dpi() {
    let temporary = TempDir::new().expect("temporary directory");
    let mut cell_heights = Vec::new();

    for dpi in [96, 144] {
        let session = temporary.path().join(format!("original-{dpi}"));
        let profile = native_profile(&format!(
            r##"{{"font_family":"JetBrains Mono","font_size_points":"12","padding_points":{{"x":4,"y":4}},"foreground":"#D8DEE9","background":"#20242c","dpi":{dpi}}}"##
        ));

        Command::cargo_bin("termtrace")
            .expect("binary")
            .args(["record", "--original-native", "--original-grid", "48x8"])
            .args(["--visual-profile", &profile, "--out"])
            .arg(&session)
            .args([
                "--max-duration",
                "10s",
                "--",
                "/bin/sh",
                "-c",
                "printf 'wide \\346\\274\\242 e\\314\\201 \\033[1mbold\\033[0m \\033[3mitalic\\033[0m\\n'; sleep 0.3",
            ])
            .timeout(Duration::from_secs(60))
            .assert()
            .success();

        let completion = validate_completed_session(&session).expect("valid completed recording");
        let manifest = read_manifest(&session).expect("session manifest");
        let recorded = manifest.visual_profile.clone().expect("recorded profile");
        recorded.validate().expect("recorded profile verifies");
        let native = recorded.profile.native.clone().expect("native section");
        assert_eq!(native.dpi, Some(dpi));
        assert_eq!(native.foreground.as_deref(), Some("#d8dee9"));
        assert_eq!((manifest.geometry.cols, manifest.geometry.rows), (48, 8));
        let provenance = manifest.ghostty_visual.clone().expect("native provenance");
        assert_eq!(provenance.overrides, native.overrides());
        let original: VisualManifest = serde_json::from_slice(
            &fs::read(session.join("visual-original/manifest.json")).expect("manifest bytes"),
        )
        .expect("original visual manifest");
        assert_eq!(
            original.effective_config_hash,
            provenance.effective_config_hash
        );
        assert_eq!(
            (original.geometry_report.cols, original.geometry_report.rows),
            (48, 8)
        );
        let replay = temporary.path().join(format!("replay-{dpi}"));
        Command::cargo_bin("termtrace")
            .expect("binary")
            .arg("screenshot")
            .arg(&session)
            .args(["--renderer", "native", "--out"])
            .arg(&replay)
            .timeout(Duration::from_secs(60))
            .assert()
            .success();

        let replayed: VisualManifest =
            serde_json::from_slice(&fs::read(replay.join("manifest.json")).expect("manifest"))
                .expect("replay visual manifest");
        assert_eq!(replayed.session_id, manifest.session_id);
        assert_eq!(
            replayed.effective_config_hash,
            provenance.effective_config_hash
        );
        assert_eq!(
            (replayed.geometry_report.cols, replayed.geometry_report.rows),
            (48, 8)
        );
        assert_eq!(
            replayed.source_event_hash,
            Some(completion.final_event_hash)
        );
        let mismatched = temporary.path().join(format!("mismatch-{dpi}"));
        Command::cargo_bin("termtrace")
            .expect("binary")
            .arg("screenshot")
            .arg(&session)
            .args(["--renderer", "native", "--viewport", "300x200", "--out"])
            .arg(&mismatched)
            .timeout(Duration::from_secs(60))
            .assert()
            .failure()
            .stderr(predicate::str::contains("viewport_mismatch"));
        assert!(!mismatched.exists());
        cell_heights.push(replayed.geometry_report.cell_height);
    }

    assert!(
        cell_heights.first() < cell_heights.get(1),
        "144 DPI must render taller cells than 96 DPI: {cell_heights:?}"
    );
}
