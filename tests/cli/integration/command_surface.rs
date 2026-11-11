//! command surface CLI integration tests.

use crate::support::*;
use std::env;

#[test]
fn session_recovery_preserves_context_and_never_claims_completion() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("source.termtrace");
    let recovered = temporary.path().join("recovered.termtrace");

    Command::cargo_bin("termtrace")
        .expect("binary")
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/echo", "RECOVERED-CONTEXT"])
        .assert()
        .success();

    let log_path = session.join("events.jsonl");
    fs::OpenOptions::new()
        .append(true)
        .open(&log_path)
        .expect("source log")
        .write_all(b"{\"partial\"")
        .expect("damage tail");

    let original = fs_err::read(&log_path).expect("original bytes");

    Command::cargo_bin("termtrace")
        .expect("binary")
        .args(["integrity", "recover", "--session"])
        .arg(&session)
        .arg("--out")
        .arg(&recovered)
        .assert()
        .success();

    assert_eq!(
        fs_err::read(session.join("session.json")).expect("source manifest"),
        fs_err::read(recovered.join("session.json")).expect("recovered manifest")
    );
    assert_eq!(fs_err::read(&log_path).expect("source retained"), original);
    assert!(!recovered.join("completion.json").exists());

    let report: serde_json::Value =
        serde_json::from_slice(&fs_err::read(recovered.join("recovery.json")).expect("report"))
            .expect("report JSON");

    assert_eq!(report["complete_evidence"], false);
    assert!(report["recovery"]["discarded_tail"].is_object());

    Command::cargo_bin("termtrace")
        .expect("binary")
        .arg("screen")
        .arg(&recovered)
        .arg("--recover-prefix")
        .assert()
        .success()
        .stdout(predicate::str::contains("RECOVERED-CONTEXT"));

    Command::cargo_bin("termtrace")
        .expect("binary")
        .args(["integrity", "recover", "--session"])
        .arg(&session)
        .arg("--out")
        .arg(&recovered)
        .assert()
        .failure();

    fs_err::write(session.join("session.json"), b"{}").expect("damage manifest");

    let invalid = temporary.path().join("invalid.termtrace");
    Command::cargo_bin("termtrace")
        .expect("binary")
        .args(["integrity", "recover", "--session"])
        .arg(&session)
        .arg("--out")
        .arg(&invalid)
        .assert()
        .failure();
    assert!(!invalid.exists());
}

#[test]
fn top_level_video_rejects_non_presentation_frame_rates_before_session_access() {
    let temporary = TempDir::new().expect("temporary directory");
    let output = temporary.path().join("video.mp4");
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    command
        .args(["video", "missing-session", "--fps", "25", "-o"])
        .arg(output)
        .assert()
        .failure()
        .stderr(predicate::str::contains("video FPS must be 24, 30, or 60"));
}

#[test]
fn top_level_video_never_falls_back_from_native_to_semantic() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("session");
    let output = temporary.path().join("native.mp4");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf native"])
        .assert()
        .success();

    let mut video = Command::cargo_bin("termtrace").expect("binary should build");
    video
        .env("PATH", "")
        .arg("video")
        .arg(&session)
        .args(["-o"])
        .arg(&output)
        .args(["--renderer", "native"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "required capability `native-video` is blocked",
        ));
    assert!(!output.exists(), "native failure must not publish a video");
    assert!(
        !temporary.path().join("native.mp4.termtrace").exists(),
        "native failure must not publish a semantic sidecar"
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, FFmpeg and FFprobe"]
fn top_level_video_auto_captures_native_and_overwrites_output() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("session");
    let output = temporary.path().join("native.mp4");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf native"])
        .assert()
        .success();
    assert!(
        !session.join("visual/manifest.json").exists(),
        "session must not have visual capture initially"
    );

    let mut video = Command::cargo_bin("termtrace").expect("binary should build");
    video
        .arg("video")
        .arg(&session)
        .args(["-o"])
        .arg(&output)
        .args(["--renderer", "native"])
        .assert()
        .success();

    assert!(
        fs_err::read_dir(temporary.path())
            .expect("capture siblings")
            .any(|entry| {
                let entry = entry.expect("capture entry");
                return entry
                    .file_name()
                    .as_encoded_bytes()
                    .starts_with(b"native-replay-")
                    && entry.path().join("manifest.json").is_file();
            }),
        "termtrace video should retain fresh replay pixels without altering existing captures"
    );
    assert!(output.is_file(), "output video should be published");
    assert!(
        temporary.path().join("native.mp4.termtrace").is_dir(),
        "sidecar directory should be published"
    );

    // Rerun on existing output file to verify clean overwrite without EEXIST
    let mut rerun = Command::cargo_bin("termtrace").expect("binary should build");
    rerun
        .arg("video")
        .arg(&session)
        .args(["-o"])
        .arg(&output)
        .args(["--renderer", "native"])
        .assert()
        .success();
    assert!(
        output.is_file(),
        "output video should still exist after clean overwrite"
    );
}

#[test]
fn wait_rejects_idle_options_without_idle_and_conflicting_stability() {
    let mut idle_only = Command::cargo_bin("termtrace").expect("binary should build");
    idle_only
        .args(["wait", "session", "--screen", "READY", "--full-state"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--idle"));

    let mut conflicting_stability = Command::cargo_bin("termtrace").expect("binary should build");
    conflicting_stability
        .args(["wait", "session", "--idle", "--stable-for", "10ms"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn wait_requires_the_screen_flag_for_screen_predicates() {
    let mut positional = Command::cargo_bin("termtrace").expect("binary should build");
    positional
        .args(["wait", "session", "READY"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unexpected argument 'READY'"));
}

#[test]
fn doctor_require_passes_ready_capabilities_and_fails_blocked_ones() {
    let doctor_json = |output: &[u8]| -> serde_json::Value {
        return serde_json::from_slice(output).expect("doctor JSON");
    };

    let check = |report: &serde_json::Value, id: &str| -> serde_json::Value {
        return report["checks"]
            .as_array()
            .expect("checks array")
            .iter()
            .find(|check| return check["id"] == id)
            .unwrap_or_else(|| panic!("missing {id} check in {report:?}"))
            .clone();
    };

    let mut semantic = Command::cargo_bin("termtrace").expect("binary should build");
    let ready = semantic
        .args(["doctor", "--require", "semantic", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    assert_eq!(
        check(&doctor_json(&ready), "capabilities.semantic")["status"],
        "ok"
    );

    for required in ["native-video", "semantic-video"] {
        let mut blocked = Command::cargo_bin("termtrace").expect("binary should build");
        let output = blocked
            .env("PATH", "")
            .args(["doctor", "--require", required, "--json"])
            .assert()
            .code(1)
            .get_output()
            .stdout
            .clone();

        let report = doctor_json(&output);
        let capability = check(&report, &format!("capabilities.{required}"));

        assert_eq!(capability["status"], "skip");

        let needs = capability["needs"]
            .as_array()
            .expect("skipped check names its causes");
        assert!(!needs.is_empty(), "{capability:?}");

        for cause in needs {
            let cause = check(&report, cause.as_str().expect("check id"));

            assert_eq!(cause["status"], "fail", "{cause:?}");
            assert!(
                cause["fix"]["manual"].is_string(),
                "failed {required} dependency must carry a fix: {cause:?}"
            );
        }
    }
}

#[test]
fn doctor_tmux_integration_applicability_preserves_diagnostics() {
    let script = r#"#!/bin/sh
if [ "$1" = "-V" ]; then
    printf 'tmux 3.6\n'
    exit 0
fi
if [ "$1" = "-u" ]; then
    shift
fi
if [ "$1" = "-S" ]; then
    printf 'invalid option: pane-launch-command\n' >&2
    exit 1
fi
if [ "$1 $2" != 'show-options -gqv' ]; then
    printf 'unexpected fake tmux arguments\n' >&2
    exit 2
fi
case "$FAKE_TMUX_CASE" in
    disabled) exit 0 ;;
    no-server) printf 'no server running on fixture\n' >&2; exit 1 ;;
    read-error) printf 'fixture read error\n' >&2; exit 1 ;;
esac
if [ "$3" = '@termtrace-recording-version' ]; then
    if [ "$FAKE_TMUX_CASE" = mismatch ]; then printf '999\n'; else printf '1\n'; fi
elif [ "$FAKE_TMUX_CASE" = hook-error ]; then
    printf 'fixture hook error\n' >&2; exit 1
elif [ "$FAKE_TMUX_CASE" = broken ]; then
    printf '%s/missing-hook\n' "$HOME"
else
    printf '/bin/sh\n'
fi
"#;

    for (case, expected) in [
        ("disabled", None),
        ("missing", Some("skip")),
        ("no-server", Some("skip")),
        ("read-error", Some("warn")),
        ("mismatch", Some("warn")),
        ("hook-error", Some("warn")),
        ("broken", Some("fail")),
        ("enabled", Some("ok")),
    ] {
        let temporary = TempDir::new().expect("temporary directory");
        let home = temporary.path().join("home");
        let tools = temporary.path().join("bin");
        let state = temporary.path().join("state");

        fs::create_dir_all(&home).expect("private home");

        fs::create_dir_all(&tools).expect("private tools");

        fs::create_dir_all(&state).expect("private state");
        fs::set_permissions(&state, Permissions::from_mode(0o700)).expect("private permissions");

        let tmux = tools.join("tmux");
        fs::write(&tmux, script).expect("fake tmux script");
        fs::set_permissions(&tmux, Permissions::from_mode(0o755)).expect("fake executable");

        for fix in [false, true] {
            let command = || {
                let mut command = Command::cargo_bin("termtrace").expect("binary should build");
                command
                    .env_clear()
                    .env("HOME", &home)
                    .env(
                        "PATH",
                        if case == "missing" { Path::new("") } else { &tools },
                    )
                    .env("TMPDIR", temporary.path())
                    .env("TERMTRACE_STATE_DIR", &state)
                    .env("FAKE_TMUX_CASE", case)
                    .env("NO_COLOR", "1")
                    .env("TERM", "dumb")
                    .args(["doctor", "--require", "semantic"]);

                if fix {
                    command.arg("--fix");
                }

                return command;
            };
            let json = command()
                .arg("--json")
                .output()
                .expect("doctor JSON command");

            let report: serde_json::Value =
                serde_json::from_slice(&json.stdout).expect("doctor JSON");
            let checks = report["checks"].as_array().expect("checks array");
            let integration = checks
                .iter()
                .find(|entry| return entry["id"] == "state.tmux-integration");
            assert_eq!(
                integration.and_then(|entry| return entry["status"].as_str()),
                expected,
                "case {case}, fix {fix}: {report}"
            );

            if matches!(case, "read-error" | "mismatch" | "hook-error" | "broken") {
                assert!(integration.expect("diagnostic check")["fix"]["manual"].is_string());
            }

            if case == "missing" {
                assert_eq!(
                    integration.expect("missing dependency check")["needs"],
                    serde_json::json!(["dependencies.tmux"])
                );
            }

            assert!(checks
                .iter()
                .any(|entry| return entry["id"] == "state.root"));
            assert!(checks
                .iter()
                .any(|entry| return entry["id"] == "capabilities.native-video"));
            let summary = &report["summary"];
            for status in ["ok", "info", "warn", "fail", "skip"] {
                assert_eq!(
                    summary[status].as_u64().expect("status count"),
                    checks
                        .iter()
                        .filter(|entry| return entry["status"] == status)
                        .count() as u64,
                    "case {case}, fix {fix}, status {status}"
                );
            }

            let text = command().output().expect("doctor text command");
            let stdout = String::from_utf8(text.stdout).expect("doctor text");
            assert_eq!(
                stdout.contains("tmux-integration"),
                expected.is_some(),
                "{case}"
            );
            let counts = format!(
                "{} ok, {} info, {} warn, {} fail, {} skip",
                summary["ok"], summary["info"], summary["warn"], summary["fail"], summary["skip"]
            );
            assert!(stdout.contains(&counts), "case {case}: {stdout}");
            assert_eq!(
                json.status.code(),
                Some(if case == "broken" { 1 } else { 0 })
            );
            assert_eq!(text.status.code(), json.status.code());
        }
    }
}

#[test]
fn semantic_preview_accepts_a_relative_directory() {
    let temporary = TempDir::new().expect("temporary directory");
    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .current_dir(temporary.path())
        .args(["record", "--no-input", "--out", "session"])
        .args(["/bin/sh", "-c", "printf PREVIEW; sleep 0.3"])
        .assert()
        .success();
    let log = EventLog::read(temporary.path().join("session/events.jsonl")).expect("event log");
    let end = log.events.last().expect("final event").monotonic_ns;
    fs::write(
        temporary.path().join("edit.json"),
        serde_json::to_vec(&serde_json::json!({
            "version": 2,
            "clips": [{ "from": { "monotonicNs": 0 }, "to": { "monotonicNs": end } }]
        }))
        .expect("edit JSON"),
    )
    .expect("write edit plan");
    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .current_dir(temporary.path())
        .args(["video", "session", "--renderer", "semantic", "--fps", "24"])
        .args(["--edit", "edit.json", "--preview-out", "review/preview"])
        .assert()
        .success();

    assert!(temporary
        .path()
        .join("review/preview/contact-sheet.png")
        .is_file());
    assert!(temporary
        .path()
        .join("review/preview/preview.mp4")
        .is_file());
}

#[test]
fn semantic_video_is_explicit_and_native_remains_the_default() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("semantic-video-session");
    let edit = temporary.path().join("edit.json");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["/bin/sh", "-c", "printf SEMANTIC"])
        .assert()
        .success();

    let log = EventLog::read(session.join("events.jsonl")).expect("event log");
    let end = log.events.last().expect("final event").monotonic_ns;

    fs::write(
        &edit,
        serde_json::to_vec(&serde_json::json!({
            "version": 2,
            "clips": [{
                "from": { "monotonicNs": 0 },
                "to": { "monotonicNs": end },
                "caption": null
            }],
            "change_visualization": null
        }))
        .expect("edit JSON"),
    )
    .expect("write edit plan");

    let mut semantic = Command::cargo_bin("termtrace").expect("binary should build");
    let plan_output = semantic
        .env("PATH", "")
        .args(["video"])
        .arg(&session)
        .args(["--renderer", "semantic", "--fps", "60", "--edit"])
        .arg(&edit)
        .arg("--dry-run")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let plan: serde_json::Value =
        serde_json::from_slice(&plan_output).expect("semantic video plan JSON");

    assert_eq!(plan["source"]["kind"], "semantic");
    assert_eq!(plan["source"]["plan"]["fps"], 60);

    let mut supersampled = Command::cargo_bin("termtrace").expect("binary should build");
    let supersampled_output = supersampled
        .env("PATH", "")
        .args(["video"])
        .arg(&session)
        .args([
            "--renderer",
            "semantic",
            "--semantic-raster-profile",
            "supersampled-2x",
            "--edit",
        ])
        .arg(&edit)
        .arg("--dry-run")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let supersampled_plan: serde_json::Value =
        serde_json::from_slice(&supersampled_output).expect("supersampled video plan JSON");

    assert_eq!(
        supersampled_plan["source"]["plan"]["raster_profile"],
        "supersampled-2x"
    );

    let mut invalid_native_profile = Command::cargo_bin("termtrace").expect("binary should build");
    invalid_native_profile
        .args(["video"])
        .arg(&session)
        .args([
            "--renderer",
            "native",
            "--semantic-raster-profile",
            "direct",
            "--edit",
        ])
        .arg(&edit)
        .arg("--dry-run")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--semantic-raster-profile configures semantic video only",
        ));

    let mut native_default = Command::cargo_bin("termtrace").expect("binary should build");
    native_default
        .env("PATH", "")
        .args(["video"])
        .arg(&session)
        .arg("--edit")
        .arg(&edit)
        .arg("--dry-run")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "required capability `native-video` is blocked",
        ));

    let mut index = Command::cargo_bin("termtrace").expect("binary should build");
    index
        .args(["analyze", "index"])
        .arg(&session)
        .assert()
        .success();

    let screenshot = temporary.path().join("native-screenshot");
    let mut native_screenshot = Command::cargo_bin("termtrace").expect("binary should build");
    native_screenshot
        .env("PATH", "")
        .args(["screenshot"])
        .arg(&session)
        .args(["--renderer", "native", "--out"])
        .arg(&screenshot)
        .assert()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "required capability `native-screenshot` is blocked",
        ))
        .stderr(predicate::str::contains("native_host_tools_missing"))
        .stderr(predicate::str::contains(
            "termtrace doctor --require native-screenshot",
        ));
    assert!(
        !screenshot.exists(),
        "blocked screenshot created its destination"
    );

    assert_no_entry_with_prefix(temporary.path(), ".native-screenshot");

    let mut native_capture = Command::cargo_bin("termtrace").expect("binary should build");
    native_capture
        .env("PATH", "")
        .args(["artifact", "capture"])
        .arg(&session)
        .args(["--width", "640", "--height", "480"])
        .assert()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "required capability `native-video` is blocked",
        ))
        .stderr(predicate::str::contains("native_video_dependency_missing"))
        .stderr(predicate::str::contains(
            "termtrace doctor --require native-video",
        ));
    assert!(
        !session.join("visual").exists(),
        "blocked capture published visual evidence"
    );

    assert_no_entry_with_prefix(&session, ".visual");

    let preview = temporary.path().join("semantic-preview");
    let mut semantic_preview = Command::cargo_bin("termtrace").expect("binary should build");
    semantic_preview
        .env("PATH", "")
        .args(["video"])
        .arg(&session)
        .args(["--renderer", "semantic", "--fps", "24", "--edit"])
        .arg(&edit)
        .arg("--preview-out")
        .arg(&preview)
        .assert()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "required capability `semantic-video` is blocked",
        ))
        .stderr(predicate::str::contains(
            "semantic_video_dependency_missing",
        ))
        .stderr(predicate::str::contains(
            "termtrace doctor --require semantic-video",
        ));
    assert!(!preview.exists(), "blocked preview created its destination");

    assert_no_entry_with_prefix(temporary.path(), ".preview-");

    let export = temporary.path().join("semantic-export.mp4");
    let mut semantic_export = Command::cargo_bin("termtrace").expect("binary should build");
    semantic_export
        .env("PATH", "")
        .args(["video"])
        .arg(&session)
        .args(["--renderer", "semantic", "--fps", "24", "--edit"])
        .arg(&edit)
        .arg("--out")
        .arg(&export)
        .assert()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "required capability `semantic-video` is blocked",
        ))
        .stderr(predicate::str::contains(
            "semantic_video_dependency_missing",
        ))
        .stderr(predicate::str::contains(
            "termtrace doctor --require semantic-video",
        ));
    assert!(!export.exists(), "blocked export created its destination");

    assert_no_entry_with_prefix(temporary.path(), ".presentation-");
}

#[test]
#[ignore = "requires native Ghostty capture tools and FFmpeg with libx264"]
fn retained_native_video_exports_without_native_capture_tools() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("retained-native-session");

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args([
            "record",
            "--no-input",
            "--geometry",
            "fit",
            "--viewport",
            "640x400",
            "--out",
        ])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf RETAINED_NATIVE; sleep 0.25"])
        .assert()
        .success();

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["analyze", "index"])
        .arg(&session)
        .assert()
        .success();

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["artifact", "capture"])
        .arg(&session)
        .args(["--fps", "24"])
        .assert()
        .success();

    let retained_manifest = fs::read(session.join("visual/manifest.json"))
        .expect("retained verified native capture manifest");

    let presentation_tools = temporary.path().join("presentation-tools");
    fs::create_dir_all(&presentation_tools).expect("isolated presentation tool directory");

    let search_path = env::var_os("PATH").expect("host executable search path");

    for name in ["ffmpeg", "ffprobe"] {
        let executable = env::split_paths(&search_path)
            .map(|directory| return directory.join(name))
            .find(|path| return path.is_file())
            .expect("required presentation executable");

        symlink(
            fs::canonicalize(executable).expect("absolute presentation executable"),
            presentation_tools.join(name),
        )
        .expect("expose only presentation tools");
    }

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .env("PATH", &presentation_tools)
        .args(["doctor", "--require", "native-video"])
        .assert()
        .code(1);

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .env("PATH", &presentation_tools)
        .args(["doctor", "--require", "presentation"])
        .assert()
        .success();

    let destination = temporary.path().join("retained.mp4");
    let output = Command::cargo_bin("termtrace")
        .expect("binary should build")
        .env("PATH", &presentation_tools)
        .args(["video"])
        .arg(&session)
        .args(["--renderer", "native", "--fps", "24", "--out"])
        .arg(&destination)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: serde_json::Value = serde_json::from_slice(&output).expect("video result JSON");

    assert_eq!(report["renderer"], "native");
    assert!(
        destination.is_file(),
        "retained native export must publish its destination"
    );

    let manifest = PathBuf::from(
        report["manifest"]
            .as_str()
            .expect("published manifest path"),
    );
    assert!(
        manifest.is_file(),
        "retained native export must publish its manifest"
    );
    assert_eq!(
        fs::read(session.join("visual/manifest.json")).expect("retained capture manifest"),
        retained_manifest,
        "export must reuse native evidence without recapture"
    );
}

#[test]
fn publishing_and_generation_commands_emit_compact_result_envelopes() {
    let temporary = TempDir::new().expect("temporary directory");
    fs_err::set_permissions(temporary.path(), Permissions::from_mode(0o700))
        .expect("owner-only temporary directory");
    let session = temporary.path().join("result-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf RESULT"])
        .assert()
        .success();

    let cast = temporary.path().join("result.cast");
    let mut asciicast = Command::cargo_bin("termtrace").expect("binary should build");
    let output = asciicast
        .args(["artifact", "asciicast"])
        .arg(&session)
        .arg("--out")
        .arg(&cast)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let result = compact_json_line(&output);

    assert_eq!(result["kind"], "asciicast");
    assert!(result["output"].is_string());
    assert_eq!(result["output"], cast.display().to_string());
    assert!(cast.is_file());

    let damaged = temporary.path().join("damaged-events.jsonl");
    fs_err::copy(session.join("events.jsonl"), &damaged).expect("copy damaged fixture");
    fs::OpenOptions::new()
        .append(true)
        .open(&damaged)
        .expect("open damaged fixture")
        .write_all(b"{\"partial\"")
        .expect("append partial tail");

    let recovered = temporary.path().join("recovered-events.jsonl");
    let mut recover = Command::cargo_bin("termtrace").expect("binary should build");
    let output = recover
        .args(["integrity", "recover", "--event-log"])
        .arg(&damaged)
        .arg("--out")
        .arg(&recovered)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let result = compact_json_line(&output);
    assert_eq!(result["source"], damaged.display().to_string());
    assert_eq!(result["recovered"], recovered.display().to_string());
    assert!(result["discarded_tail"].is_object());
    assert!(recovered.is_file());

    let schema_directory = temporary.path().join("schemas");
    let mut first = Command::cargo_bin("termtrace").expect("binary should build");
    let first = first
        .args(["schema", "--out"])
        .arg(&schema_directory)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let first = compact_json_line(&first);

    assert_eq!(first["kind"], "schema_generation");
    assert!(first["directory"].is_string());
    assert_eq!(first["directory"], schema_directory.display().to_string());
    assert_eq!(first["replaced_existing_files"], false);

    let mut second = Command::cargo_bin("termtrace").expect("binary should build");
    let second = second
        .args(["schema", "--out"])
        .arg(&schema_directory)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let second = compact_json_line(&second);

    assert_eq!(second["kind"], "schema_generation");
    assert_eq!(second["file_count"], first["file_count"]);
    assert_eq!(second["replaced_existing_files"], true);
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helper should stop when its fixture directory cannot be inspected"
)]
fn assert_no_entry_with_prefix(directory: &Path, prefix: &str) {
    let entries = fs_err::read_dir(directory)
        .unwrap_or_else(|error| panic!("read {}: {error}", directory.display()));

    let matching = entries
        .map(|entry| return entry.expect("directory entry").file_name())
        .filter(|name| return name.as_bytes().starts_with(prefix.as_bytes()))
        .collect::<Vec<_>>();

    assert!(
        matching.is_empty(),
        "unexpected staging entries in {}: {matching:?}",
        directory.display()
    );
}

#[test]
fn readme_command_references_name_real_commands() {
    let help_subcommands = |path: &[&str]| -> BTreeSet<String> {
        let output = Command::cargo_bin("termtrace")
            .expect("binary should build")
            .args(path)
            .arg("--help")
            .output()
            .expect("help should run");

        let help = String::from_utf8(output.stdout).expect("help is UTF-8");
        return help
            .lines()
            .skip_while(|line| return *line != "Commands:")
            .skip(1)
            .take_while(|line| return !line.trim().is_empty())
            .filter_map(|line| return line.split_whitespace().next())
            .filter(|name| return *name != "help")
            .map(str::to_owned)
            .collect();
    };

    let starts_lowercase = |word: &str| {
        return word
            .chars()
            .next()
            .is_some_and(|first| return first.is_ascii_lowercase());
    };

    let readme = fs_err::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
        .expect("read README.md");

    let references: Vec<[String; 2]> = readme
        .split("termtrace ")
        .skip(1)
        .map(|rest| {
            let mut words = rest
                .split(|character: char| return character.is_whitespace() || character == '`')
                .map(str::to_owned);

            return [
                words.next().unwrap_or_default(),
                words.next().unwrap_or_default(),
            ];
        })
        .filter(|[command, _]| return starts_lowercase(command))
        .collect();

    let visible = help_subcommands(&[]);
    assert!(!visible.is_empty(), "help parsing found no commands");

    for [command, next] in &references {
        assert!(
            visible.contains(command),
            "README references `termtrace {command}`, but the CLI has only {visible:?}"
        );

        let subcommands = help_subcommands(&[command.as_str()]);

        if !subcommands.is_empty() && starts_lowercase(next) {
            assert!(
                subcommands.contains(next),
                "README references `termtrace {command} {next}`, but `{command}` has only {subcommands:?}"
            );
        }
    }
}

#[test]
fn top_level_video_derivatives_use_explicit_sources_and_never_replace() {
    use std::{
        fs::Permissions,
        os::unix::fs::{symlink, PermissionsExt},
    };
    use termtrace_model::artifact::sdk::{
        ArtifactProvenance, ArtifactSourceKind, MediaArtifactResult,
    };
    let temp = TempDir::new().unwrap();
    let session = temp.path().join("session");
    let sentinel = temp.path().join("runs");
    Command::cargo_bin("termtrace")
        .unwrap()
        .args([
            "record",
            "--no-input",
            "--cols",
            "12",
            "--rows",
            "3",
            "--out",
        ])
        .arg(&session)
        .args(["--", "/bin/sh", "-c"])
        .arg(format!(
            "echo once >> '{}'; printf READY; sleep 0.15",
            sentinel.display()
        ))
        .assert()
        .success();

    let canonical: Vec<_> = ["session.json", "events.jsonl", "completion.json"]
        .map(|p| return fs_err::read(session.join(p)).unwrap())
        .into();

    for extension in ["mp4", "gif", "webm"] {
        let unavailable = temp.path().join(format!("original.{extension}"));
        Command::cargo_bin("termtrace")
            .unwrap()
            .arg("video")
            .arg(&session)
            .args(["--renderer", "original-execution", "--out"])
            .arg(&unavailable)
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "original_execution_source_unavailable",
            ));
        assert!(!unavailable.exists());

        let blocked = temp.path().join(format!("blocked.{extension}"));
        Command::cargo_bin("termtrace")
            .unwrap()
            .env("PATH", "")
            .arg("video")
            .arg(&session)
            .args(["--renderer", "replay-native", "--out"])
            .arg(&blocked)
            .assert()
            .failure()
            .stderr(predicate::str::contains("native-video"));
        assert!(!blocked.exists());
    }

    for extension in ["gif", "webm"] {
        let out = temp.path().join(format!("semantic.{extension}"));
        let result = Command::cargo_bin("termtrace")
            .unwrap()
            .arg("video")
            .arg(&session)
            .args([
                "--renderer",
                "semantic",
                "--fps",
                "30",
                "--hold-start",
                "0ms",
                "--hold-end",
                "100ms",
                "--out",
            ])
            .arg(&out)
            .arg("--json")
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();

        let report: serde_json::Value = serde_json::from_slice(&result).unwrap();
        let artifact: MediaArtifactResult =
            serde_json::from_value(report["artifact"].clone()).unwrap();
        assert_eq!(
            artifact.provenance,
            ArtifactProvenance::PresentationOutput {
                source_kind: ArtifactSourceKind::SemanticRendering
            }
        );

        let bytes = fs_err::read(&out).unwrap();
        Command::cargo_bin("termtrace")
            .unwrap()
            .arg("video")
            .arg(&session)
            .args(["--renderer", "semantic", "--out"])
            .arg(&out)
            .assert()
            .failure()
            .stderr(predicate::str::contains("already exists"));

        assert_eq!(fs_err::read(out).unwrap(), bytes);
    }

    assert_eq!(fs_err::read_to_string(sentinel).unwrap(), "once\n");

    let after: Vec<_> = ["session.json", "events.jsonl", "completion.json"]
        .map(|p| return fs_err::read(session.join(p)).unwrap())
        .into();
    assert_eq!(canonical, after);

    let at_25 = temp.path().join("semantic-25.gif");
    Command::cargo_bin("termtrace")
        .unwrap()
        .arg("video")
        .arg(&session)
        .args([
            "--renderer",
            "semantic",
            "--fps",
            "25",
            "--hold-start",
            "0ms",
            "--hold-end",
            "0ms",
            "--out",
        ])
        .arg(&at_25)
        .assert()
        .success()
        .stdout(predicate::str::contains("\"fps\":25"));

    Command::cargo_bin("termtrace")
        .unwrap()
        .args(["video", "missing-session", "--fps", "101", "--out"])
        .arg(temp.path().join("bad-rate.gif"))
        .assert()
        .failure()
        .stderr(predicate::str::contains("GIF FPS must be in 1..=100"));
    // An actual failed bounded encode probe is unavailable, never a semantic/codec fallback.
    let tools = temp.path().join("limited-tools");
    fs_err::create_dir(&tools).unwrap();
    symlink(
        termtrace_process::find_executable("ffprobe").unwrap(),
        tools.join("ffprobe"),
    )
    .unwrap();
    let ffmpeg = tools.join("ffmpeg");
    let real_ffmpeg = termtrace_process::find_executable("ffmpeg").unwrap();
    fs_err::write(&ffmpeg,format!("#!/bin/sh\ncase \" $* \" in *libvpx-vp9*|*palettegen*) echo 'encoder or palette filter unavailable' >&2; exit 1;; esac\nexec '{}' \"$@\"\n",real_ffmpeg.display())).unwrap();
    {
        fs_err::set_permissions(&ffmpeg, Permissions::from_mode(0o700)).unwrap();
    }

    for extension in ["gif", "webm"] {
        let out = temp.path().join(format!("unsupported.{extension}"));
        Command::cargo_bin("termtrace")
            .unwrap()
            .env("PATH", &tools)
            .arg("video")
            .arg(&session)
            .args(["--renderer", "semantic", "--out"])
            .arg(&out)
            .assert()
            .failure()
            .stderr(predicate::str::contains("derivative_encoder_unavailable"));
        assert!(!out.exists());
    }

    for (extension, flag, value) in [
        ("webm", "--gif-loop", "once"),
        ("gif", "--preset", "lossless"),
    ] {
        let out = temp.path().join(format!("invalid.{extension}"));
        Command::cargo_bin("termtrace")
            .unwrap()
            .arg("video")
            .arg(&session)
            .args(["--renderer", "semantic", flag, value, "--out"])
            .arg(&out)
            .assert()
            .failure();
        assert!(!out.exists());
    }
}

#[allow(clippy::expect_used, reason = "test setup")]
fn record_marked_edit_session(session: &Path) {
    let spawned = Instant::now();
    let recorder = StdCommand::new(env!("CARGO_BIN_EXE_termtrace"))
        .args(["record", "--no-input", "--out"])
        .arg(session)
        .args([
            "--",
            "/bin/sh",
            "-c",
            "trap 'exit 0' TERM; printf 'alpha\\n'; sleep 0.3; printf 'beta\\n'; sleep 0.3; printf 'gamma\\n'; while :; do sleep 0.1; done",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn recorder");

    wait_for_control(session);

    let ready = Instant::now();

    // The edit below selects from the third marker (event 6) to 6.7 s. Timing from the spawn
    // keeps that marker well before 6.7 s of session time, and timing from control keeps the
    // session past it.
    thread::sleep(Duration::from_millis(4_000).saturating_sub(spawned.elapsed()));

    for label in ["first", "second", "third"] {
        Command::cargo_bin("termtrace")
            .expect("binary should build")
            .arg("send")
            .arg(session)
            .arg(format!("mark:{label}"))
            .assert()
            .success();

        thread::sleep(Duration::from_millis(400));
    }

    thread::sleep(Duration::from_millis(7_000).saturating_sub(ready.elapsed()));

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .arg("stop")
        .arg(session)
        .assert()
        .success();

    let recorded = recorder.wait_with_output().expect("recorder exit");
    assert!(recorded.status.success(), "{recorded:?}");
}

#[allow(clippy::expect_used, reason = "test setup")]
fn edit_plan_json(session: &Path, edit: &Path, renderer: &str) -> serde_json::Value {
    let output = Command::cargo_bin("termtrace")
        .expect("binary should build")
        .current_dir(env::temp_dir())
        .args(["video"])
        .arg(session)
        .args(["--renderer", renderer, "--fps", "30", "--edit"])
        .arg(edit)
        .arg("--dry-run")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    return serde_json::from_slice(&output).expect("plan JSON");
}

#[test]
fn edit_files_keep_their_identity_and_resolve_subtitles_beside_the_file() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("edit-session");
    let edits = temporary.path().join("edits");
    fs_err::create_dir(&edits).expect("edit directory");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/conformance/sdk/editing-captions.ass"),
        edits.join("captions.ass"),
    )
    .expect("subtitle resource");

    record_marked_edit_session(&session);

    // Marker, monotonicNs, eventSeq, and stateId selectors with a relative subtitle path.
    let edit = edits.join("edit.json");
    fs::write(
        &edit,
        serde_json::to_vec(&serde_json::json!({
            "version": 2,
            "clips": [
                { "from": { "stateId": 1 }, "to": { "stateId": 14 }, "caption": null },
                {
                    "from": { "marker": "first" },
                    "to": { "marker": "second" },
                    "speed": { "numerator": 2, "denominator": 3 },
                    "hold_ms": 250,
                    "caption": "Middle"
                },
                { "from": { "eventSeq": 6 }, "to": { "monotonicNs": 6_700_000_000_u64 }, "caption": null }
            ],
            "subtitles": [{ "ass": "captions.ass" }],
            "change_visualization": null
        }))
        .expect("edit JSON"),
    )
    .expect("write edit");

    let plan = edit_plan_json(&session, &edit, "semantic");

    // Recorded from this edit at plan version 2; the identity must not move.
    assert_eq!(
        plan["edit_plan_hash"],
        "a5d77c97241c51880b6658ebd9685ef6d3cdab3f3750e0b8fbaf9dc20ceb9063"
    );
    assert_eq!(
        plan["subtitles"][0]["source_path"],
        edits.join("captions.ass").to_str().expect("UTF-8 path")
    );
    assert_eq!(plan["clips"].as_array().map(Vec::len), Some(3));
}

#[test]
#[ignore = "requires native Ghostty capture tools and FFmpeg with libx264"]
fn native_frame_edits_start_at_the_frame_they_name() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("native-frame-session");
    record_marked_edit_session(&session);

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["analyze", "index"])
        .arg(&session)
        .assert()
        .success();

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["artifact", "capture"])
        .arg(&session)
        .args(["--fps", "30"])
        .assert()
        .success();

    let markers = temporary.path().join("markers.json");
    fs::write(
        &markers,
        br#"{"version":2,"clips":[{"from":{"marker":"first"},"to":{"marker":"third"},"caption":null}],"change_visualization":null}"#,
    )
    .expect("marker edit");

    let marker_plan = edit_plan_json(&session, &markers, "native");
    let first = marker_plan["markers"]
        .as_array()
        .and_then(|markers| {
            return markers
                .iter()
                .find(|marker| return marker["label"] == "first");
        })
        .and_then(|marker| return marker["nominal_frame"].as_u64())
        .expect("nominal frame of the first marker");

    // Offsets with n % 3 == 1 rounded down to the previous frame before the fix.
    let frames = [first + 1, first + 3, first + 9];
    let edit = temporary.path().join("frames.json");
    fs::write(
        &edit,
        serde_json::to_vec(&serde_json::json!({
            "version": 2,
            "clips": [
                { "from": { "nativeFrame": frames[0] }, "to": { "nativeFrame": frames[1] }, "caption": null },
                { "from": { "nativeFrame": frames[1] }, "to": { "nativeFrame": frames[2] }, "caption": null }
            ],
            "change_visualization": null
        }))
        .expect("edit JSON"),
    )
    .expect("frame edit");

    let plan = edit_plan_json(&session, &edit, "native");
    assert_eq!(plan["source"], marker_plan["source"]);

    let start_ns = |frame: u64| return (u128::from(frame) * 1_000_000_000).div_ceil(30);

    for (index, (from, to)) in [(frames[0], frames[1]), (frames[1], frames[2])]
        .into_iter()
        .enumerate()
    {
        let clip = &plan["clips"][index];

        assert_eq!(clip["source_start_frame"], from);
        assert_eq!(clip["source_end_frame"], to);

        let range = &clip["source_range"];
        let span = range["end_monotonic_ns"].as_u64().expect("end")
            - range["start_monotonic_ns"].as_u64().expect("start");

        assert_eq!(
            u128::from(span),
            start_ns(to) - start_ns(from),
            "clip {index}"
        );
    }
}
