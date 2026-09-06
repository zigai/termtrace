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
        assert!(
            !fs_err::read_to_string(stream.join("final.txt"))
                .expect("final text")
                .contains("LATER")
        );
    }
    let invalid = session.join("invalid");
    create_private_directory(&invalid).expect("invalid directory");
    let mut wrong = summary;
    wrong.terminal.as_mut().expect("terminal").hash = "0".repeat(64);
    assert!(
        write_source_index(
            &invalid,
            &IndexSource::File(&event_path, &wrong),
            None,
            false,
            StateSelection::All
        )
        .is_err()
    );
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
    )
    .expect("streamed million-event index");
    assert_eq!(retained, 1);
    assert_eq!(artifacts.state_count, 1);
}
