//! command surface CLI integration tests.

use crate::support::*;

#[test]
fn root_help_groups_commands_by_workflow() {
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");

    command
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Commands:\n  Recording:"))
        .stdout(predicate::str::contains("\n  Live workflow:"))
        .stdout(predicate::str::contains("\n  Command groups:"))
        .stdout(predicate::str::contains("\n  Control:").not())
        .stdout(predicate::str::contains("\n  Analysis:").not())
        .stdout(predicate::str::contains("\n  Artifacts:").not())
        .stdout(predicate::str::contains("\n  Sessions:").not())
        .stdout(predicate::str::contains("\n  Integrity:").not())
        .stdout(predicate::str::contains("\n  Tooling:").not())
        .stdout(predicate::str::contains("\n  Meta:").not())
        .stdout(predicate::str::contains("bundle-verify").not())
        .stdout(predicate::str::contains("export-asciicast").not())
        .stdout(predicate::str::contains(
            "Run `termtrace <command> --help` for more details.",
        ))
        .stdout(predicate::str::contains(
            "tmux       Record managed tmux panes for their full lifetime",
        ));
}

#[test]
fn commands_subcommand_is_removed() {
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");

    command
        .args(["--response", "full", "commands"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "unrecognized subcommand 'commands'",
        ));
}

#[test]
fn tmux_help_exposes_setup_audit_and_pane_commands() {
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    let output = command
        .args(["tmux", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(output).expect("UTF-8 help");
    for subcommand in ["config", "enable", "disable", "status", "scope", "run"] {
        assert!(
            help.lines()
                .any(|line| return line.trim_start().starts_with(subcommand)),
            "tmux help omitted `{subcommand}`: {help}"
        );
    }
    assert!(
        !help.contains("guard"),
        "hidden guard leaked into help: {help}"
    );
    assert!(
        !help.contains("supervise"),
        "hidden supervisor leaked into help: {help}"
    );
}

#[test]
fn native_screenshot_help_uses_the_solver_without_a_noop_flag() {
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");

    command
        .args(["artifact", "screenshot", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("omit to solve the"))
        .stdout(predicate::str::contains("exact recorded grid"))
        .stdout(predicate::str::contains("--fit-viewport").not());
}

#[test]
fn top_level_debug_and_handoff_facades_are_directly_discoverable() {
    for (command_name, required_option) in [
        ("inspect", "--last-action"),
        ("diagnose", "--since-last-action"),
        ("screenshot", "--out <OUT>"),
        ("bundle", "--out <OUT>"),
        ("bug-report", "--out <OUT>"),
    ] {
        let mut command = Command::cargo_bin("termtrace").expect("binary should build");
        let output = command
            .args([command_name, "--help"])
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let help = String::from_utf8(output).expect("UTF-8 help");
        assert!(
            help.contains(required_option),
            "{command_name} help omitted `{required_option}`:\n{help}"
        );
        assert!(
            help.contains("<SESSION>"),
            "{command_name} help omitted the session operand:\n{help}"
        );
    }

    let mut bug_report = Command::cargo_bin("termtrace").expect("binary should build");
    bug_report
        .args(["bug-report", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--screenshot"));
}

#[test]
fn top_level_video_exposes_complete_manifested_workflow() {
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    let output = command
        .args(["video", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(output).expect("UTF-8 help");
    assert!(help.contains("Usage: termtrace video [OPTIONS] --out <OUT> <SESSION>"));
    for option in [
        "--preset",
        "--fps",
        "--renderer",
        "--resolution",
        "--scale",
        "--start-marker",
        "--end-marker",
        "--trim-idle",
        "--hold-start",
        "--hold-end",
        "--cursor-style",
    ] {
        assert!(
            help.contains(option),
            "video help omitted `{option}`:\n{help}"
        );
    }
    assert!(help.contains("docs"));
    assert!(help.contains("social"));
    assert!(help.contains("lossless"));
    assert!(help.contains("native never falls back"));
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
        .arg("video")
        .arg(&session)
        .args(["-o"])
        .arg(&output)
        .args(["--renderer", "native"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "explicitly choose `--renderer semantic`",
        ));
    assert!(!output.exists(), "native failure must not publish a video");
    assert!(
        !temporary.path().join("native.mp4.termtrace").exists(),
        "native failure must not publish a semantic sidecar"
    );
}
#[test]
fn complex_selector_help_stays_within_one_hundred_columns() {
    for (arguments, synopsis, headings) in [
        (
            &["analyze", "diff", "--help"][..],
            "termtrace analyze diff [OPTIONS] <SESSION> <FROM_SELECTION> <TO_SELECTION>",
            &["From selection:", "To selection:"][..],
        ),
        (
            &["analyze", "explain", "--help"][..],
            "termtrace analyze explain [OPTIONS] <SESSION> <TARGET_SELECTION>",
            &["Target selection:"][..],
        ),
        (&["playback", "--help"][..], "termtrace playback", &[][..]),
        (&["show", "--help"][..], "termtrace show", &[][..]),
        (&["wait", "--help"][..], "termtrace wait", &[][..]),
        (
            &["artifact", "save", "--help"][..],
            "termtrace artifact save",
            &[][..],
        ),
        (
            &["artifact", "screenshot", "--help"][..],
            "termtrace artifact screenshot",
            &[][..],
        ),
        (&["tmux", "--help"][..], "termtrace tmux", &[][..]),
        (
            &["tmux", "config", "--help"][..],
            "termtrace tmux config",
            &[][..],
        ),
        (
            &["tmux", "enable", "--help"][..],
            "termtrace tmux enable",
            &[][..],
        ),
        (
            &["tmux", "scope", "--help"][..],
            "termtrace tmux scope",
            &[][..],
        ),
        (
            &["tmux", "run", "--help"][..],
            "termtrace tmux run",
            &[][..],
        ),
    ] {
        let mut command = Command::cargo_bin("termtrace").expect("binary should build");
        let output = command
            .env("COLUMNS", "100")
            .args(arguments)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let help = String::from_utf8(output).expect("UTF-8 help");
        assert!(
            help.contains(synopsis),
            "missing concise synopsis in:\n{help}"
        );
        for heading in headings {
            assert!(help.contains(heading), "missing `{heading}` in:\n{help}");
        }
        for line in help.lines() {
            assert!(
                line.chars().count() <= 100,
                "{}-column help line exceeds 100 columns:\n{line}\n\n{help}",
                line.chars().count()
            );
        }
    }
}

#[test]
fn nested_command_paths_dispatch() {
    let mut version = Command::cargo_bin("termtrace").expect("binary should build");
    version
        .args(["tooling", "version"])
        .assert()
        .success()
        .stdout(predicate::str::contains("termtrace "));

    let mut keys = Command::cargo_bin("termtrace").expect("binary should build");
    keys.args(["control", "keys"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ctrl"));

    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("nested-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["--", "/bin/sh", "-c", "printf NESTED"])
        .assert()
        .success();

    let mut index = Command::cargo_bin("termtrace").expect("binary should build");
    index
        .args(["analyze", "index", "--session"])
        .arg(&session)
        .assert()
        .success();

    let mut verify = Command::cargo_bin("termtrace").expect("binary should build");
    verify
        .args(["integrity", "verify"])
        .arg(&session)
        .assert()
        .success();
}

#[test]
fn tooling_doctor_is_the_only_capability_inventory_command() {
    let mut help = Command::cargo_bin("termtrace").expect("binary should build");
    let output = help
        .args(["tooling", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(output).expect("UTF-8 help");
    assert!(
        help.lines()
            .any(|line| return line.trim_start().starts_with("doctor ")),
        "doctor command missing from tooling help: {help}"
    );
    assert!(
        !help
            .lines()
            .any(|line| return line.trim_start().starts_with("capabilities ")),
        "redundant capabilities command remains in tooling help: {help}"
    );

    let mut removed = Command::cargo_bin("termtrace").expect("binary should build");
    removed
        .args(["tooling", "capabilities"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unrecognized subcommand"));
}

#[test]
fn version_subcommand_reports_package_version() {
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");

    command
        .args(["tooling", "version"])
        .assert()
        .success()
        .stdout(predicate::str::contains(concat!(
            "termtrace ",
            env!("CARGO_PKG_VERSION")
        )))
        .stdout(predicate::str::contains(format!(
            "libghostty-vt {}",
            GHOSTTY_REVISION
        )))
        .stdout(predicate::str::contains(format!(
            "event-log v{EVENT_LOG_VERSION}"
        )));
}

#[test]
fn doctor_reports_capability_boundaries_and_enforces_requirements() {
    let mut version = Command::cargo_bin("termtrace").expect("binary should build");
    version
        .args(["tooling", "version", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""termtrace_version":"0.1.0""#))
        .stdout(predicate::str::contains("ghostty_vt_revision"))
        .stdout(predicate::str::contains("control_protocol_version"));

    let mut semantic = Command::cargo_bin("termtrace").expect("binary should build");
    semantic
        .args(["tooling", "doctor", "--require", "semantic", "--json"])
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""id":"semantic""#))
        .stdout(predicate::str::contains(r#""status":"ready""#));

    let mut blocked = Command::cargo_bin("termtrace").expect("binary should build");
    blocked
        .env("PATH", "")
        .args([
            "tooling",
            "doctor",
            "--require",
            "native-video",
            "--explain",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("native-video"))
        .stdout(predicate::str::contains("blocked"))
        .stdout(predicate::str::contains("fix:"));

    let mut semantic_video = Command::cargo_bin("termtrace").expect("binary should build");
    semantic_video
        .env("PATH", "")
        .args([
            "tooling",
            "doctor",
            "--require",
            "semantic-video",
            "--explain",
        ])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("semantic-video"))
        .stdout(predicate::str::contains("blocked"))
        .stdout(predicate::str::contains(
            "install FFmpeg/FFprobe with libx264rgb",
        ))
        .stdout(predicate::str::contains("fix:"))
        .stderr(predicate::str::contains("unexpected argument").not());
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
            "version": 1,
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
    semantic
        .env("PATH", "")
        .args(["artifact", "video", "plan"])
        .arg(&session)
        .args(["--source", "semantic", "--fps", "60", "--edit"])
        .arg(&edit)
        .assert()
        .success()
        .stdout(predicate::str::contains(r#""version":2"#))
        .stdout(predicate::str::contains(r#""kind":"semantic""#))
        .stdout(predicate::str::contains(r#""fps":60"#));

    let mut native_default = Command::cargo_bin("termtrace").expect("binary should build");
    native_default
        .args(["artifact", "video", "plan"])
        .arg(&session)
        .arg("--edit")
        .arg(&edit)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "native video source requires a verified capture",
        ))
        .stderr(predicate::str::contains(
            "checking the native-video tooling capability",
        ))
        .stderr(predicate::str::contains("--source semantic"));

    let mut invalid_native_fps = Command::cargo_bin("termtrace").expect("binary should build");
    invalid_native_fps
        .args(["artifact", "video", "plan"])
        .arg(&session)
        .args(["--source", "native", "--fps", "60", "--edit"])
        .arg(&edit)
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--fps configures semantic video only",
        ));

    let mut index = Command::cargo_bin("termtrace").expect("binary should build");
    index
        .args(["analyze", "index", "--session"])
        .arg(&session)
        .assert()
        .success();

    let mut native_show = Command::cargo_bin("termtrace").expect("binary should build");
    native_show
        .env("PATH", "")
        .env("TMPDIR", temporary.path())
        .arg("show")
        .arg(&session)
        .args(["--format", "png", "--renderer", "native"])
        .assert()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "required capability `native-screenshot` is blocked",
        ))
        .stderr(predicate::str::contains("native_host_tools_missing"))
        .stderr(predicate::str::contains("termtrace tooling doctor"))
        .stderr(predicate::str::contains("native-screenshot --explain"));
    assert_no_entry_with_prefix(temporary.path(), ".termtrace-native-show-");

    let screenshot = temporary.path().join("native-screenshot");
    let mut native_screenshot = Command::cargo_bin("termtrace").expect("binary should build");
    native_screenshot
        .env("PATH", "")
        .args(["artifact", "screenshot"])
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
        .stderr(predicate::str::contains("termtrace tooling doctor"))
        .stderr(predicate::str::contains("native-screenshot --explain"));
    assert!(
        !screenshot.exists(),
        "blocked screenshot created its destination"
    );
    assert_no_entry_with_prefix(temporary.path(), ".native-screenshot");

    let mut native_capture = Command::cargo_bin("termtrace").expect("binary should build");
    native_capture
        .env("PATH", "")
        .args(["artifact", "capture", "--session"])
        .arg(&session)
        .args(["--width", "640", "--height", "480"])
        .assert()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "required capability `native-video` is blocked",
        ))
        .stderr(predicate::str::contains("native_video_dependency_missing"))
        .stderr(predicate::str::contains("termtrace tooling doctor"))
        .stderr(predicate::str::contains("native-video --explain"));
    assert!(
        !session.join("visual").exists(),
        "blocked capture published visual evidence"
    );
    assert_no_entry_with_prefix(&session, ".visual");

    let preview = temporary.path().join("semantic-preview");
    let mut semantic_preview = Command::cargo_bin("termtrace").expect("binary should build");
    semantic_preview
        .env("PATH", "")
        .args(["artifact", "video", "preview"])
        .arg(&session)
        .args(["--source", "semantic", "--fps", "24", "--edit"])
        .arg(&edit)
        .arg("--out")
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
        .stderr(predicate::str::contains("termtrace tooling doctor"))
        .stderr(predicate::str::contains("semantic-video --explain"));
    assert!(!preview.exists(), "blocked preview created its destination");
    assert_no_entry_with_prefix(temporary.path(), ".preview-");

    let export = temporary.path().join("semantic-export");
    let mut semantic_export = Command::cargo_bin("termtrace").expect("binary should build");
    semantic_export
        .env("PATH", "")
        .args(["artifact", "video", "export"])
        .arg(&session)
        .args(["--source", "semantic", "--fps", "24", "--edit"])
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
        .stderr(predicate::str::contains("termtrace tooling doctor"))
        .stderr(predicate::str::contains("semantic-video --explain"));
    assert!(!export.exists(), "blocked export created its destination");
    assert_no_entry_with_prefix(temporary.path(), ".presentation-");
}

#[test]
fn completions_are_generated_from_the_current_command_graph() {
    for (shell, signature) in [
        ("bash", "complete -F _termtrace"),
        ("zsh", "#compdef termtrace"),
        ("fish", "complete -c termtrace"),
        ("elvish", "edit:completion:arg-completer[termtrace]"),
        ("powershell", "Register-ArgumentCompleter"),
    ] {
        let mut command = Command::cargo_bin("termtrace").expect("binary should build");
        command
            .args(["tooling", "completions", shell])
            .assert()
            .success()
            .stdout(predicate::str::contains(signature))
            .stdout(predicate::str::contains("control"))
            .stdout(predicate::str::contains("analyze"))
            .stdout(predicate::str::contains("artifact"))
            .stdout(predicate::str::contains("timeline"))
            .stdout(predicate::str::contains("explain"))
            .stdout(predicate::str::contains("events"))
            .stdout(predicate::str::contains("bundle-verify").not())
            .stdout(predicate::str::contains("export-asciicast").not())
            .stdout(
                predicate::str::contains("Report the current bounded capability inventory").not(),
            );
    }
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
        .args(["artifact", "asciicast", "--session"])
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
    assert_eq!(result["output_display"], cast.display().to_string());
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
        .args(["tooling", "schemas", "--out"])
        .arg(&schema_directory)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let first = compact_json_line(&first);
    assert_eq!(first["kind"], "schema_generation");
    assert!(first["directory"].is_string());
    assert_eq!(
        first["directory_display"],
        schema_directory.display().to_string()
    );
    assert!(
        first["file_count"]
            .as_u64()
            .is_some_and(|count| return count > 50)
    );
    assert_eq!(first["replaced_existing_files"], false);

    let mut second = Command::cargo_bin("termtrace").expect("binary should build");
    let second = second
        .args(["tooling", "schemas", "--out"])
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

    let mut schema_help = Command::cargo_bin("termtrace").expect("binary should build");
    schema_help
        .args(["tooling", "schemas", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "existing files with generated schema names are replaced",
        ));
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
