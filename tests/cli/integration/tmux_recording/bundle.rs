use super::*;
use nix::{
    sys::signal::{kill, Signal},
    unistd::Pid,
};
use std::env;
use termtrace_export::tmux;
use termtrace_tmux::{
    artifact::{
        TmuxCatalogSummary, TmuxCompositeTrack, TmuxEvent, TmuxJournalBoundary, TmuxPaneAddress,
        TmuxPaneTrack, TmuxRecordingCoverage, TmuxRecordingLifecycle, TmuxRecordingManifest,
        TmuxRecordingState, TmuxSessionLink, TmuxSocket, TMUX_RECORDING_FORMAT,
        TMUX_RECORDING_VERSION,
    },
    journal,
    store::RecordingStore,
};

#[test]
fn tmux_bundle_contains_the_aggregate_manifest_and_every_canonical_track() {
    let temporary = TempDir::new().expect("temporary directory");
    fs::set_permissions(temporary.path(), Permissions::from_mode(0o700))
        .expect("secure aggregate root");
    let composite = temporary.path().join("composite");
    let pane = temporary.path().join("pane");
    let previous = temporary.path().join("previous-pane");

    for (session, output) in [
        (&composite, "COMPOSITE"),
        (&pane, "PANE"),
        (&previous, "PREVIOUS"),
    ] {
        let mut record = Command::cargo_bin("termtrace").expect("binary should build");
        record
            .args(["record", "--no-input", "--out"])
            .arg(session)
            .args(["--", "/bin/sh", "-c", &format!("printf {output}")])
            .assert()
            .success();
    }

    let state = temporary.path().join("state");
    fs::create_dir_all(&state).expect("state root");
    fs::set_permissions(&state, Permissions::from_mode(0o700)).expect("private state root");
    let store = RecordingStore::create(&state, "work").expect("aggregate directory");
    let aggregate = store.directory().to_path_buf();
    let composite_manifest = read_manifest(&composite).expect("composite manifest");
    let manifest = TmuxRecordingManifest {
        format: TMUX_RECORDING_FORMAT.to_owned(),
        version: TMUX_RECORDING_VERSION,
        recording_id: uuid::Uuid::now_v7(),
        name: "work".to_owned(),
        created_unix_ns: 1,
        coverage: TmuxRecordingCoverage::Warn,
        tmux_executable: ByteString::from(b"/usr/bin/tmux".as_slice()),
        socket: TmuxSocket::Default,
        state: TmuxRecordingState {
            lifecycle: TmuxRecordingLifecycle::Live,
            composite_track: TmuxCompositeTrack {
                alias: "tmux-work-composite".to_owned(),
                link: Some(TmuxSessionLink {
                    session_id: composite_manifest.session_id,
                    session: ByteString::from(composite.as_os_str().as_bytes()),
                }),
            },
            server: None,
            panes: Vec::new(),
            pane_tracks: BTreeMap::from([(
                "%1".to_owned(),
                ByteString::from(pane.as_os_str().as_bytes()),
            )]),
            coverage_gaps: Vec::new(),
            updated_unix_ns: 2,
        },
        lifecycle_journal: TmuxJournalBoundary::empty(),
        catalog: TmuxCatalogSummary::default(),
    };

    let linked = [&previous, &pane]
        .into_iter()
        .map(|path| {
            return TmuxEvent::PaneTrackLinked {
                track: TmuxPaneTrack {
                    pane_id: "%1".to_owned(),
                    session_id: read_manifest(path).expect("pane manifest").session_id,
                    session: ByteString::from(path.as_os_str().as_bytes()),
                    linked_unix_ns: 1,
                },
            };
        })
        .collect();

    store
        .initialize(manifest, linked, 1)
        .expect("linked tracks");

    for sequence in 1..=260_u64 {
        let mut locked = store.lock().expect("aggregate writer");
        let state = locked.manifest().state.clone();
        locked
            .commit(
                state,
                vec![TmuxEvent::LayoutChanged {
                    pane: TmuxPaneAddress {
                        session_id: "$0".to_owned(),
                        window_id: "@0".to_owned(),
                        pane_id: "%1".to_owned(),
                    },
                    layout: format!("layout {sequence}"),
                }],
                sequence,
            )
            .expect("append lifecycle");
    }

    let snapshot: serde_json::Value =
        serde_json::from_slice(&fs::read(store.manifest_path()).expect("snapshot"))
            .expect("snapshot JSON");
    assert!(
        snapshot.get("events").is_none() && snapshot.get("track_history").is_none(),
        "history stays in the journal, never in the snapshot"
    );

    let boundary = store.read().expect("snapshot").lifecycle_journal;
    let lifecycle = aggregate.join(journal::JOURNAL_NAME);
    let mut count = 0;
    journal::visit(&lifecycle, &boundary, |_| {
        count += 1;

        return Ok(());
    })
    .expect("complete lifecycle history");

    assert_eq!(count, 262);

    {
        use std::io::Write as _;

        fs::OpenOptions::new()
            .append(true)
            .open(&lifecycle)
            .expect("journal")
            .write_all(b"interrupted uncommitted suffix")
            .expect("partial write");
    }

    {
        let mut locked = store.lock().expect("aggregate writer");
        let state = locked.manifest().state.clone();
        locked
            .commit(state, Vec::new(), 300)
            .expect("recover publication suffix");
    }

    assert_eq!(
        fs::metadata(&lifecycle).expect("journal metadata").len(),
        boundary.bytes
    );

    let mut page = Command::cargo_bin("termtrace").expect("binary should build");
    let output = page
        .args(["tmux", "events", "work", "--output-root"])
        .arg(&state)
        .args(["--from-sequence", "2", "--limit", "3"])
        .assert()
        .success();

    let page: serde_json::Value =
        serde_json::from_slice(&output.get_output().stdout).expect("event page");
    assert_eq!(page["events"][0]["sequence"], 2);
    assert_eq!(page["events"].as_array().expect("events").len(), 3);
    assert_eq!(page["next_sequence"], 5);

    let rejected = temporary.path().join("live-aggregate-bundle");
    let mut live = Command::cargo_bin("termtrace").expect("binary should build");
    live.args(["tmux", "bundle", "work", "--output-root"])
        .arg(&state)
        .arg("--out")
        .arg(&rejected)
        .assert()
        .failure()
        .stderr(predicate::str::contains("must be completed before export"));

    assert!(!rejected.exists());

    {
        let mut locked = store.lock().expect("aggregate writer");
        let mut state = locked.manifest().state.clone();
        state.lifecycle = TmuxRecordingLifecycle::Completed;
        locked
            .commit(state, vec![TmuxEvent::RecordingCompleted], 301)
            .expect("completed aggregate manifest");
    }

    let bundle = temporary.path().join("aggregate-bundle");
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    let output = command
        .args(["tmux", "bundle", "work", "--output-root"])
        .arg(&state)
        .arg("--out")
        .arg(&bundle)
        .assert()
        .success();

    let response = compact_json_line(&output.get_output().stdout);
    assert_eq!(response["recording_name"], "work");
    assert_eq!(response["track_count"], 3);
    assert!(bundle.join("aggregate-recording-manifest.json").is_file());

    let bundled: TmuxStreamingBundleManifest = serde_json::from_slice(
        &fs::read(bundle.join("manifest.json")).expect("tmux bundle manifest"),
    )
    .expect("tmux bundle manifest JSON");
    assert_eq!(bundled.track_count, 3);
    let inventory = fs::read_to_string(bundle.join("tracks.jsonl")).expect("track inventory");
    let tracks = inventory
        .lines()
        .map(|line| return serde_json::from_str::<TmuxBundleTrack>(line).expect("track row"))
        .collect::<Vec<_>>();
    assert!(
        tracks
            .iter()
            .any(|track| return track.canonical_session.as_bytes()
                == previous.as_os_str().as_bytes()),
        "respawned pane history must remain in complete exports"
    );
    assert_eq!(tracks[0].role, "composite");
    assert_eq!(tracks[1].pane_id.as_deref(), Some("%1"));

    for track in &tracks {
        assert!(
            bundle
                .join(&track.directory)
                .join("canonical/completion.json")
                .is_file(),
            "missing canonical evidence for {}",
            track.directory
        );
    }

    assert!(bundle.join("aggregate-lifecycle.jsonl").is_file());

    tmux::streaming::verify(&bundle, aggregate.join("manifest.json"))
        .expect("journal-bound bundle");

    fs::write(bundle.join("extra"), b"extra").expect("extra file");

    assert!(tmux::streaming::verify(&bundle, aggregate.join("manifest.json")).is_err());

    fs::remove_file(bundle.join("extra")).expect("remove extra file");
    fs::write(
        bundle.join("tracks.jsonl"),
        format!("{inventory}{inventory}"),
    )
    .expect("duplicate inventory");

    assert!(tmux::streaming::verify(&bundle, aggregate.join("manifest.json")).is_err());

    fs::write(bundle.join("tracks.jsonl"), inventory).expect("restore inventory");
    fs::write(bundle.join("aggregate-lifecycle.jsonl"), b"corrupt\n").expect("tamper journal");

    assert!(tmux::streaming::verify(&bundle, aggregate.join("manifest.json")).is_err());
}

#[test]
fn tmux_aggregate_manifest_links_composite_and_pane_tracks() {
    let temporary = TempDir::new().expect("temporary directory");
    fs::set_permissions(temporary.path(), Permissions::from_mode(0o700))
        .expect("secure aggregate root");

    let binary = tmux_binary();
    let socket = format!("termtrace-aggregate-test-{}", uuid::Uuid::now_v7());
    let root = temporary.path().to_str().expect("UTF-8 temporary path");
    let tmux = binary.to_str().expect("UTF-8 tmux path");
    let output = Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args([
            "tmux",
            "record",
            "work",
            "--new",
            "-L",
            socket.as_str(),
            "--tmux",
            tmux,
            "--output-root",
            root,
            "--ghostty-profile",
            "off",
            "--",
            tmux,
            "new-session",
            "-s",
            "aggregate",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let _server = PrivateTmux {
        binary: binary.clone(),
        socket: socket.clone(),
        session: "aggregate".to_owned(),
        tmux_tmpdir: env::var_os("TMUX_TMPDIR").map(PathBuf::from),
    };

    let started: serde_json::Value =
        serde_json::from_slice(&output).expect("aggregate start response");
    assert_eq!(started["name"], "work");
    assert!(started["composite_track"]["link"]["session"].is_string());

    let manifest = temporary.path().join("tmux-recordings/work/manifest.json");
    let deadline = Instant::now() + Duration::from_secs(5);
    let linked = loop {
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest).expect("read aggregate manifest"))
                .expect("aggregate manifest JSON");
        if value["composite_track"]["link"]["session_id"].is_string()
            && value["pane_tracks"]
                .as_object()
                .is_some_and(|tracks| return !tracks.is_empty())
        {
            break value;
        }

        assert!(
            Instant::now() < deadline,
            "aggregate manifest did not link both track layers"
        );

        thread::sleep(Duration::from_millis(20));
    };

    assert_eq!(linked["format"], "termtrace-tmux-recording");

    let inventory = tmux_json(&["tmux", "tracks", "work", "--output-root", root]);
    let history = inventory["entries"]
        .as_array()
        .expect("incarnation history");
    assert!(!history.is_empty());

    for entry in history {
        let track = &entry["track"];

        assert_eq!(
            linked["pane_tracks"][track["pane_id"].as_str().expect("pane ID")],
            track["session"]
        );
    }

    assert_eq!(linked["coverage"], "warn");

    let deadline = Instant::now() + Duration::from_secs(5);
    let observer = loop {
        let status = tmux_json(&[
            "tmux",
            "status",
            "--json",
            "--tmux",
            tmux,
            "-L",
            socket.as_str(),
        ]);

        let events = tmux_events(root, "work");
        if let Some(pid) = status["observers"]["supervisor"].as_i64()
            && events
                .iter()
                .any(|event| return event["kind"] == "observer_started" && event["pid"] == pid)
        {
            break i32::try_from(pid).expect("observer PID");
        }

        assert!(Instant::now() < deadline, "observer did not start");

        thread::sleep(Duration::from_millis(20));
    };

    kill(Pid::from_raw(observer), Signal::SIGKILL).expect("kill isolated observer");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let events = tmux_events(root, "work");
        let restarted = events
            .iter()
            .filter(|event| return event["kind"] == "observer_started")
            .count()
            >= 2
            && events.iter().any(|event| {
                return event["kind"] == "coverage_gap_resolved"
                    && event["gap"]["kind"] == "observer_start";
            });
        if restarted {
            break;
        }

        assert!(
            Instant::now() < deadline,
            "observer did not restart with a durable coverage gap"
        );

        thread::sleep(Duration::from_millis(20));
    }

    assert!(tmux_events(root, "work")
        .iter()
        .any(|event| return event["kind"] == "pane_track_linked"));

    let status: serde_json::Value = serde_json::from_slice(
        &Command::cargo_bin("termtrace")
            .expect("binary should build")
            .args(["tmux", "status", "work", "--output-root", root])
            .assert()
            .success()
            .get_output()
            .stdout,
    )
    .expect("aggregate status response");
    assert_eq!(status["recording_id"], linked["recording_id"]);
    assert_eq!(
        status["composite_track"]["link"]["session_id"],
        linked["composite_track"]["link"]["session_id"]
    );
    assert_eq!(status["pane_tracks"], linked["pane_tracks"]);

    let bypass = String::from_utf8(trim_newline(
        StdCommand::new(&binary)
            .args([
                "-L",
                socket.as_str(),
                "new-window",
                "-d",
                "-P",
                "-F",
                "#{pane_id}",
                "/bin/sleep",
                "30",
            ])
            .output()
            .expect("create uncovered aggregate pane")
            .stdout,
    ))
    .expect("UTF-8 pane ID");

    let gap_deadline = Instant::now() + Duration::from_secs(5);
    let uncovered = loop {
        let refreshed: serde_json::Value = serde_json::from_slice(
            &Command::cargo_bin("termtrace")
                .expect("binary should build")
                .args(["tmux", "status", "work", "--output-root", root])
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .expect("aggregate status with uncovered pane");

        if let Some(gap) = refreshed["coverage_gaps"]
            .as_array()
            .and_then(|gaps| {
                return gaps.iter().find(|gap| {
                    return gap["kind"] == "uncovered_pane"
                        && gap["pane_id"] == bypass
                        && gap["resolved_unix_ns"].is_null();
                });
            })
            .cloned()
        {
            break gap;
        }

        assert!(
            Instant::now() < gap_deadline,
            "aggregate manifest did not retain the uncovered pane"
        );

        thread::sleep(Duration::from_millis(20));
    };

    assert!(uncovered["first_observed_unix_ns"].is_u64());
    assert!(uncovered["last_observed_unix_ns"].is_u64());

    let killed = StdCommand::new(&binary)
        .args(["-L", socket.as_str(), "kill-pane", "-t", bypass.as_str()])
        .status()
        .expect("kill uncovered aggregate pane");
    assert!(killed.success());

    let resolution_deadline = Instant::now() + Duration::from_secs(5);
    let resolved = loop {
        let refreshed: serde_json::Value = serde_json::from_slice(
            &Command::cargo_bin("termtrace")
                .expect("binary should build")
                .args(["tmux", "status", "work", "--output-root", root])
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .expect("aggregate status after gap resolution");

        let gap_page: serde_json::Value = serde_json::from_slice(
            &Command::cargo_bin("termtrace")
                .expect("binary should build")
                .args(["tmux", "gaps", "work", "--output-root", root])
                .assert()
                .success()
                .get_output()
                .stdout,
        )
        .expect("historical gap inventory");

        let historical = gap_page["entries"]
            .as_array()
            .and_then(|gaps| {
                return gaps.iter().find(|gap| {
                    return gap["gap"]["kind"] == "uncovered_pane"
                        && gap["gap"]["pane_id"] == bypass;
                });
            })
            .map(|entry| return &entry["gap"])
            .cloned()
            .expect("historical uncovered pane gap");
        if historical["resolved_unix_ns"].is_u64() {
            break (historical, refreshed);
        }

        assert!(
            Instant::now() < resolution_deadline,
            "aggregate gap did not acquire resolution evidence"
        );

        thread::sleep(Duration::from_millis(20));
    };

    assert_eq!(
        resolved.0["first_observed_unix_ns"],
        uncovered["first_observed_unix_ns"]
    );

    let initial_last_observed = uncovered["last_observed_unix_ns"]
        .as_u64()
        .expect("initial gap last observation");

    let final_last_observed = resolved.0["last_observed_unix_ns"]
        .as_u64()
        .expect("resolved gap last observation");
    let resolved_unix_ns = resolved.0["resolved_unix_ns"]
        .as_u64()
        .expect("gap resolution timestamp");

    assert!(final_last_observed >= initial_last_observed);
    assert!(resolved_unix_ns >= final_last_observed);

    let events = tmux_events(root, "work");
    assert!(["coverage_gap_opened", "coverage_gap_resolved"]
        .iter()
        .all(|kind| {
            return events.iter().any(|event| {
                return event["kind"] == *kind && event["gap"]["pane_id"] == bypass;
            });
        }));

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["tmux", "stop", "work", "--output-root", root, "--wait"])
        .assert()
        .success();

    let video = temporary.path().join("aggregate.mp4");
    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .env("PATH", "")
        .args([
            "tmux",
            "video",
            "work",
            "--output-root",
            root,
            "--renderer",
            "semantic",
            "--preset",
            "social",
            "-o",
        ])
        .arg(&video)
        .assert()
        .failure()
        .stderr(predicate::str::contains("Planning video from"))
        .stderr(predicate::str::contains(
            "required capability `semantic-video` is blocked",
        ))
        .stderr(predicate::str::contains(
            "semantic_video_dependency_missing",
        ));

    let stopped: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest).expect("read stopped aggregate manifest"))
            .expect("stopped aggregate manifest JSON");
    assert_eq!(stopped["lifecycle"], "completed");
}

#[allow(
    clippy::expect_used,
    reason = "tmux test helper stops when the CLI cannot report JSON"
)]
fn tmux_json(arguments: &[&str]) -> serde_json::Value {
    let output = Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(arguments)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    return serde_json::from_slice(&output).expect("tmux JSON response");
}

fn tmux_events(root: &str, name: &str) -> Vec<serde_json::Value> {
    let page = tmux_json(&[
        "tmux",
        "events",
        name,
        "--output-root",
        root,
        "--limit",
        "4096",
    ]);

    return page
        .get("events")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
}
