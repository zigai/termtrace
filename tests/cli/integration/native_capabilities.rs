//! Native capability probes against real and deliberately broken native tools.

use crate::support::*;

const PROBED_TOOLS: [(&str, &str); 6] = [
    ("ghostty", "TERMTRACE_PROBE_REAL_GHOSTTY"),
    ("Xvfb", "TERMTRACE_PROBE_REAL_XVFB"),
    ("dbus-run-session", "TERMTRACE_PROBE_REAL_DBUS"),
    ("import", "TERMTRACE_PROBE_REAL_IMPORT"),
    ("ffmpeg", "TERMTRACE_PROBE_REAL_FFMPEG"),
    ("ffprobe", "TERMTRACE_PROBE_REAL_FFPROBE"),
];

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "native capability tests require every probed tool on PATH"
)]
fn host_tool(name: &str) -> PathBuf {
    let search_path = env::var_os("PATH").expect("host executable search path");

    return env::split_paths(&search_path)
        .map(|directory| return directory.join(name))
        .find(|path| return path.is_file())
        .unwrap_or_else(|| panic!("missing native test tool: {name}"));
}

fn tool_body(tool: &str, variable: &str, broken: &str) -> String {
    let display = if tool == "Xvfb" && broken != "Xvfb" {
        "printf \"%s\\n\" \"$1\" >> \"$TERMTRACE_PROBE_DISPLAYS\"\n"
    } else {
        ""
    };

    let hanging = format!("hanging-{tool}");
    let behavior = match (broken, tool) {
        _ if broken == tool => "exec /usr/bin/false\n".to_owned(),
        ("isolated-native", "ghostty") => "exec /usr/bin/false\n".to_owned(),
        ("isolated-native", "Xvfb" | "dbus-run-session" | "import") => {
            "exec /bin/sleep 60\n".to_owned()
        }
        ("unpainted-ghostty", "ghostty") => concat!(
            "for ready do :; done\n",
            "printf ready > \"$ready\"\n",
            "/bin/sleep 60 &\n",
            "printf \"%s\\n\" \"$!\" >> \"$TERMTRACE_PROBE_PIDS\"\n",
            "wait\n",
        )
        .to_owned(),
        ("noisy-ffmpeg", "ffmpeg") => {
            "while :; do printf \"unbounded probe diagnostics 0123456789\\n\" >&2; done\n"
                .to_owned()
        }
        ("noisy-ffprobe", "ffprobe") => {
            "while :; do printf \"unbounded probe output 0123456789\\n\"; done\n".to_owned()
        }
        _ if broken == hanging => concat!(
            "/bin/sleep 60 &\n",
            "printf \"%s\\n\" \"$!\" >> \"$TERMTRACE_PROBE_PIDS\"\n",
            "wait\n",
        )
        .to_owned(),
        _ => format!("exec \"${variable}\" \"$@\"\n"),
    };

    return format!(
        "#!/bin/sh\nprintf \"%s\\n\" \"$$\" >> \"$TERMTRACE_PROBE_PIDS\"\nprintf \"%s\\n\" \"{tool}\" >> \"$TERMTRACE_PROBE_CALLS\"\n{display}{behavior}"
    );
}

struct ProbeCase {
    directory: PathBuf,
}

#[allow(
    clippy::expect_used,
    reason = "test fixture setup should stop when the tool wrappers cannot be written"
)]
impl ProbeCase {
    fn new(directory: PathBuf, broken: &str) -> Self {
        let case = Self { directory };
        fs_err::create_dir_all(case.bin()).expect("wrapper directory");
        fs_err::create_dir_all(case.tmp()).expect("probe temporary directory");

        for path in [case.pids(), case.displays(), case.calls()] {
            fs_err::write(path, b"").expect("probe evidence file");
        }

        for (tool, variable) in PROBED_TOOLS {
            let wrapper = case.bin().join(tool);
            fs_err::write(&wrapper, tool_body(tool, variable, broken)).expect("tool wrapper");
            fs_err::set_permissions(&wrapper, Permissions::from_mode(0o755))
                .expect("executable tool wrapper");
        }

        return case;
    }

    fn bin(&self) -> PathBuf {
        return self.directory.join("bin");
    }

    fn tmp(&self) -> PathBuf {
        return self.directory.join("tmp");
    }

    fn pids(&self) -> PathBuf {
        return self.directory.join("pids");
    }

    fn displays(&self) -> PathBuf {
        return self.directory.join("displays");
    }

    fn calls(&self) -> PathBuf {
        return self.directory.join("calls");
    }

    fn lines(path: &Path) -> Vec<String> {
        return fs_err::read_to_string(path)
            .expect("probe evidence")
            .lines()
            .map(str::to_owned)
            .collect();
    }

    fn termtrace(&self) -> Command {
        let mut search_path = vec![self.bin()];
        search_path.extend(env::split_paths(
            &env::var_os("PATH").expect("host executable search path"),
        ));

        let mut command = Command::cargo_bin("termtrace").expect("binary should build");
        command
            .env("PATH", env::join_paths(search_path).expect("wrapper PATH"))
            .env("TERMTRACE_PROBE_PIDS", self.pids())
            .env("TERMTRACE_PROBE_DISPLAYS", self.displays())
            .env("TERMTRACE_PROBE_CALLS", self.calls());

        for (tool, variable) in PROBED_TOOLS {
            command.env(variable, host_tool(tool));
        }

        return command;
    }

    fn assert_not_invoked(&self, forbidden: &[&str]) {
        let invoked: Vec<String> = Self::lines(&self.calls())
            .into_iter()
            .filter(|call| return forbidden.contains(&call.as_str()))
            .collect();

        assert!(
            invoked.is_empty(),
            "unrelated capability tools were invoked: {invoked:?}"
        );
    }
}

#[allow(
    clippy::panic,
    reason = "capability assertions should stop when the doctor report lacks a check"
)]
fn check<'report>(report: &'report serde_json::Value, id: &str) -> &'report serde_json::Value {
    return report["checks"]
        .as_array()
        .and_then(|checks| return checks.iter().find(|check| return check["id"] == id))
        .unwrap_or_else(|| panic!("doctor report has no {id} check: {report}"));
}

fn process_is_live(pid: &str) -> bool {
    let Ok(stat) = fs_err::read_to_string(format!("/proc/{pid}/stat")) else {
        return false;
    };
    let state = stat
        .rsplit_once(") ")
        .and_then(|(_, rest)| return rest.chars().next());

    return state != Some('Z');
}

fn display_lock_owner(number: &str) -> Option<String> {
    return fs_err::read_to_string(format!("/tmp/.X{number}-lock"))
        .ok()
        .map(|pid| return pid.trim().to_owned());
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "capability probe assertions should stop at the first broken invariant"
)]
fn assert_doctor_case(broken: &str, requirement: &str, expected_exit: i32, code: &str) {
    let temporary = TempDir::new().expect("temporary directory");
    let case = ProbeCase::new(temporary.path().join("case"), broken);

    let output = case
        .termtrace()
        .env("TMPDIR", case.tmp())
        .args(["doctor", "--require", requirement, "--json"])
        .timeout(Duration::from_secs(45))
        .output()
        .expect("doctor process");

    let report: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("doctor JSON ({error}): {output:?}"));

    assert_eq!(
        output.status.code(),
        Some(expected_exit),
        "doctor exit status: {report}"
    );
    assert_eq!(
        check(&report, &format!("capabilities.{requirement}"))["code"],
        code
    );
    assert_eq!(check(&report, "capabilities.semantic")["status"], "ok");

    if broken == "import" {
        assert_eq!(check(&report, "capabilities.native-video")["status"], "ok");
    } else if broken == "ffmpeg" || broken == "hanging-ffmpeg" {
        assert_eq!(
            check(&report, "capabilities.native-screenshot")["status"],
            "ok"
        );
    }

    let displays = ProbeCase::lines(&case.displays());

    if broken != "Xvfb" {
        assert_eq!(
            displays.len(),
            1,
            "screenshot and video did not share one display probe"
        );
    }

    let pids = ProbeCase::lines(&case.pids());

    for pid in &pids {
        assert!(!process_is_live(pid), "leaked live child {pid}");
    }

    // A concurrent capture may hold the same display number by now. Its server names itself in
    // the lock and keeps the lock for as long as its socket exists.
    for display in &displays {
        let number = display.trim_start_matches(':');
        let before = display_lock_owner(number);
        let socket = PathBuf::from(format!("/tmp/.X11-unix/X{number}"));
        let socket_exists = socket.symlink_metadata().is_ok();
        let after = display_lock_owner(number);

        for owner in before.iter().chain(&after) {
            assert!(
                !pids.contains(owner),
                "leaked display lock /tmp/.X{number}-lock of child {owner}"
            );
        }

        assert!(
            !socket_exists || before.is_some() || after.is_some(),
            "leaked display resource {}",
            socket.display()
        );
    }

    let leaked: Vec<OsString> = fs_err::read_dir(case.tmp())
        .expect("probe temporary directory")
        .map(|entry| return entry.expect("probe temporary entry").file_name())
        .filter(|name| {
            return name
                .as_encoded_bytes()
                .starts_with(b"termtrace-capability-");
        })
        .collect();
    assert_eq!(leaked, Vec::<OsString>::new(), "leaked probe directory");
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn healthy_tools_report_native_screenshot_ready() {
    assert_doctor_case("", "native-screenshot", 0, "native_screenshot_ready");
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn healthy_tools_report_native_video_ready() {
    assert_doctor_case("", "native-video", 0, "native_video_ready");
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn failing_ghostty_blocks_native_screenshot() {
    assert_doctor_case(
        "ghostty",
        "native-screenshot",
        1,
        "native_screenshot_probe_failed",
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn failing_display_server_blocks_native_screenshot() {
    assert_doctor_case(
        "Xvfb",
        "native-screenshot",
        1,
        "native_screenshot_probe_failed",
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn failing_screenshot_tool_blocks_only_native_screenshot() {
    assert_doctor_case(
        "import",
        "native-screenshot",
        1,
        "native_screenshot_probe_failed",
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn failing_codec_blocks_only_native_video() {
    assert_doctor_case("ffmpeg", "native-video", 1, "native_video_probe_failed");
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn failing_media_probe_blocks_native_screenshot() {
    assert_doctor_case(
        "ffprobe",
        "native-screenshot",
        1,
        "native_screenshot_probe_failed",
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn hanging_ghostty_blocks_native_screenshot() {
    assert_doctor_case(
        "hanging-ghostty",
        "native-screenshot",
        1,
        "native_screenshot_probe_failed",
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn hanging_codec_blocks_only_native_video() {
    assert_doctor_case(
        "hanging-ffmpeg",
        "native-video",
        1,
        "native_video_probe_failed",
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn hanging_media_probe_blocks_native_screenshot() {
    assert_doctor_case(
        "hanging-ffprobe",
        "native-screenshot",
        1,
        "native_screenshot_probe_failed",
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn unpainted_ghostty_root_blocks_native_screenshot() {
    assert_doctor_case(
        "unpainted-ghostty",
        "native-screenshot",
        1,
        "native_screenshot_probe_failed",
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn noisy_codec_blocks_native_video() {
    assert_doctor_case(
        "noisy-ffmpeg",
        "native-video",
        1,
        "native_video_probe_failed",
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn noisy_media_probe_blocks_native_screenshot() {
    assert_doctor_case(
        "noisy-ffprobe",
        "native-screenshot",
        1,
        "native_screenshot_probe_failed",
    );
}

#[allow(
    clippy::expect_used,
    reason = "export assertions should stop at the first unreadable artifact"
)]
fn read_json(path: &Path) -> serde_json::Value {
    return serde_json::from_slice(&fs_err::read(path).expect("JSON artifact")).expect("JSON");
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn native_capture_and_exports_invoke_only_the_tools_they_need() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("isolation.termtrace");

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
        .args([
            "--",
            "/bin/sh",
            "-c",
            r#"printf "\033[2J\033[Hcapability isolation\n"; /bin/sleep 0.2; printf "complete\n""#,
        ])
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["analyze", "index"])
        .arg(&session)
        .assert()
        .success();

    let video = ProbeCase::new(temporary.path().join("video"), "hanging-import");
    video
        .termtrace()
        .args(["artifact", "capture"])
        .arg(&session)
        .args(["--fps", "24"])
        .timeout(Duration::from_secs(90))
        .assert()
        .success();
    video.assert_not_invoked(&["import"]);
    let manifest = read_json(&session.join("visual/manifest.json"));
    assert_eq!(manifest["capture_kind"], "video");
    assert_eq!(manifest["video_probe"]["streams"][0]["codec_name"], "h264");

    let screenshot = ProbeCase::new(temporary.path().join("screenshot"), "hanging-ffmpeg");
    let native_screenshot = temporary.path().join("native-screenshot");
    screenshot
        .termtrace()
        .arg("screenshot")
        .arg(&session)
        .args(["--renderer", "native", "--out"])
        .arg(&native_screenshot)
        .timeout(Duration::from_secs(90))
        .assert()
        .success();
    screenshot.assert_not_invoked(&["ffmpeg"]);
    let manifest = read_json(&native_screenshot.join("manifest.json"));
    assert_eq!(manifest["capture_kind"], "screenshot");
    assert_eq!(
        manifest["screenshot_probe"]["streams"][0]["codec_name"],
        "png"
    );

    let export = ProbeCase::new(temporary.path().join("export"), "isolated-native");
    export
        .termtrace()
        .arg("screenshot")
        .arg(&session)
        .args(["--renderer", "semantic", "--out"])
        .arg(temporary.path().join("semantic-screenshot"))
        .timeout(Duration::from_secs(30))
        .assert()
        .success();
    export.assert_not_invoked(&[
        "ghostty",
        "Xvfb",
        "dbus-run-session",
        "import",
        "ffmpeg",
        "ffprobe",
    ]);

    for renderer in ["semantic", "native"] {
        let destination = temporary.path().join(format!("{renderer}.mp4"));
        export
            .termtrace()
            .arg("video")
            .arg(&session)
            .args([
                "--renderer",
                renderer,
                "--fps",
                "24",
                "--hold-start",
                "0ms",
                "--hold-end",
                "0ms",
                "--out",
            ])
            .arg(&destination)
            .timeout(Duration::from_secs(90))
            .assert()
            .success();
        export.assert_not_invoked(&["ghostty", "Xvfb", "dbus-run-session", "import"]);

        let probe = StdCommand::new(host_tool("ffprobe"))
            .args(["-v", "error", "-show_streams", "-of", "json"])
            .arg(&destination)
            .output()
            .expect("ffprobe process");
        assert!(probe.status.success(), "{renderer} export does not decode");

        let streams: serde_json::Value =
            serde_json::from_slice(&probe.stdout).expect("ffprobe JSON");
        assert_eq!(streams["streams"][0]["codec_name"], "h264", "{renderer}");

        let duration: f64 = streams["streams"][0]["duration"]
            .as_str()
            .expect("stream duration")
            .parse()
            .expect("numeric stream duration");
        assert!(duration > 0.0, "{renderer} export has no duration");
    }
}
