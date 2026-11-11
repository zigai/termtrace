use super::*;
use termtrace_evidence::event_log::{EventLogWriter, SessionClock};
use termtrace_model::{
    execution::event::{Event, EventBody, EventLogHeader},
    value::ByteString,
};

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

fn set_output_burst_timestamps(path: &Path) {
    let log = EventLog::read(path).expect("recorded output burst");
    let mut bytes = serde_json::to_vec(&log.header).expect("header JSON");
    let mut previous_hash = blake3::hash(&bytes);
    bytes.push(b'\n');

    for mut record in log.events {
        record.monotonic_ns = (record.seq + 1) * 10_000;
        record.prev_hash = previous_hash.to_hex().to_string();

        let body = EventBody {
            seq: record.seq,
            monotonic_ns: record.monotonic_ns,
            observed_monotonic_ns: record.observed_monotonic_ns,
            event: &record.event,
        };

        let mut hasher = blake3::Hasher::new();
        hasher.update(previous_hash.as_bytes());
        serde_json::to_writer(&mut hasher, &body).expect("event body JSON");
        previous_hash = hasher.finalize();
        record.hash = previous_hash.to_hex().to_string();
        serde_json::to_writer(&mut bytes, &record).expect("event record JSON");
        bytes.push(b'\n');
    }

    fs_err::write(path, bytes).expect("timed output burst");
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

        write(&memory, &log, range, false, StateSelection::All).expect("memory index");
        write_source_index(
            &stream,
            &IndexSource::File(&event_path, &summary),
            range,
            false,
            StateSelection::All,
            None,
            TEST_CHECKPOINT_SPACING,
        )
        .expect("stream index");

        for name in [
            "state-index.jsonl",
            "control-index.jsonl.zst",
            "control-source-index.jsonl.zst",
            "checkpoints.jsonl.zst",
            "checkpoints.bin",
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
        TEST_CHECKPOINT_SPACING,
    )
    .is_err());
}

#[test]
#[ignore = "manual million-event bounded-memory storage/index stress"]
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
        TEST_CHECKPOINT_SPACING,
    )
    .expect("streamed million-event index");

    assert_eq!(retained, 1);
    assert_eq!(artifacts.final_state_id, 0);
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

    drop(writer);

    set_output_burst_timestamps(&event_path);

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
        TEST_CHECKPOINT_SPACING,
    )
    .expect("full index");

    let (video_artifacts, video_retained) = write_source_index(
        &video_dir,
        &IndexSource::File(&event_path, &summary),
        None,
        false,
        StateSelection::Presented,
        Some(1_000_000_000 / 60), // 60 FPS
        TEST_CHECKPOINT_SPACING,
    )
    .expect("video stride index");

    // A presented index lists the initial state and the end of every output event.
    assert_eq!(full_retained, 51);

    // Video stride index collapsed the flutter within the frame into a compact settled state
    assert_eq!(video_retained, 2);
    assert_eq!(
        video_artifacts.final_state_hash,
        full_artifacts.final_state_hash
    );
    assert_eq!(video_artifacts.final_text, full_artifacts.final_text);

    let video_index_content =
        fs_err::read_to_string(video_dir.join("state-index.jsonl")).expect("state index");

    let entries: Vec<StateIndexEntry> = video_index_content
        .lines()
        .map(|line| return serde_json::from_str(line).expect("entry"))
        .collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].state_id, 0);
    assert_eq!(entries[1].state_id, video_artifacts.final_state_id);
    assert_eq!(entries[1].state_hash, video_artifacts.final_state_hash);

    let manifest_bytes = fs_err::read(video_dir.join("manifest.json")).expect("manifest");
    let manifest: ReplayManifest = serde_json::from_slice(&manifest_bytes).expect("manifest json");
    assert_eq!(manifest.state_count, 2);
    assert_eq!(manifest.state_selection, StateSelection::Presented);
    assert_eq!(
        manifest.final_retained_state_hash,
        Some(video_artifacts.final_state_hash)
    );
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

    set_output_burst_timestamps(&event_path);

    let outcome = index_session_with_options(
        &session,
        IndexOptions {
            live_snapshot: true,
            ..IndexOptions::for_video(60)
        },
    )
    .expect("index session with video options");
    assert_eq!(outcome.state_selection, StateSelection::All);
    assert_eq!(outcome.state_count, 2);

    // Sampling only thins the listing; listed states keep the positions every seek uses.
    let listed: Vec<StateIndexEntry> = fs_err::read_to_string(outcome.directory.join(STATE_INDEX))
        .expect("state index")
        .lines()
        .map(|line| return serde_json::from_str(line).expect("entry"))
        .collect();
    assert_eq!(
        listed.last().map(|entry| return entry.state_id),
        Some(outcome.final_state_id)
    );

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
fn high_resolution_canvas_video_stride_captures_only_stride_states() {
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

    // Wide rows make every captured state expensive, so this bounds the capture cost.
    for i in 0..200 {
        writer
            .append(Event::Output {
                data: ByteString::from(
                    format!(
                        "4K high resolution canvas flutter line {i}: {}\r\n",
                        "=".repeat(120)
                    )
                    .into_bytes(),
                ),
            })
            .expect("output");
    }

    drop(writer);

    set_output_burst_timestamps(&event_path);

    let outcome = index_session_with_options(
        &session,
        IndexOptions {
            live_snapshot: true,
            ..IndexOptions::for_video(60)
        },
    )
    .expect("index high resolution canvas with video stride");
    assert_eq!(outcome.state_selection, StateSelection::All);
    assert_eq!(outcome.state_count, 2);
}

#[test]
fn seekable_members_read_like_plain_files_from_any_offset() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let path = temporary.path().join("member.zst");
    let plain = (0..100_000_u32)
        .flat_map(|line| return format!("{{\"line\":{line},\"pad\":\"{line:x}\"}}\n").into_bytes())
        .collect::<Vec<_>>();
    assert!(
        plain.len() > 2 * (1 << 20),
        "the member must span several frames"
    );

    let mut writer = seekable::SeekableWriter::create(&path).expect("seekable writer");
    for chunk in plain.chunks(4093) {
        writer.write_all(chunk).expect("seekable write");
    }

    writer.finish().expect("seekable finish");

    let length = u64::try_from(plain.len()).expect("length");
    assert_eq!(seekable_len(&path).expect("seekable length"), length);

    for offset in [
        0,
        1,
        4095,
        (1 << 20) - 1,
        1 << 20,
        (1 << 20) + 7,
        2 << 20,
        length - 3,
        length,
    ] {
        let mut read = Vec::new();
        open_seekable(&path, offset)
            .expect("seekable reader")
            .read_to_end(&mut read)
            .expect("seekable read");
        assert_eq!(
            read,
            plain[usize::try_from(offset).expect("offset")..],
            "offset {offset}"
        );
    }

    assert!(open_seekable(&path, length + 1).is_err());

    let empty = temporary.path().join("empty.zst");
    seekable::SeekableWriter::create(&empty)
        .expect("empty writer")
        .finish()
        .expect("empty finish");

    let mut read = Vec::new();
    open_seekable(&empty, 0)
        .expect("empty reader")
        .read_to_end(&mut read)
        .expect("empty read");
    assert!(read.is_empty());
}

fn recorded_fixture(events: Vec<Event>) -> (tempfile::TempDir, EventLog) {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let session = temporary.path().join("session");
    create_private_directory(&session).expect("private session");

    let path = session.join("events.jsonl");
    let mut writer = writer(&path);
    for event in events {
        writer.append(event).expect("canonical event");
    }

    drop(writer);

    let log = EventLog::read(&path).expect("canonical log");

    return (temporary, log);
}

fn catalog_file_index(session: &Path, log: &EventLog, sampling_stride_ns: Option<u64>) {
    let directory = session.join(layout::DERIVED);
    create_private_directory(&directory).expect("derived directory");
    let path = session.join("events.jsonl");
    let summary = EventLogSummary::read(&path).expect("completed summary");
    write_source_index(
        &directory,
        &IndexSource::File(&path, &summary),
        None,
        true,
        StateSelection::All,
        sampling_stride_ns,
        TEST_CHECKPOINT_SPACING,
    )
    .expect("file index");

    append_artifact_root(
        session,
        &directory,
        "semantic_index",
        &fs_err::read(directory.join(INDEX_MANIFEST)).expect("manifest"),
        &log.header.session_manifest_hash,
        log.events.last().map(|record| return record.hash.as_str()),
        None,
    )
    .expect("catalog file index");
}

#[test]
fn old_v7_manifest_without_final_state_id_falls_back_before_current_decode() {
    use super::super::{inspect_many, StateSelector};
    let (temporary, log) = recorded_fixture(vec![Event::Output {
        data: ByteString::from(b"legacy".as_slice()),
    }]);
    let session = temporary.path().join("session");
    let selectors = [StateSelector::Final, StateSelector::StateId(0)];
    let expected = inspect_many(&log, &selectors).expect("canonical batch");
    catalog_file_index(&session, &log, None);
    assert!(DerivedIndex::open(&log).expect("current index").is_some());

    let path = session.join(layout::DERIVED).join(INDEX_MANIFEST);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs_err::read(&path).expect("manifest")).expect("JSON");
    manifest["version"] = serde_json::json!(7);
    assert!(manifest
        .as_object_mut()
        .expect("object")
        .remove("final_state_id")
        .is_some());

    let bytes = serde_json::to_vec(&manifest).expect("legacy JSON");
    assert!(
        serde_json::from_slice::<ReplayManifest>(&bytes).is_err(),
        "legacy fixture cannot decode as the current schema"
    );

    fs_err::write(&path, bytes).expect("legacy manifest");

    assert!(DerivedIndex::open(&log)
        .expect("ignore old schema before catalog verification")
        .is_none());
    assert_eq!(
        inspect_many(&log, &selectors).expect("legacy memory fallback"),
        expected
    );
}

#[test]
fn file_checkpoint_replays_pending_resize_intent() {
    use super::super::{inspect_many, StateSelector};
    use termtrace_model::execution::event::OperationStatus;
    let operation_id = Uuid::now_v7();
    let (temporary, log) = recorded_fixture(vec![
        Event::ResizeIntent {
            operation_id,
            cols: 40,
            rows: 10,
            pixel_width: 360,
            pixel_height: 180,
        },
        Event::Output {
            data: ByteString::from(vec![b'x'; 96]),
        },
        Event::ResizeResult {
            operation_id,
            status: OperationStatus::Applied,
            error_code: None,
        },
        Event::Output {
            data: ByteString::from(b"after resize".as_slice()),
        },
    ]);
    let session = temporary.path().join("session");
    let selectors = [
        StateSelector::StateId(96),
        StateSelector::StateId(97),
        StateSelector::Final,
    ];
    let expected = inspect_many(&log, &selectors).expect("canonical batch");
    catalog_file_index(&session, &log, None);
    let index = DerivedIndex::open(&log)
        .expect("trusted lookup")
        .expect("file index");

    let checkpoints = index.checkpoints().expect("checkpoints");
    let checkpoint = checkpoints
        .iter()
        .rfind(|checkpoint| return checkpoint.event_seq == Some(1))
        .expect("checkpoint between intent and result");

    let seed = index.seed(checkpoint).expect("file checkpoint seed");
    assert_eq!(
        seed.source_position
            .as_ref()
            .expect("file seek position")
            .next_event_seq,
        0,
        "resume must reread the pending intent, not start at its intervening output"
    );

    let actual = inspect_many(&log, &selectors).expect("indexed batch");
    let canonical = super::super::replay(&log).expect("canonical replay");
    let mut resumed = Vec::new();
    let start = canonical
        .states
        .iter()
        .position(|record| return record.state_id == seed.record.state_id)
        .expect("checkpoint boundary");

    super::super::replay_with_seed_and_limits(
        &log,
        None,
        seed,
        super::super::DEFAULT_REPLAY_LIMITS,
        |record, _, _| {
            resumed.push(record);
            return Ok(());
        },
    )
    .expect("restored checkpoint replay");

    assert_eq!(resumed, canonical.states[start..]);
    assert_eq!(actual, expected);
    assert_eq!(actual[1].state.cols, 40);
    assert_eq!(actual[1].state.rows, 10);
}

#[test]
fn sampled_index_batches_find_latest_recurring_initial_hash() {
    use super::super::{inspect_many, StateSelector};
    let (temporary, log) = recorded_fixture(vec![
        Event::Output {
            data: ByteString::from(b"x".as_slice()),
        },
        Event::Output {
            data: ByteString::from(b"\x1b[2J\x1b[H".as_slice()),
        },
        Event::Output {
            data: ByteString::from(b"tail".as_slice()),
        },
    ]);
    let session = temporary.path().join("session");
    let initial = inspect_many(&log, &[StateSelector::StateId(0)])
        .expect("initial state")
        .remove(0);

    let selectors = [
        StateSelector::StateHash(initial.record.state_hash.clone()),
        StateSelector::Final,
        StateSelector::StateHash(initial.record.state_hash),
    ];
    let expected = inspect_many(&log, &selectors).expect("canonical batch");
    assert!(
        expected[0].record.state_id > 0,
        "initial content must recur later"
    );
    assert_eq!(
        expected[0].record.event_seq,
        Some(1),
        "the cleared screen recurs after the first output"
    );

    catalog_file_index(&session, &log, Some(u64::MAX));
    let index = DerivedIndex::open(&log)
        .expect("trusted lookup")
        .expect("sampled index");
    assert!(!index.lists_every_state());
    assert_eq!(index.listed_states().expect("sampled listing").len(), 2);
    assert_eq!(
        inspect_many(&log, &selectors).expect("sampled memory batch"),
        expected
    );
}
