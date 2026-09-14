use super::*;
use termtrace_evidence::event_log::{EventLogWriter, SessionClock};
use termtrace_model::execution::event::{Event, EventLogHeader};

fn writer(path: &Path) -> EventLogWriter {
    let clock = SessionClock::now().expect("clock");
    let header = EventLogHeader::new(
        Uuid::now_v7(),
        "0".repeat(64),
        clock.started_unix_ns(),
        80,
        24,
        800,
        480,
        "xterm-ghostty",
    );

    return EventLogWriter::create(path, &header, clock).expect("writer");
}

#[test]
fn file_index_matches_memory_and_excludes_later_appends() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let session = temporary.path().join("session");
    create_private_directory(&session).expect("private directory");

    let event_path = session.join("events.jsonl");

    let mut writer = writer(&event_path);
    for bytes in [
        b"hello\x1b[".as_slice(),
        b"31mred\x1b]2;title".as_slice(),
        b"\x1b\\\xe2".as_slice(),
        b"\x82\xac\x1b[0m".as_slice(),
    ] {
        writer
            .append(Event::Output {
                data: ByteString::from(bytes),
            })
            .expect("output");
    }

    let log = EventLog::read(&event_path).expect("memory source");
    let summary = EventLogSummary::snapshot(&event_path).expect("snapshot");

    writer
        .append(Event::Output {
            data: ByteString::from(b"LATER".as_slice()),
        })
        .expect("later output");

    for range in [
        None,
        Some(SourceRange {
            start_monotonic_ns: log.events[1].monotonic_ns,
            end_monotonic_ns: log.events[3].monotonic_ns + 1,
        }),
    ] {
        let memory = session.join(format!("memory-{}", range.is_some()));
        let stream = session.join(format!("stream-{}", range.is_some()));
        create_private_directory(&memory).expect("memory directory");

        create_private_directory(&stream).expect("stream directory");

        write_index(&memory, &log, range, false, StateSelection::All).expect("memory index");
        write_source_index(
            &stream,
            &IndexSource::File(&event_path, &summary),
            range,
            false,
            StateSelection::All,
            None,
        )
        .expect("stream index");

        for name in [
            "state-index.jsonl",
            "control-index.jsonl",
            "control-source-index.jsonl",
            "states.jsonl.zst",
            "checkpoints.jsonl.zst",
            "final.txt",
            "manifest.json",
        ] {
            assert_eq!(
                fs_err::read(memory.join(name)).expect("memory artifact"),
                fs_err::read(stream.join(name)).expect("stream artifact"),
                "{name}"
            );
        }

        assert!(!fs_err::read_to_string(stream.join("final.txt"))
            .expect("final text")
            .contains("LATER"));
    }

    let invalid = session.join("invalid");
    create_private_directory(&invalid).expect("invalid directory");

    let mut wrong = summary;
    wrong.terminal.as_mut().expect("terminal").hash = "0".repeat(64);

    assert!(write_source_index(
        &invalid,
        &IndexSource::File(&event_path, &wrong),
        None,
        false,
        StateSelection::All,
        None,
    )
    .is_err());
}

#[test]
#[ignore = "million-event bounded-memory storage/index stress"]
fn streaming_index_exceeds_materializing_reader_event_limit() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let session = temporary.path().join("session");
    create_private_directory(&session).expect("private directory");

    let event_path = session.join("events.jsonl");

    let mut writer = writer(&event_path);
    for _ in 0..1_000_001 {
        writer
            .append(Event::HostError {
                code: "stress".to_owned(),
                message: String::new(),
            })
            .expect("marker");
    }

    drop(writer);

    assert!(
        EventLog::read(&event_path).is_err(),
        "materializing reader retains its allocation ceiling"
    );

    let summary = EventLogSummary::read(&event_path).expect("unbounded lifetime summary");
    assert_eq!(summary.event_count, 1_000_001);

    let derived = session.join("derived");
    create_private_directory(&derived).expect("derived directory");
    let (artifacts, retained) = write_source_index(
        &derived,
        &IndexSource::File(&event_path, &summary),
        None,
        false,
        StateSelection::All,
        None,
    )
    .expect("streamed million-event index");
    assert_eq!(retained, 1);
    assert_eq!(artifacts.state_count, 1);
}

#[test]
fn video_stride_samples_intermediate_flutter_into_compact_index() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let session = temporary.path().join("session");
    create_private_directory(&session).expect("private directory");

    let event_path = session.join("events.jsonl");
    let mut writer = writer(&event_path);

    // Output 50 intermediate byte updates within 10ms (less than one 60fps frame).
    for i in 0..50 {
        writer
            .append(Event::Output {
                data: ByteString::from(format!("flutter {i}\r\n").into_bytes()),
            })
            .expect("output");
    }

    let summary = EventLogSummary::snapshot(&event_path).expect("snapshot");
    let full_dir = session.join("full");
    let video_dir = session.join("video");
    create_private_directory(&full_dir).expect("full directory");
    create_private_directory(&video_dir).expect("video directory");

    let (full_artifacts, full_retained) = write_source_index(
        &full_dir,
        &IndexSource::File(&event_path, &summary),
        None,
        false,
        StateSelection::Presented,
        None,
    )
    .expect("full index");

    let (video_artifacts, video_retained) = write_source_index(
        &video_dir,
        &IndexSource::File(&event_path, &summary),
        None,
        false,
        StateSelection::Presented,
        Some(1_000_000_000 / 60), // 60 FPS
    )
    .expect("video stride index");

    // Full index recorded every single intermediate byte delta (591 states!)
    assert_eq!(full_retained, 591);

    // Video stride index collapsed the flutter within the frame into a compact settled state
    assert_eq!(video_retained, 2);
    assert_eq!(video_artifacts.final_state_hash, full_artifacts.final_state_hash);
    assert_eq!(video_artifacts.final_text, full_artifacts.final_text);

    let video_index_content = fs_err::read_to_string(video_dir.join("state-index.jsonl")).expect("state index");
    let entries: Vec<StateIndexEntry> = video_index_content
        .lines()
        .map(|line| serde_json::from_str(line).expect("entry"))
        .collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].state_id, 0);
    assert_eq!(entries[1].state_id, 1);
    assert_eq!(entries[1].state_hash, video_artifacts.final_state_hash);

    let manifest_bytes = fs_err::read(video_dir.join("manifest.json")).expect("manifest");
    let manifest: ReplayManifest = serde_json::from_slice(&manifest_bytes).expect("manifest json");
    assert_eq!(manifest.state_count, 2);
    assert_eq!(manifest.state_selection, StateSelection::Presented);
    assert_eq!(manifest.final_retained_state_hash, Some(video_artifacts.final_state_hash));
}

#[test]
fn index_session_with_video_options_samples_cleanly() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let session = temporary.path().join("session");
    create_private_directory(&session).expect("private directory");

    let event_path = session.join("events.jsonl");
    let mut writer = writer(&event_path);
    for i in 0..30 {
        writer
            .append(Event::Output {
                data: ByteString::from(format!("frame {i}\n").into_bytes()),
            })
            .expect("output");
    }
    drop(writer);

    let outcome = index_session_with_options(
        &session,
        IndexOptions {
            live_snapshot: true,
            ..IndexOptions::for_video(60)
        },
    )
    .expect("index session with video options");

    assert_eq!(outcome.state_selection, StateSelection::Presented);
    assert_eq!(outcome.state_count, 2);

    // Also test write_index_with_stride
    let memory_dir = session.join("memory_stride");
    create_private_directory(&memory_dir).expect("memory directory");
    let log = EventLog::read(&event_path).expect("read log");
    let (_, memory_retained) = write_index_with_stride(
        &memory_dir,
        &log,
        None,
        false,
        StateSelection::Presented,
        Some(1_000_000_000 / 60),
    )
    .expect("write_index_with_stride");
    assert_eq!(memory_retained, 2);
}

#[test]
fn high_resolution_canvas_video_stride_completes_in_seconds() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let session = temporary.path().join("session_4k");
    create_private_directory(&session).expect("private directory");

    let event_path = session.join("events.jsonl");
    let clock = SessionClock::now().expect("clock");
    let header = EventLogHeader::new(
        Uuid::now_v7(),
        "0".repeat(64),
        clock.started_unix_ns(),
        300,  // 300 cols (4K canvas)
        100,  // 100 rows
        3840, // 3840px
        2160, // 2160px
        "xterm-ghostty",
    );
    let mut writer = EventLogWriter::create(&event_path, &header, clock).expect("writer");

    // Write 200 high-resolution lines of terminal flutter output
    for i in 0..200 {
        writer
            .append(Event::Output {
                data: ByteString::from(format!("4K high resolution canvas flutter line {i}: {}\r\n", "=".repeat(120)).into_bytes()),
            })
            .expect("output");
    }
    drop(writer);

    let start = std::time::Instant::now();
    let outcome = index_session_with_options(
        &session,
        IndexOptions {
            live_snapshot: true,
            ..IndexOptions::for_video(60)
        },
    )
    .expect("index high resolution canvas with video stride");
    let elapsed = start.elapsed();

    println!("4K high resolution 300x100 indexing completed in: {elapsed:?}");
    assert!(elapsed < std::time::Duration::from_secs(5), "indexing 4K session with video stride took too long: {elapsed:?}");
    assert_eq!(outcome.state_selection, StateSelection::Presented);
    assert_eq!(outcome.state_count, 2);
}
