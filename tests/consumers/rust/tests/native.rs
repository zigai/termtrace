mod support;

use serde_json::Value;
use std::{
    fs,
    path::Path,
    process::Command,
    sync::{atomic::AtomicBool, Arc},
    thread,
    time::{Duration, Instant},
};
use support::{binary, scratch, shell, stop, terminal, wait_text, Launch, Live};
use termtrace_sdk::{
    artifacts::{
        native_capture_target, validate_replay_screenshot, validate_visual_manifest, CaptureKind,
        CaptureOptions, StateSelection, VisualManifest,
    },
    session::LaunchSpec,
    StatePoint,
};

const NATIVE_TIMEOUT: Duration = Duration::from_secs(120);

fn png_dimensions(path: &Path) -> (u32, u32) {
    let bytes = fs::read(path).expect("png");
    assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n", "{}", path.display());
    assert_eq!(&bytes[12..16], b"IHDR");
    let word = |at: usize| return u32::from_be_bytes(bytes[at..at + 4].try_into().expect("u32"));

    return (word(16), word(20));
}

fn recorded(name: &str, script: &str, ready: &str, cols: u16, rows: u16) -> Live {
    let mut session = terminal(name)
        .launch(LaunchSpec::new(shell(script)).geometry(cols, rows))
        .expect("launch");
    wait_text(&session, ready);
    stop(&mut session);

    return session;
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn replay_screenshots_capture_the_selected_state() {
    let session = recorded(
        "native-screenshot",
        "printf 'FIRST'; sleep 0.3; printf '\\r\\n\\033[42mSECOND-STATE\\033[0m'; IFS= read -r _",
        "SECOND-STATE",
        40,
        8,
    );
    let first_state = session
        .semantic()
        .timeline(false, false)
        .expect("timeline")
        .iter()
        .map(|entry| return serde_json::to_value(&entry.item).expect("item"))
        .find(|item| {
            return item["kind"] == "semantic_state" && item["changed_cells"].as_u64() > Some(0);
        })
        .expect("first drawn state")["state_id"]
        .as_u64()
        .expect("state id");
    let output = scratch("native-screenshot-output");
    let cancelled = Arc::new(AtomicBool::new(false));

    let mut worker = session
        .native()
        .replay_worker(None, Arc::clone(&cancelled), NATIVE_TIMEOUT)
        .expect("replay worker");
    let early = worker
        .capture(
            &StatePoint::StateId {
                state_id: first_state,
            },
            &output.join("early"),
        )
        .expect("early capture");
    let latest = worker
        .capture(&StatePoint::LatestPresented, &output.join("latest"))
        .expect("latest capture");
    worker.close();

    let endpoint =
        validate_replay_screenshot(session.path(), &latest.directory, &cancelled).expect("valid");
    let observed = session
        .observe(StatePoint::LatestPresented, false)
        .expect("latest");
    assert_eq!(endpoint.state_hash, observed.state_hash);
    let early_endpoint =
        validate_replay_screenshot(session.path(), &early.directory, &cancelled).expect("valid");
    assert_eq!(early_endpoint.state_id, first_state);
    assert_ne!(early_endpoint.state_hash, endpoint.state_hash);

    let single = session
        .native()
        .screenshot(
            &StatePoint::StateId {
                state_id: first_state,
            },
            &output.join("single"),
            None,
            Arc::clone(&cancelled),
            NATIVE_TIMEOUT,
        )
        .expect("single screenshot");
    let single_endpoint =
        validate_replay_screenshot(session.path(), &single.directory, &cancelled).expect("valid");
    assert_eq!(single_endpoint.state_hash, early_endpoint.state_hash);
    assert!(
        session
            .native()
            .screenshot(
                &StatePoint::LatestPresented,
                &output.join("single"),
                None,
                Arc::clone(&cancelled),
                NATIVE_TIMEOUT,
            )
            .is_err(),
        "an existing destination is not replaced"
    );
    assert_eq!(
        single_endpoint.state_hash,
        validate_replay_screenshot(session.path(), &single.directory, &cancelled)
            .expect("still valid")
            .state_hash
    );

    let (width, height) = png_dimensions(&latest.screenshot);
    assert!(width >= 64 && height >= 32, "{width}x{height}");
    assert_eq!(png_dimensions(&early.screenshot), (width, height));
    assert_ne!(
        fs::read(&early.screenshot).expect("early png"),
        fs::read(&latest.screenshot).expect("latest png"),
        "different states render different pixels"
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn capture_publishes_a_video_whose_manifest_validates() {
    let session = recorded(
        "native-video",
        "printf 'VIDEO-ONE'; sleep 0.3; printf '\\r\\nVIDEO-TWO'; IFS= read -r _",
        "VIDEO-TWO",
        40,
        8,
    );
    let completion = session
        .completion()
        .expect("completion")
        .expect("completed");
    let indexed = Command::new(binary())
        .args(["analyze", "index", "--quiet"])
        .arg(session.path())
        .status()
        .expect("termtrace analyze index");
    assert!(indexed.success());
    let (target, geometry_solver) =
        native_capture_target(&binary(), session.path(), None).expect("capture target");

    let outcome = session
        .native()
        .capture(&CaptureOptions {
            session_directory: session.path().to_path_buf(),
            width: target.width,
            height: target.height,
            padding: target.padding,
            fps: 24,
            start_monotonic_ns: None,
            end_monotonic_ns: None,
            state_selection: StateSelection::All,
            geometry_solver,
            kind: CaptureKind::Video,
            destination: None,
            scale: None,
            dpi: None,
        })
        .expect("native capture");

    let manifest: VisualManifest = serde_json::from_slice(
        &fs::read(outcome.directory.join("manifest.json")).expect("manifest"),
    )
    .expect("manifest JSON");
    validate_visual_manifest(&manifest).expect("manifest validates");
    assert_eq!(manifest.session_id, completion.session_id);
    assert_eq!(
        manifest.source_event_hash.as_deref(),
        Some(completion.final_event_hash.as_str())
    );
    let video = outcome.video.expect("video path");
    let video_name = video
        .strip_prefix(&outcome.directory)
        .expect("video inside the capture")
        .to_str()
        .expect("UTF-8 member");
    let member = manifest
        .members
        .iter()
        .find(|member| return member.path == video_name)
        .expect("video member");
    assert_eq!(
        member.byte_length,
        fs::metadata(&video).expect("video").len()
    );
    assert!(member.byte_length > 0);
    assert_eq!(
        png_dimensions(&outcome.screenshot),
        (u32::from(manifest.width), u32::from(manifest.height))
    );
}

#[test]
#[ignore = "requires tmux as the playback terminal"]
fn playback_replays_the_session_onto_the_calling_terminal() {
    let session = recorded(
        "native-playback",
        "printf 'REPLAYED-ALPHA\\r\\nREPLAYED-BETA'; IFS= read -r _",
        "REPLAYED-BETA",
        60,
        8,
    );
    let completion = session
        .completion()
        .expect("completion")
        .expect("completed");
    let handshake = scratch("native-playback-handshake");
    let server = scratch("native-playback-tmux").join("server");
    let tmux = |arguments: &[&str]| {
        return Command::new("tmux")
            .arg("-S")
            .arg(&server)
            .args(["-f", "/dev/null"])
            .args(arguments)
            .env_remove("TMUX")
            .output()
            .expect("tmux");
    };
    let script = format!(
        "'{}' '{}' '{}' '{}'; printf ' EXIT=%s' $?; sleep 60",
        env!("CARGO_BIN_EXE_native_playback"),
        binary().display(),
        session.path().display(),
        handshake.display(),
    );

    let started = tmux(&["new-session", "-d", "-x", "60", "-y", "8", &script]);
    assert!(started.status.success(), "{started:?}");
    let deadline = Instant::now() + Duration::from_secs(30);
    let pane = loop {
        let pane = String::from_utf8_lossy(&tmux(&["capture-pane", "-p"]).stdout).into_owned();
        if pane.contains("EXIT=") || Instant::now() > deadline {
            break pane;
        }
        thread::sleep(Duration::from_millis(100));
    };
    tmux(&["kill-server"]);

    assert!(pane.contains("EXIT=0"), "{pane}");
    let lines: Vec<&str> = pane.lines().collect();
    assert_eq!(lines[0].trim_end(), "REPLAYED-ALPHA", "{pane}");
    assert!(lines[1].starts_with("REPLAYED-BETA"), "{pane}");
    assert!(handshake.join("ready").is_file());
    assert!(!handshake.join("error.json").exists());
    let timing: Value =
        serde_json::from_slice(&fs::read(handshake.join("timing.json")).expect("timing"))
            .expect("timing JSON");
    assert_eq!(timing["source_event_count"], completion.event_count);
    assert_eq!(timing["source_event_hash"], completion.final_event_hash);
    let geometry: Value =
        serde_json::from_slice(&fs::read(handshake.join("geometry.json")).expect("geometry"))
            .expect("geometry JSON");
    assert_eq!(
        (geometry["cols"].as_u64(), geometry["rows"].as_u64()),
        (Some(60), Some(8))
    );
}
