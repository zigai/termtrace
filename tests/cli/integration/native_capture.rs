//! Native recording, replay, video, and screenshot verification on a timed fixture.

use crate::support::*;

// The shell may print its first line up to two seconds after the session clock starts, so the
// range keeps that much slack before "Selected range" (12 s in) and the tail (15 s in).
const RANGE_FROM_NS: u64 = 11_000_000_000;
const RANGE_TO_NS: u64 = 14_500_000_000;
const WIDTH: u64 = 1280;
const HEIGHT: u64 = 720;

#[allow(
    clippy::expect_used,
    reason = "verification should stop at the first unreadable artifact"
)]
fn read_json(path: &Path) -> serde_json::Value {
    return serde_json::from_slice(&fs_err::read(path).expect("JSON artifact")).expect("JSON");
}

#[allow(
    clippy::expect_used,
    reason = "verification should stop at the first unreadable artifact"
)]
fn read_json_lines(path: &Path) -> Vec<serde_json::Value> {
    return fs_err::read_to_string(path)
        .expect("JSONL artifact")
        .lines()
        .map(|line| return serde_json::from_str(line).expect("JSONL record"))
        .collect();
}

fn is_hash(value: &serde_json::Value) -> bool {
    return value.as_str().is_some_and(|text| {
        return text.len() == 64
            && text
                .bytes()
                .all(|byte| return byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    });
}

fn numeric(value: &serde_json::Value) -> Option<f64> {
    return value
        .as_f64()
        .or_else(|| return value.as_str().and_then(|text| return text.parse().ok()));
}

fn assert_nonempty_files(directory: &Path, names: &[&str]) {
    for name in names {
        let path = directory.join(name);
        let metadata = fs_err::symlink_metadata(&path);

        assert!(
            metadata.is_ok_and(|metadata| return metadata.is_file() && metadata.len() > 0),
            "{} is not a non-empty regular file",
            path.display()
        );
    }
}

#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "verification should stop when FFprobe cannot run or reports no stream"
)]
fn assert_media_stream(path: &Path, codec: &str) {
    let probe = StdCommand::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,width,height",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .expect("ffprobe process");
    assert!(
        probe.status.success(),
        "FFprobe could not decode {}",
        path.display()
    );

    let report: serde_json::Value = serde_json::from_slice(&probe.stdout).expect("ffprobe JSON");
    let streams = report["streams"].as_array().expect("probed streams");
    assert_eq!(streams.len(), 1, "{}: {report}", path.display());
    assert_eq!(streams[0]["codec_name"], codec, "{}", path.display());
    assert_eq!(streams[0]["width"], WIDTH, "{}", path.display());
    assert_eq!(streams[0]["height"], HEIGHT, "{}", path.display());
}

#[allow(
    clippy::expect_used,
    reason = "verification should stop when FFmpeg cannot decode a screenshot"
)]
fn decoded_pixels(path: &Path) -> Vec<u8> {
    let decode = StdCommand::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .output()
        .expect("ffmpeg process");
    assert!(
        decode.status.success(),
        "screenshot pixel decoding failed: {}",
        path.display()
    );

    return decode.stdout;
}

#[allow(
    clippy::indexing_slicing,
    reason = "verification should stop when a manifest lacks a probed stream"
)]
fn assert_capture_manifest(manifest: &serde_json::Value, source_range: &serde_json::Value) {
    assert_eq!(manifest["version"], 6, "{manifest}");
    assert!(is_hash(&manifest["source_event_hash"]), "{manifest}");
    assert!(is_hash(&manifest["effective_config_hash"]), "{manifest}");
    assert_eq!(manifest["fps"], 24, "{manifest}");
    assert_eq!(&manifest["source_range"], source_range, "{manifest}");
    assert_eq!(manifest["width"], WIDTH, "{manifest}");
    assert_eq!(manifest["height"], HEIGHT, "{manifest}");
    assert_eq!(manifest["capture_kind"], "video", "{manifest}");

    let video = &manifest["video_probe"]["streams"][0];
    assert_eq!(video["codec_name"], "h264", "{manifest}");
    assert_eq!(video["width"], WIDTH, "{manifest}");
    assert_eq!(video["height"], HEIGHT, "{manifest}");
    assert_eq!(video["avg_frame_rate"], "24/1", "{manifest}");
    assert!(
        numeric(&video["nb_read_packets"]).is_some_and(|packets| return packets > 0.0),
        "{manifest}"
    );

    let screenshot = &manifest["screenshot_probe"]["streams"][0];
    assert_eq!(screenshot["codec_name"], "png", "{manifest}");
    assert_eq!(screenshot["width"], WIDTH, "{manifest}");
    assert_eq!(screenshot["height"], HEIGHT, "{manifest}");
}

#[allow(
    clippy::indexing_slicing,
    reason = "verification should stop when the video map is malformed"
)]
fn assert_video_map(entries: &[serde_json::Value], range: Option<(u64, u64)>) {
    assert!(!entries.is_empty(), "video map is empty");

    for entry in entries {
        assert!(entry["state_id"].as_u64().is_some(), "{entry}");
        assert!(is_hash(&entry["state_hash"]), "{entry}");
        assert!(entry["frame"].as_u64().is_some(), "{entry}");
        assert!(entry["presented"].is_boolean(), "{entry}");

        let monotonic = entry["monotonic_ns"].as_u64();
        assert!(monotonic.is_some(), "{entry}");

        if let (Some(time), Some((from, to))) = (monotonic, range) {
            assert!((from..to).contains(&time), "{entry} is outside the range");
        }
    }

    for pair in entries.windows(2) {
        assert!(
            pair[0]["monotonic_ns"].as_u64() <= pair[1]["monotonic_ns"].as_u64(),
            "video map times are unordered: {} then {}",
            pair[0],
            pair[1]
        );
        assert!(
            pair[0]["frame"].as_u64() <= pair[1]["frame"].as_u64(),
            "video map frames are unordered: {} then {}",
            pair[0],
            pair[1]
        );
    }

    assert!(
        entries
            .iter()
            .any(|entry| return entry["presented"] == true),
        "video map has no presented frame"
    );
}

#[allow(
    clippy::expect_used,
    reason = "verification should stop when the capture has no duration"
)]
fn video_duration(manifest: &serde_json::Value) -> f64 {
    let duration = manifest
        .pointer("/video_probe/format/duration")
        .and_then(numeric)
        .expect("numeric video duration");
    assert!(duration > 0.0, "video duration is not positive");

    return duration;
}

fn assert_session_manifest(session: &serde_json::Value) {
    let visual = &session["ghostty_visual"];
    let overrides = &visual["overrides"];

    assert_eq!(session["version"], 12, "{session}");
    assert!(!visual["source_config"].is_null(), "{session}");
    assert_eq!(overrides["font_size"], "18", "{session}");
    assert_eq!(overrides["padding_x"], 6, "{session}");
    assert_eq!(overrides["padding_y"], 4, "{session}");
    assert_eq!(overrides["foreground"], "#e6e6e6", "{session}");
    assert_eq!(overrides["background"], "#101010", "{session}");
    assert_eq!(
        visual["replay_profile"], "ghostty-profile.ghostty",
        "{session}"
    );
    assert!(is_hash(&visual["replay_profile_hash"]), "{session}");
    assert!(is_hash(&visual["effective_config_hash"]), "{session}");
}

#[allow(
    clippy::indexing_slicing,
    reason = "verification should stop when a manifest lacks a probed stream"
)]
fn assert_point_screenshot_manifest(manifest: &serde_json::Value) {
    assert_eq!(manifest["version"], 6, "{manifest}");
    assert!(manifest["source_range"].is_null(), "{manifest}");
    assert_eq!(manifest["width"], WIDTH, "{manifest}");
    assert_eq!(manifest["height"], HEIGHT, "{manifest}");
    assert!(is_hash(&manifest["effective_config_hash"]), "{manifest}");
    assert_eq!(manifest["capture_kind"], "screenshot", "{manifest}");
    assert_eq!(
        manifest["presentation_evidence"], "post_endpoint_stable_capture",
        "{manifest}"
    );
    assert_eq!(
        manifest["guarantee_level"], "native_capture_environment",
        "{manifest}"
    );
    assert!(
        manifest["selected_state_id"].as_u64().is_some(),
        "{manifest}"
    );
    assert!(is_hash(&manifest["selected_state_hash"]), "{manifest}");
    assert!(
        manifest["endpoint_event_seq"].as_u64().is_some(),
        "{manifest}"
    );
    assert!(
        manifest["endpoint_event_byte_offset"].as_u64().is_some(),
        "{manifest}"
    );

    let samples = manifest["frame_samples"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    assert!(samples.len() >= 3, "{manifest}");

    for sample in samples {
        assert!(sample["sample"].as_u64().is_some(), "{sample}");
        assert!(sample["captured_unix_ns"].as_u64().is_some(), "{sample}");
        assert!(is_hash(&sample["blake3"]), "{sample}");
        assert!(sample["same_as_previous"].is_boolean(), "{sample}");
    }

    assert_eq!(
        samples
            .last()
            .map(|sample| return &sample["same_as_previous"]),
        Some(&serde_json::Value::Bool(true)),
        "final frame sample is not stable"
    );

    let screenshot = &manifest["screenshot_probe"]["streams"][0];
    assert_eq!(screenshot["codec_name"], "png", "{manifest}");
    assert_eq!(screenshot["width"], WIDTH, "{manifest}");
    assert_eq!(screenshot["height"], HEIGHT, "{manifest}");
    assert!(manifest["video_probe"].is_null(), "{manifest}");
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, ImageMagick, FFmpeg and FFprobe"]
fn full_ranged_and_point_captures_of_a_timed_recording_are_consistent() {
    let temporary = TempDir::new().expect("temporary directory");
    let output = temporary.path().join("native-verification.termtrace");
    let profile = temporary.path().join("ghostty-profile");
    let range_suffix = format!("range-{RANGE_FROM_NS}-{RANGE_TO_NS}");
    let range_derived = output.join(format!("derived-{range_suffix}"));
    let range_visual = output.join(format!("visual-{range_suffix}"));
    let viewport = format!("{WIDTH}x{HEIGHT}");
    let from = format!("{RANGE_FROM_NS}ns");
    let to = format!("{RANGE_TO_NS}ns");
    let termtrace = || return Command::cargo_bin("termtrace").expect("binary should build");
    let doctor = termtrace()
        .args([
            "doctor",
            "--json",
            "--require",
            "native-screenshot,native-video",
        ])
        .output()
        .expect("doctor runs");

    let report: serde_json::Value = serde_json::from_slice(&doctor.stdout).expect("doctor JSON");
    let failing: Vec<&serde_json::Value> = report["checks"]
        .as_array()
        .expect("doctor checks")
        .iter()
        .filter(|check| return check["status"] == "fail")
        .collect();
    assert!(doctor.status.success(), "doctor failed: {failing:#?}");

    fs_err::write(
        &profile,
        "font-size = 11\nwindow-padding-x = 2\nbackground = #220000\n",
    )
    .expect("source Ghostty profile");

    termtrace()
        .args([
            "record",
            "--no-input",
            "--geometry",
            "fit",
            "--viewport",
            &viewport,
            "--ghostty-config",
        ])
        .arg(&profile)
        .args([
            "--font-size",
            "18",
            "--padding-x",
            "6",
            "--padding-y",
            "4",
            "--foreground",
            "#e6e6e6",
            "--background",
            "#101010",
            "--out",
        ])
        .arg(&output)
        .args([
            "--",
            "/bin/sh",
            "-c",
            r#"printf "\033[2J\033[H\033[38;2;255;0;0mLong preroll\033[0m\n"; sleep 12; printf "\033[38;2;0;255;0mSelected range\033[0m\n"; sleep 3; printf "excluded tail\n""#,
        ])
        .timeout(Duration::from_secs(60))
        .assert()
        .success();

    fs_err::remove_file(&profile).expect("remove source Ghostty profile");

    termtrace()
        .args(["analyze", "index"])
        .arg(&output)
        .assert()
        .success();

    termtrace()
        .args(["artifact", "capture"])
        .arg(&output)
        .args(["--fps", "24"])
        .timeout(Duration::from_secs(180))
        .assert()
        .success();

    termtrace()
        .arg("screenshot")
        .arg(&output)
        .args([
            "--renderer",
            "native",
            "--at",
            "12.2s",
            "--viewport",
            &viewport,
            "--strict",
            "--out",
        ])
        .arg(output.join("native-single"))
        .timeout(Duration::from_secs(90))
        .assert()
        .success();

    termtrace()
        .args(["analyze", "index"])
        .arg(&output)
        .args(["--from", &from, "--to", &to])
        .assert()
        .success();

    let range_started = Instant::now();
    termtrace()
        .args(["artifact", "capture"])
        .arg(&output)
        .args(["--fps", "24", "--from", &from, "--to", &to])
        .timeout(Duration::from_secs(90))
        .assert()
        .success();

    assert!(
        range_started.elapsed() < Duration::from_nanos(RANGE_FROM_NS),
        "ranged capture did not fast-forward the pre-roll: {:?}",
        range_started.elapsed()
    );

    let manifest = read_json(&output.join("visual/manifest.json"));
    let range_manifest = read_json(&range_visual.join("manifest.json"));
    let source_range = serde_json::json!({
        "start_monotonic_ns": RANGE_FROM_NS,
        "end_monotonic_ns": RANGE_TO_NS,
    });

    assert_capture_manifest(&manifest, &serde_json::Value::Null);

    assert_video_map(
        &read_json_lines(&output.join("visual/video-map.jsonl")),
        None,
    );

    assert_capture_manifest(&range_manifest, &source_range);

    assert_video_map(
        &read_json_lines(&range_visual.join("video-map.jsonl")),
        Some((RANGE_FROM_NS, RANGE_TO_NS)),
    );

    let derived_range = &read_json(&range_derived.join("manifest.json"))["source_range"];

    assert_eq!(derived_range["start_monotonic_ns"], RANGE_FROM_NS);
    assert_eq!(derived_range["end_monotonic_ns"], RANGE_TO_NS);

    let final_text = fs_err::read_to_string(range_derived.join("final.txt")).expect("final text");
    assert!(
        final_text.contains("Long preroll"),
        "range lost the pre-roll"
    );
    assert!(
        final_text.contains("Selected range"),
        "range lost the selection"
    );
    assert!(
        !final_text.contains("excluded tail"),
        "range included the tail"
    );
    assert!(
        video_duration(&range_manifest) + 5.0 < video_duration(&manifest),
        "ranged video is not at least five seconds shorter than the full video"
    );

    let session = read_json(&output.join("session.json"));
    assert_session_manifest(&session);
    let session_hash = &session["ghostty_visual"]["effective_config_hash"];

    assert_eq!(&manifest["effective_config_hash"], session_hash);
    assert_eq!(&range_manifest["effective_config_hash"], session_hash);

    assert_nonempty_files(
        &output,
        &[
            "ghostty-profile.ghostty",
            "visual/ghostty-effective-config.txt",
            "visual/session.mkv",
            "visual/final.png",
            "visual/video-map.jsonl",
        ],
    );

    assert_media_stream(&output.join("visual/session.mkv"), "h264");
    assert_media_stream(&output.join("visual/final.png"), "png");

    let replay_profile =
        fs_err::read_to_string(output.join("ghostty-profile.ghostty")).expect("replay profile");

    let external_config = replay_profile.lines().find(|line| {
        return line.split_once('=').is_some_and(|(key, _)| {
            return matches!(key.trim(), "config-file" | "config-default-files");
        });
    });
    assert_eq!(
        external_config, None,
        "replay profile references external configuration"
    );

    let point = output.join("native-single");
    assert_nonempty_files(&point, &["final.png", "ghostty-effective-config.txt"]);
    assert_media_stream(&point.join("final.png"), "png");

    assert!(
        point.join("session.mkv").symlink_metadata().is_err(),
        "point screenshot recorded a video"
    );

    assert_point_screenshot_manifest(&read_json(&point.join("manifest.json")));

    assert_nonempty_files(
        &range_visual,
        &[
            "session.mkv",
            "final.png",
            "ghostty-effective-config.txt",
            "video-map.jsonl",
        ],
    );

    assert_media_stream(&range_visual.join("session.mkv"), "h264");
    assert_media_stream(&range_visual.join("final.png"), "png");

    let roots = read_json_lines(&output.join("artifact-roots.jsonl"));
    assert!(
        roots.len() >= 5,
        "artifact catalog has {} roots",
        roots.len()
    );

    let directories: BTreeSet<&str> = roots
        .iter()
        .filter_map(|root| return root["artifact_directory"].as_str())
        .collect();

    for expected in [
        "derived".to_owned(),
        "visual".to_owned(),
        "native-single".to_owned(),
        format!("derived-{range_suffix}"),
        format!("visual-{range_suffix}"),
    ] {
        assert!(
            directories.contains(expected.as_str()),
            "artifact catalog lacks {expected}: {directories:?}"
        );
    }

    assert_ne!(
        decoded_pixels(&output.join("visual/final.png")),
        decoded_pixels(&range_visual.join("final.png")),
        "full and ranged screenshots have identical decoded pixels"
    );

    let integrity = termtrace()
        .args(["integrity", "verify", "--level", "content", "--json"])
        .arg(&output)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let report: serde_json::Value =
        serde_json::from_slice(&integrity).expect("integrity report JSON");
    assert_eq!(report["valid"], true, "{report}");
    assert_eq!(report["findings"], serde_json::json!([]), "{report}");
    assert!(
        report["artifact_roots_verified"]
            .as_u64()
            .is_some_and(|count| return count >= 5),
        "{report}"
    );
}
