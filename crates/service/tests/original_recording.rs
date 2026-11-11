//! Original recordings against the real Ghostty, Xvfb and FFmpeg toolchain.

use assert_cmd::cargo::cargo_bin;
use nix::{
    sys::{
        signal::{kill, Signal},
        stat::Mode,
    },
    unistd::{mkfifo, Pid},
};
use std::{
    ffi::OsStr,
    fs,
    os::unix::{ffi::OsStrExt as _, fs::PermissionsExt as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;
use termtrace_model::{
    artifact::{
        original::{
            original_capture_status_path, CellGrid, OriginalCaptureFacts, OriginalCaptureFailure,
            OriginalCaptureState, PixelCanvas, SurvivingKind,
        },
        sdk::ArtifactProvenance,
    },
    execution::visual_profile::{NativeVisualProfile, VisualProfile},
    protocol::{control::ScreenPredicate, mutation::MutationEffect},
    terminal::state::{KeyChord, KeyModifiers, NamedKey},
    value::ByteString,
};
use termtrace_process::ProcessIdentity;
use termtrace_service::{
    original_capture_status, ActRequest, Error, ErrorKind, MutationGuard, OriginalGeometry,
    OriginalLaunchSpec, OriginalSession, OutputService, RegistryService, SessionStatus,
    SessionTarget, StopRequest, Terminal,
};
use uuid::Uuid;

const FINISH: Duration = Duration::from_secs(60);

fn spec(script: &str) -> OriginalLaunchSpec {
    let mut spec = OriginalLaunchSpec::new(["/bin/sh", "-c", script]);
    spec.geometry = OriginalGeometry::Grid(CellGrid { cols: 80, rows: 24 });
    spec.canvas = Some(PixelCanvas {
        width: 1280,
        height: 720,
    });

    spec.startup_timeout = Duration::from_secs(60);
    return spec;
}

#[allow(
    clippy::panic,
    reason = "test helper should stop when the output never appears"
)]
fn wait_for_output(session: &OriginalSession, text: &str) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        let output = OutputService::page(session.session().path(), None, 1 << 20)
            .map(|page| return page.bytes.as_bytes().to_vec())
            .unwrap_or_default();

        if String::from_utf8_lossy(&output).contains(text) {
            return;
        }

        thread::sleep(Duration::from_millis(20));
    }

    panic!("`{text}` did not appear in the event log");
}

#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test helper should stop when /proc cannot be read"
)]
fn descendants(root: u32) -> Vec<ProcessIdentity> {
    let mut found = vec![root];

    let mut index = 0;
    while index < found.len() {
        let parent = found[index];
        for entry in fs::read_dir("/proc").expect("proc").flatten() {
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| return name.parse::<u32>().ok())
            else {
                continue;
            };
            let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
                continue;
            };
            let Some((_, fields)) = stat.rsplit_once(") ") else {
                continue;
            };
            let fields: Vec<&str> = fields.split(' ').collect();

            if fields[1].parse::<u32>().ok() == Some(parent) && !found.contains(&pid) {
                found.push(pid);
            }
        }

        index += 1;
    }

    return found
        .into_iter()
        .filter_map(|pid| return ProcessIdentity::of(pid).ok())
        .collect();
}

fn assert_all_exited(processes: &[ProcessIdentity]) {
    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        let alive: Vec<_> = processes
            .iter()
            .filter(|process| {
                return ProcessIdentity::of(process.pid)
                    .is_ok_and(|current| return current.start_ticks == process.start_ticks);
            })
            .collect();

        if alive.is_empty() {
            return;
        }

        assert!(
            Instant::now() < deadline,
            "processes outlived teardown: {alive:?}"
        );
        thread::sleep(Duration::from_millis(50));
    }
}

#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    reason = "test helper should stop when the supervisor status is invalid"
)]
fn supervisor_tree(session: &OriginalSession) -> Vec<ProcessIdentity> {
    let status = session.capture_status().expect("status");
    let tree = descendants(u32::try_from(status.supervisor_pid.0).unwrap());
    assert!(
        tree.len() > 4,
        "supervisor owns its display, encoder and terminal: {tree:?}"
    );

    return tree;
}

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when the capture has no encoder"
)]
fn kill_encoder(tree: &[ProcessIdentity]) {
    let ffmpeg = tree
        .iter()
        .find(|process| {
            return fs::read(format!("/proc/{}/comm", process.pid))
                .is_ok_and(|comm| return comm.starts_with(b"ffmpeg"));
        })
        .expect("encoder process");

    kill(
        Pid::from_raw(i32::try_from(ffmpeg.pid).expect("encoder PID")),
        Signal::SIGKILL,
    )
    .expect("kill encoder");
}

#[allow(
    clippy::panic,
    reason = "test helper should stop on an unexpected error"
)]
fn capture_failure(error: &Error) -> &OriginalCaptureFacts {
    let Error::OriginalCaptureFailed { facts } = error else {
        panic!("unexpected error {error:?}");
    };
    return facts;
}

fn surviving_paths(facts: &OriginalCaptureFacts, kind: SurvivingKind) -> Vec<PathBuf> {
    return facts
        .surviving
        .iter()
        .filter(|record| return record.kind == kind)
        .map(|record| return PathBuf::from(OsStr::from_bytes(record.path.as_bytes())))
        .collect();
}

fn assert_surviving_paths_exist(facts: &OriginalCaptureFacts) {
    for record in &facts.surviving {
        let path = Path::new(OsStr::from_bytes(record.path.as_bytes()));
        assert!(
            path.symlink_metadata().is_ok(),
            "{record:?} is reported but absent"
        );
    }
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn exact_grid_resizes_inside_the_canvas_and_publishes_the_capture() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(cargo_bin("termtrace"), temporary.path());
    let mut original = terminal
        .start_original(
            &spec("trap 'echo winch $(stty size)' WINCH; echo ready $(stty size); while :; do sleep 0.05; done"),
            &AtomicBool::new(false),
        )
        .expect("start");

    let status = original.capture_status().expect("status");
    assert_eq!(status.state, OriginalCaptureState::Live);
    wait_for_output(&original, "ready 24 80");

    let applied = original
        .session()
        .resize(100, 30, &MutationGuard::default())
        .expect("resize inside the canvas");

    drop(applied);

    wait_for_output(&original, "winch 30 100");

    let rejected = original
        .session()
        .resize(400, 200, &MutationGuard::default())
        .expect_err("resize beyond the canvas");
    assert_eq!(rejected.code(), "exceeds_native_canvas");
    assert_eq!(rejected.effect(), MutationEffect::NotApplied);

    let recording = original
        .finish(StopRequest::default(), FINISH)
        .expect("finish");
    assert_eq!(
        recording.capture.provenance,
        ArtifactProvenance::OriginalExecution
    );

    let directory = Path::new(OsStr::from_bytes(
        recording.capture.directory_bytes.as_bytes(),
    ))
    .to_path_buf();

    let resizes = fs::read_to_string(directory.join("window-resizes.jsonl")).expect("resizes");
    let lines: Vec<serde_json::Value> = resizes
        .lines()
        .map(|line| return serde_json::from_str(line).expect("resize record"))
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["decision"], "configured");
    assert_eq!(lines[0]["observed"]["cols"], 100);
    assert!(lines[0]["event_seq"].is_string());
    assert_eq!(lines[1]["decision"], "rejected");
    assert_eq!(lines[1]["code"], "exceeds_native_canvas");
    assert_eq!(
        original.capture_status().expect("status").state,
        OriginalCaptureState::Published
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn failing_application_still_publishes_its_capture() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(cargo_bin("termtrace"), temporary.path());
    let mut original = terminal
        .start_original(
            &spec("echo failing; sleep 0.5; exit 7"),
            &AtomicBool::new(false),
        )
        .expect("start");
    let recording = original.wait_finished(FINISH).expect("capture publishes");
    assert_eq!(recording.completion.exit_code, Some(7));
    assert!(!recording.canonical_success);
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session, FFmpeg and JetBrains Mono"]
fn visual_profile_is_resolved_before_startup_and_committed_into_the_session() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(cargo_bin("termtrace"), temporary.path());

    let mut profile = VisualProfile::native_standard();
    if let Some(native) = profile.native.as_mut() {
        native.dpi = Some(144);
    }

    let mut missing = spec("echo never");
    missing.visual_profile = Some(VisualProfile {
        version: 1,
        native: Some(NativeVisualProfile {
            font_family: Some("Termtrace Missing Family 7c1d".to_owned()),
            ..NativeVisualProfile::default()
        }),
        semantic: None,
    });
    let Err(error) = terminal.start_original(&missing, &AtomicBool::new(false)) else {
        panic!("a missing font must not start the capture");
    };

    assert_eq!(error.code(), "font_unavailable");
    assert_eq!(
        fs::read_dir(temporary.path())
            .expect("evidence root")
            .count(),
        0
    );

    let mut profiled = spec("echo profiled; sleep 0.3");
    profiled.geometry = OriginalGeometry::Grid(CellGrid { cols: 48, rows: 8 });
    profiled.visual_profile = Some(profile.clone());
    let mut original = terminal
        .start_original(&profiled, &AtomicBool::new(false))
        .expect("start");
    let recording = original.wait_finished(FINISH).expect("capture publishes");
    assert!(recording.canonical_success);
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(original.session().path().join("session.json")).expect("session manifest"),
    )
    .expect("manifest JSON");

    assert_eq!(manifest["version"], 13);
    assert_eq!(
        manifest["visual_profile"]["profile_hash"],
        profile.identity().expect("profile identity")
    );
    assert_eq!(manifest["visual_profile"]["profile"]["native"]["dpi"], 144);
    assert_eq!(
        (
            manifest["geometry"]["cols"].as_u64(),
            manifest["geometry"]["rows"].as_u64()
        ),
        (Some(48), Some(8))
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn killed_encoder_fails_the_capture_but_keeps_canonical_completion() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(cargo_bin("termtrace"), temporary.path());
    let mut original = terminal
        .start_original(
            &spec("echo running; while :; do sleep 0.05; done"),
            &AtomicBool::new(false),
        )
        .expect("start");

    let tree = supervisor_tree(&original);
    kill_encoder(&tree);

    let error = original
        .finish(StopRequest::default(), FINISH)
        .expect_err("capture fails");
    assert_eq!(error.kind(), ErrorKind::OriginalCaptureFailed);

    let facts = capture_failure(&error);
    assert_eq!(facts.code, Some(OriginalCaptureFailure::AcquisitionFailed));
    assert!(
        facts.completion.is_some(),
        "the application ran and completed"
    );
    assert_eq!(error.effect(), MutationEffect::Unknown);
    assert_eq!(facts.cleanup, None);

    let preserved = surviving_paths(facts, SurvivingKind::FailedVisual);
    assert_eq!(preserved.len(), 1, "{:?}", facts.surviving);
    assert!(
        preserved[0]
            .file_name()
            .is_some_and(|name| return name.as_bytes().starts_with(b"visual-failed-")),
        "{preserved:?}"
    );
    assert!(preserved[0].join("session.mkv").is_file());

    assert_surviving_paths_exist(facts);

    assert_all_exited(&tree);
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn failed_preservation_reports_cleanup_and_no_preserved_capture() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(cargo_bin("termtrace"), temporary.path());
    let mut original = terminal
        .start_original(
            &spec("echo running; while :; do sleep 0.05; done"),
            &AtomicBool::new(false),
        )
        .expect("start");

    let parent = original
        .session()
        .path()
        .parent()
        .expect("session parent")
        .to_path_buf();

    let staging = fs::read_dir(&parent)
        .expect("session parent entries")
        .map(|entry| return entry.expect("entry").path())
        .find(|path| {
            return path
                .file_name()
                .is_some_and(|name| return name.as_bytes().starts_with(b".original-visual."));
        })
        .expect("capture staging directory");

    fs::remove_dir_all(&staging).expect("remove capture staging");

    let tree = supervisor_tree(&original);
    kill_encoder(&tree);

    let error = original
        .finish(StopRequest::default(), FINISH)
        .expect_err("capture fails");

    let facts = capture_failure(&error);
    assert_eq!(facts.code, Some(OriginalCaptureFailure::AcquisitionFailed));
    assert!(facts.cleanup.is_some(), "{facts:?}");
    assert!(
        surviving_paths(facts, SurvivingKind::FailedVisual).is_empty(),
        "{:?}",
        facts.surviving
    );
    assert!(fs::read_dir(&parent)
        .expect("session parent entries")
        .all(|entry| {
            return !entry
                .expect("entry")
                .file_name()
                .as_bytes()
                .starts_with(b"visual-failed-");
        }));

    assert_surviving_paths_exist(facts);

    assert_all_exited(&tree);
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn force_close_abandons_the_capture_and_leaves_no_processes() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(cargo_bin("termtrace"), temporary.path());
    let mut original = terminal
        .start_original(
            &spec("trap '' HUP TERM; echo stubborn; while :; do sleep 0.05; done"),
            &AtomicBool::new(false),
        )
        .expect("start");

    let tree = supervisor_tree(&original);

    original.force_close().expect("force close");

    assert_all_exited(&tree);

    assert_eq!(
        original.capture_status().expect("status").state,
        OriginalCaptureState::Abandoned
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn cancelled_startup_tears_down_and_reports_cancellation() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(cargo_bin("termtrace"), temporary.path());
    let cancelled = AtomicBool::new(false);
    let (started, tree) = thread::scope(|scope| {
        let canceller = scope.spawn(|| {
            // Cancel while the display and encoder are being acquired.
            let deadline = Instant::now() + Duration::from_secs(30);
            let status = loop {
                let found = fs::read_dir(temporary.path())
                    .expect("root")
                    .flatten()
                    .find(|entry| {
                        return entry
                            .file_name()
                            .to_string_lossy()
                            .ends_with(".original-capture.json");
                    });

                if let Some(entry) = found
                    && let Ok(bytes) = fs::read(entry.path())
                    && let Ok(status) = serde_json::from_slice::<serde_json::Value>(&bytes)
                {
                    break status;
                }

                assert!(Instant::now() < deadline, "capture never started acquiring");

                thread::sleep(Duration::from_millis(5));
            };

            thread::sleep(Duration::from_millis(100));

            let supervisor = status["supervisor_pid"].as_str().unwrap().parse().unwrap();
            let tree = descendants(supervisor);

            cancelled.store(true, Ordering::Release);

            return tree;
        });

        let started = terminal.start_original(&spec("echo never; sleep 30"), &cancelled);

        return (started, canceller.join().expect("canceller"));
    });

    assert_all_exited(&tree);

    let Err(error) = started else {
        panic!("cancelled startup returned a session");
    };
    let Error::OriginalCaptureFailed { facts } = &error else {
        panic!("unexpected error {error:?}");
    };

    assert_eq!(facts.code, Some(OriginalCaptureFailure::Cancelled));

    let leftovers: Vec<_> = fs::read_dir(temporary.path())
        .expect("evidence root")
        .flatten()
        .map(|entry| return entry.file_name().to_string_lossy().into_owned())
        .filter(|name| return name.starts_with(".original-environment"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "environment staging survived: {leftovers:?}"
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn cancellation_during_profile_resolution_stops_the_query_and_reports_cancellation() {
    let temporary = TempDir::new().expect("temporary directory");
    let evidence = temporary.path().join("evidence");
    let terminal = Terminal::new(cargo_bin("termtrace"), &evidence);

    // Ghostty blocks opening a FIFO config that has no writer, so the query never finishes.
    let config = temporary.path().join("config.ghostty");
    mkfifo(&config, Mode::S_IRUSR | Mode::S_IWUSR).expect("config FIFO");

    let mut blocked = spec("echo never");
    blocked.visual_profile = Some(VisualProfile {
        version: 1,
        native: Some(NativeVisualProfile {
            config: Some(config.to_str().expect("UTF-8 path").to_owned()),
            ..NativeVisualProfile::default()
        }),
        semantic: None,
    });

    let cancelled = AtomicBool::new(false);
    let (outcome, returned, cancelled_at) = thread::scope(|scope| {
        let canceller = scope.spawn(|| {
            let deadline = Instant::now() + Duration::from_secs(30);
            while processes_naming(&config).is_empty() {
                assert!(
                    Instant::now() < deadline,
                    "Ghostty never queried the config"
                );

                thread::sleep(Duration::from_millis(5));
            }

            thread::sleep(Duration::from_millis(200));

            cancelled.store(true, Ordering::Release);

            return Instant::now();
        });

        let outcome = terminal.start_original(&blocked, &cancelled);
        let returned = Instant::now();

        return (outcome, returned, canceller.join().expect("canceller"));
    });

    let Err(error) = outcome else {
        panic!("a cancelled launch returned a session");
    };
    let Error::OriginalCaptureFailed { facts } = &error else {
        panic!("unexpected error {error:?}");
    };

    assert_eq!(facts.code, Some(OriginalCaptureFailure::Cancelled));
    assert!(returned.duration_since(cancelled_at) < Duration::from_secs(5));
    assert!(!evidence.exists());

    let queries = processes_naming(&config);
    assert!(queries.is_empty(), "Ghostty query survived: {queries:?}");
}

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when /proc cannot be read"
)]
fn processes_naming(path: &Path) -> Vec<String> {
    let argument = path.as_os_str().as_bytes();

    return fs::read_dir("/proc")
        .expect("process table")
        .flatten()
        .filter(|entry| {
            return fs::read(entry.path().join("cmdline")).is_ok_and(|cmdline| {
                return cmdline
                    .windows(argument.len())
                    .any(|window| return window == argument);
            });
        })
        .map(|entry| return entry.file_name().to_string_lossy().into_owned())
        .collect();
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn secret_environment_values_stay_out_of_metadata_and_arguments() {
    let secret = "termtrace-secret-6b1f0c";
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(cargo_bin("termtrace"), temporary.path());
    let mut launch = spec("test -n \"$SECRET_TOKEN\" && echo has-secret; sleep 0.5");
    launch.environment.set.insert(
        "SECRET_TOKEN".to_owned(),
        ByteString::from(secret.as_bytes()),
    );
    let mut original = terminal
        .start_original(&launch, &AtomicBool::new(false))
        .expect("start");

    for process in supervisor_tree(&original) {
        let cmdline = fs::read(format!("/proc/{}/cmdline", process.pid)).unwrap_or_default();
        assert!(
            !String::from_utf8_lossy(&cmdline).contains(secret),
            "secret in arguments of {}",
            process.pid
        );
    }

    original.wait_finished(FINISH).expect("published");
    wait_for_output(&original, "has-secret");

    for entry in walkdir(temporary.path()) {
        let bytes = fs::read(&entry).unwrap_or_default();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(secret),
            "secret written to {}",
            entry.display()
        );
    }
}

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when the evidence root cannot be read"
)]
fn walkdir(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).expect("directory").flatten() {
            let path = entry.path();

            if entry.file_type().expect("type").is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }

    return files;
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn named_recording_restarts_into_a_faithful_successor() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let terminal = Terminal::new(cargo_bin("termtrace"), temporary.path().join("evidence"));
    let mut launch = spec("echo generation $(stty size); while :; do sleep 0.05; done");
    launch.alias = Some("original-demo".to_owned());
    launch.state_root = Some(state.clone());
    let predecessor = terminal
        .start_original(&launch, &AtomicBool::new(false))
        .expect("start")
        .detach();

    let registry = RegistryService::open(&state).expect("registry");
    let report = registry
        .restart_original(&cargo_bin("termtrace"), "original-demo", None, FINISH)
        .expect("restart");

    assert_eq!(
        original_capture_status(&predecessor)
            .expect("predecessor")
            .state,
        OriginalCaptureState::Published
    );

    let mut successor = terminal
        .attach_original(report.successor.session_path())
        .expect("attach successor");

    let status = successor.capture_status().expect("successor status");
    assert_eq!(status.grid, Some(CellGrid { cols: 80, rows: 24 }));
    assert_eq!(
        status.canvas,
        PixelCanvas {
            width: 1280,
            height: 720
        }
    );

    wait_for_output(&successor, "generation 24 80");

    let recording = successor
        .finish(StopRequest::default(), FINISH)
        .expect("successor publishes");
    assert_eq!(
        recording.capture.provenance,
        ArtifactProvenance::OriginalExecution
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn named_background_recording_attaches_by_alias_and_exact_path_bytes() {
    let temporary = TempDir::new().expect("temporary directory");
    let state = temporary.path().join("state");
    let evidence = temporary.path().join(OsStr::from_bytes(b"evidence-\xff"));
    fs::create_dir_all(&evidence).expect("non-UTF-8 evidence root");
    let terminal = Terminal::new(cargo_bin("termtrace"), &evidence);
    let mut launch = spec(
        "(trap 'echo winch $(stty size </dev/tty)' WINCH; while :; do sleep 0.05; done) & echo ready; while read line; do echo got $line; done",
    );
    launch.alias = Some("original-attach".to_owned());
    launch.state_root = Some(state.clone());
    let detached = terminal
        .start_original(&launch, &AtomicBool::new(false))
        .expect("start")
        .detach();
    let named = terminal
        .attach_original_target(
            &SessionTarget::Name("original-attach".to_owned()),
            Some(&state),
        )
        .expect("attach by alias");
    assert_eq!(
        named.capture_status().expect("status").state,
        OriginalCaptureState::Live
    );

    wait_for_output(&named, "ready");
    named
        .session()
        .resize(100, 30, &MutationGuard::default())
        .expect("resize through the alias");

    wait_for_output(&named, "winch 30 100");

    let canonical = fs::canonicalize(&detached).expect("canonical session");
    assert!(canonical.as_os_str().as_bytes().contains(&0xff));

    let mut exact = terminal
        .attach_original_target(&SessionTarget::Path(canonical.clone()), None)
        .expect("attach by exact path bytes");
    assert_eq!(exact.session().session_id(), named.session().session_id());
    assert!(terminal
        .attach_original_target(
            &SessionTarget::Name("original-missing".to_owned()),
            Some(&state)
        )
        .is_err());

    let control = exact.session();
    assert_eq!(control.path(), canonical);

    let presented = control
        .wait_screen(contains("ready"), Duration::ZERO, Duration::from_secs(20))
        .expect("presented screen");
    assert!(presented.matched);

    let acted = control
        .act(
            ActRequest::new()
                .text("hello")
                .key(KeyChord {
                    key: NamedKey::Enter,
                    modifiers: KeyModifiers::default(),
                })
                .if_state(presented.frame.state_hash)
                .until_screen(contains("got hello"), Duration::from_secs(20)),
        )
        .expect("guarded action on the exact path");
    assert!(acted.is_matched());

    control
        .resize(90, 26, &MutationGuard::default())
        .expect("resize through the exact path");

    wait_for_output(&exact, "winch 26 90");

    let recording = exact
        .finish(StopRequest::default(), FINISH)
        .expect("finish through the exact path");
    assert_eq!(
        recording.capture.provenance,
        ArtifactProvenance::OriginalExecution
    );
    assert_eq!(
        recording.capture.source.session_id,
        exact.session().session_id()
    );

    let sidecar = original_capture_status_path(&canonical);
    let published = fs::read(&sidecar).expect("sidecar");
    fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o644)).expect("chmod");

    assert!(
        terminal.attach_original(&canonical).is_err(),
        "group-readable status"
    );

    fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600)).expect("chmod");

    let mut foreign: serde_json::Value = serde_json::from_slice(&published).expect("status");
    foreign["result"]["source"]["session_id"] =
        serde_json::Value::from("00000000-0000-0000-0000-000000000001");
    fs::write(&sidecar, serde_json::to_vec(&foreign).expect("json")).expect("rewrite");

    assert!(
        terminal.attach_original(&canonical).is_err(),
        "status published for another session"
    );

    fs::write(&sidecar, published).expect("restore");

    terminal
        .attach_original(&canonical)
        .expect("attach restored");
}

fn contains(text: &str) -> ScreenPredicate {
    return ScreenPredicate::Contains {
        text: text.to_owned(),
    };
}

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when its sidecar cannot be read"
)]
fn status_bytes(session: &Path) -> Vec<u8> {
    return fs::read(original_capture_status_path(session)).expect("status sidecar");
}

#[allow(
    clippy::expect_used,
    reason = "test helper should stop when its sidecar cannot be replaced"
)]
fn replace_status(session: &Path, bytes: &[u8]) {
    let path = original_capture_status_path(session);
    let staged = path.with_extension("rewrite");
    fs::write(&staged, bytes).expect("staged status");
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o600)).expect("chmod");
    fs::rename(&staged, &path).expect("replace status");
}

#[allow(
    clippy::expect_used,
    clippy::panic,
    reason = "test helper should stop when a forged capture is accepted"
)]
fn assert_rejected(attached: &mut OriginalSession, session_id: Uuid) {
    let error = attached
        .wait_finished(FINISH)
        .expect_err("a capture that does not belong to this session");

    let Error::OriginalCaptureFailed { facts } = &error else {
        panic!("unexpected error {error:?}");
    };
    assert_eq!(facts.code, Some(OriginalCaptureFailure::PublicationFailed));
    assert_eq!(
        facts
            .completion
            .as_ref()
            .map(|completion| return completion.session_id),
        Some(session_id),
        "canonical truth is kept"
    );
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn swapped_or_tampered_sidecars_never_publish_for_an_attached_session() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(cargo_bin("termtrace"), temporary.path());
    let mut first = terminal
        .start_original(&spec("echo first"), &AtomicBool::new(false))
        .expect("first");
    first.wait_finished(FINISH).expect("first publishes");
    let first_path = first.session().path().to_path_buf();
    let mut second = terminal
        .start_original(&spec("echo second"), &AtomicBool::new(false))
        .expect("second");
    second.wait_finished(FINISH).expect("second publishes");
    let second_path = second.session().path().to_path_buf();
    let second_id = second.session().session_id();
    let published = status_bytes(&second_path);

    let mut attached = terminal
        .attach_original(&second_path)
        .expect("attach the second session");

    replace_status(&second_path, &status_bytes(&first_path));

    assert!(
        attached.capture_status().is_err(),
        "another session's status"
    );

    assert_rejected(&mut attached, second_id);

    let mut redirected: serde_json::Value = serde_json::from_slice(&published).expect("status");
    let other: serde_json::Value =
        serde_json::from_slice(&status_bytes(&first_path)).expect("status");
    redirected["result"]["directory_bytes"] = other["result"]["directory_bytes"].clone();
    redirected["result"]["manifest"] = other["result"]["manifest"].clone();
    redirected["result"]["members"] = other["result"]["members"].clone();
    replace_status(
        &second_path,
        &serde_json::to_vec(&redirected).expect("json"),
    );

    assert_rejected(&mut attached, second_id);

    let mut shifted: serde_json::Value = serde_json::from_slice(&published).expect("status");
    shifted["result"]["source"]["boundary"]["output_bytes"] = serde_json::Value::from("1");
    replace_status(&second_path, &serde_json::to_vec(&shifted).expect("json"));

    assert_rejected(&mut attached, second_id);

    replace_status(&second_path, &published);
    let member = second_path
        .join("visual-original")
        .join("window-resizes.jsonl");

    let original_member = fs::read(&member).expect("member");

    fs::write(&member, b"{}\n").expect("tamper member");

    assert_rejected(&mut attached, second_id);

    fs::write(&member, original_member).expect("restore member");

    let recording = attached.wait_finished(FINISH).expect("restored capture");
    assert_eq!(recording.capture.source.session_id, second_id);
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn a_copied_live_sidecar_authorizes_nothing_for_another_session() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(cargo_bin("termtrace"), temporary.path());
    let script = "echo running; while :; do sleep 0.05; done";
    let mut first = terminal
        .start_original(&spec(script), &AtomicBool::new(false))
        .expect("first");
    let mut second = terminal
        .start_original(&spec(script), &AtomicBool::new(false))
        .expect("second");

    let second_path = second.session().path().to_path_buf();
    let attached = terminal
        .attach_original(&second_path)
        .expect("attach the second session");

    let own = status_bytes(&second_path);

    replace_status(&second_path, &status_bytes(first.session().path()));

    assert!(attached.capture_status().is_err());
    assert!(terminal.attach_original(&second_path).is_err());

    replace_status(&second_path, &own);

    assert_eq!(
        attached.capture_status().expect("own status").state,
        OriginalCaptureState::Live
    );

    let first_recording = first
        .finish(StopRequest::default(), FINISH)
        .expect("first finishes");
    assert_eq!(
        first_recording.capture.source.session_id,
        first.session().session_id()
    );

    second
        .finish(StopRequest::default(), FINISH)
        .expect("second finishes");
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn an_owner_stops_a_hup_ignoring_application_when_its_supervisor_is_killed() {
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(cargo_bin("termtrace"), temporary.path());
    let mut original = terminal
        .start_original(
            &spec("trap '' HUP TERM; echo stubborn; while :; do sleep 0.05; done"),
            &AtomicBool::new(false),
        )
        .expect("start");

    let tree = supervisor_tree(&original);
    let supervisor = original.capture_status().expect("status").supervisor_pid.0;
    let SessionStatus::Live {
        pid: Some(application),
        ..
    } = original.session().status().expect("session status")
    else {
        panic!("the application is running");
    };
    let stat = fs::read_to_string(format!("/proc/{application}/stat")).expect("stat");
    let recorder: i32 = stat
        .rsplit_once(") ")
        .and_then(|(_, fields)| return fields.split(' ').nth(1))
        .and_then(|parent| return parent.parse().ok())
        .expect("recorder");

    // The supervisor and the recorder that would stop the application both die at once.
    for pid in [i32::try_from(supervisor).unwrap(), recorder] {
        kill(Pid::from_raw(pid), Signal::SIGKILL).expect("kill");
    }

    // The owner is alive and nobody calls force_close.
    assert_all_exited(&tree);

    let error = original
        .wait_finished(FINISH)
        .expect_err("an abandoned capture never publishes");
    assert_eq!(error.kind(), ErrorKind::OriginalCaptureFailed);
}

#[test]
#[ignore = "requires Ghostty, Xvfb, dbus-run-session and FFmpeg"]
fn a_supervisor_killed_during_startup_leaves_no_secret_behind() {
    let secret = "termtrace-secret-91d2ab";
    let temporary = TempDir::new().expect("temporary directory");
    let terminal = Terminal::new(cargo_bin("termtrace"), temporary.path());
    let mut launch = spec("sleep 30");
    launch.environment.set.insert(
        "SECRET_TOKEN".to_owned(),
        ByteString::from(secret.as_bytes()),
    );

    let (started, tree) = thread::scope(|scope| {
        let killer = scope.spawn(|| {
            let deadline = Instant::now() + Duration::from_secs(30);
            let status = loop {
                let found = fs::read_dir(temporary.path())
                    .expect("root")
                    .flatten()
                    .find(|entry| {
                        return entry
                            .file_name()
                            .to_string_lossy()
                            .ends_with(".original-capture.json");
                    });

                if let Some(entry) = found
                    && let Ok(bytes) = fs::read(entry.path())
                    && let Ok(status) = serde_json::from_slice::<serde_json::Value>(&bytes)
                {
                    break status;
                }

                assert!(Instant::now() < deadline, "capture never started acquiring");

                thread::sleep(Duration::from_millis(5));
            };

            thread::sleep(Duration::from_millis(300));

            let supervisor: u32 = status["supervisor_pid"].as_str().unwrap().parse().unwrap();
            let tree = descendants(supervisor);
            kill(
                Pid::from_raw(i32::try_from(supervisor).unwrap()),
                Signal::SIGKILL,
            )
            .expect("kill supervisor");

            return tree;
        });

        let started = terminal.start_original(&launch, &AtomicBool::new(false));

        return (started, killer.join().expect("killer"));
    });
    assert!(started.is_err(), "a killed supervisor never starts");

    assert_all_exited(&tree);

    for entry in walkdir(temporary.path()) {
        let bytes = fs::read(&entry).unwrap_or_default();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(secret),
            "secret written to {}",
            entry.display()
        );
        assert!(
            !entry.to_string_lossy().contains(".original-environment"),
            "environment staging at {}",
            entry.display()
        );
    }
}
