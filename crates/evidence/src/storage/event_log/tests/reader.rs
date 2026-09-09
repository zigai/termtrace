use crate::{
    event_log::{
        EventLog, EventLogCursor, EventLogSummary, EventLogWriter, OperationKind, PendingOperation,
        SessionClock,
    },
    Error,
};
use std::{fs::OpenOptions, io::Write, os::unix::fs::PermissionsExt as _};
use tempfile::TempDir;
use termtrace_model::execution::event::{
    Event, InputDescriptor, InputKind, InputSource, OperationStatus,
};
use termtrace_model::value::ByteString;
use uuid::Uuid;

#[test]
fn finite_snapshot_excludes_later_appends_and_partial_records() {
    let temporary = TempDir::new().expect("temporary directory");
    let path = temporary.path().join("session/events.jsonl");
    secure_directory(path.parent().expect("parent"));

    let clock = SessionClock::now().expect("clock");
    let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("writer");
    writer
        .append_at(
            10,
            Event::Output {
                data: ByteString::from(&b"before"[..]),
            },
        )
        .expect("first output");

    let mut snapshot = EventLogCursor::open_snapshot(&path).expect("snapshot");

    writer
        .append_at(
            20,
            Event::Output {
                data: ByteString::from(&b"after"[..]),
            },
        )
        .expect("later output");

    assert_eq!(
        snapshot
            .next_record()
            .expect("record")
            .expect("first")
            .monotonic_ns,
        10
    );
    assert!(snapshot.next_record().expect("snapshot end").is_none());
    assert!(snapshot
        .finish_snapshot()
        .expect("prefix verification")
        .is_empty());
    assert_eq!(
        EventLogSummary::snapshot(&path)
            .expect("new snapshot")
            .event_count,
        2
    );

    drop(writer);

    OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("append partial record")
        .write_all(b"{\"unfinished\":")
        .expect("partial bytes");

    assert_eq!(
        EventLogSummary::snapshot(&path)
            .expect("complete prefix")
            .event_count,
        2
    );
    assert!(EventLogSummary::read(&path).is_err());

    let page = EventLog::read_page(&path, 1, 1).expect("bounded prefix page");
    assert_eq!(page.len(), 1);
    assert_eq!(page.first().expect("paged event").monotonic_ns, 20);

    OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("terminate corrupt record")
        .write_all(b"\n")
        .expect("terminator");

    assert!(
        EventLog::read_page(&path, 0, 1).is_err(),
        "a valid selected page cannot hide later corruption"
    );
}

use super::{
    super::validation::{OWNER_DIRECTORY_MODE, OWNER_FILE_MODE},
    support::{header, secure_directory},
};

#[test]
fn live_suffix_crosses_pending_operation_without_claiming_full_verification() {
    use std::io::{Seek as _, SeekFrom};

    let temporary = TempDir::new().expect("temporary directory");
    let path = temporary.path().join("session/events.jsonl");
    secure_directory(path.parent().expect("parent"));

    let clock = SessionClock::now().expect("clock");
    let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("writer");
    let operation_id = Uuid::now_v7();
    writer
        .append_at(
            10,
            Event::InputIntent {
                operation_id,
                source: InputSource::Agent,
                input_kind: InputKind::Keys,
                descriptor: InputDescriptor::Raw,
                data: ByteString::from(&b"x"[..]),
                terminal_modes_hash: None,
                semantic_input: None,
                caller_intent: None,
            },
        )
        .expect("pending intent");
    let (start, _) = writer.follow_cursors(writer.next_seq());
    let start = start.expect("retained position");

    writer
        .append_at(
            11,
            Event::InputResult {
                operation_id,
                status: OperationStatus::Applied,
                bytes_written: 1,
                error_code: None,
            },
        )
        .expect("input result");
    let (_, durable) = writer.follow_cursors(start.next_event_seq);
    let (device, inode, header_hash) = writer.source_identity();

    assert!(EventLog::read(&path)
        .expect("full verification")
        .pending_operations
        .is_empty());

    // The old intent is outside the requested suffix. Corrupting it makes any
    // hidden prefix scan fail, while the actor-anchored result remains authentic.
    let (header_position, _) = writer.follow_cursors(0);
    let mut source = OpenOptions::new()
        .write(true)
        .open(&path)
        .expect("open old prefix");
    source
        .seek(SeekFrom::Start(
            header_position.expect("header cursor").file_offset,
        ))
        .expect("seek old intent");
    source.write_all(b"!").expect("corrupt old intent");
    source.sync_all().expect("sync corruption");

    let mut cursor = EventLogCursor::open(&path).expect("open");
    cursor
        .resume_live_suffix(&start, device, inode, &header_hash)
        .expect("actor seed");
    let record = cursor.next_record().expect("read suffix").expect("result");
    assert!(matches!(record.event, Event::InputResult { .. }));

    cursor
        .validate_live_boundary(&durable)
        .expect("actor endpoint");

    assert!(cursor.next_record().expect("eof").is_none());
    assert!(matches!(
        cursor.finish(),
        Err(Error::InvalidEventLog { .. })
    ));
    assert!(
        EventLog::read(&path).is_err(),
        "suffix success must not imply full-log validity"
    );
}

#[test]
fn rejects_header_without_terminal_colors() {
    let temporary = TempDir::new().expect("temporary directory");
    let path = temporary.path().join("session/events.jsonl");
    secure_directory(path.parent().expect("parent"));

    let clock = SessionClock::now().expect("valid clock");
    let mut value = serde_json::to_value(header(&clock)).expect("header JSON");
    value
        .as_object_mut()
        .expect("header object")
        .remove("colors");
    let mut encoded = serde_json::to_vec(&value).expect("encoded header");
    encoded.push(b'\n');
    fs_err::write(&path, encoded).expect("write incomplete header");

    let error = EventLog::read(&path).expect_err("missing terminal colors must fail");
    assert!(matches!(error, Error::InvalidEventLog { line: 1, .. }));
}

#[test]
fn round_trip_preserves_binary_events_and_integrity_chain() {
    let temporary = TempDir::new().expect("temporary directory");
    let path = temporary.path().join("session/events.jsonl");
    secure_directory(path.parent().expect("parent"));

    let clock = SessionClock::now().expect("valid clock");
    let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("create log");
    writer
        .append_at(
            10,
            Event::Output {
                data: ByteString::from(&b"\x1b[31m\xff"[..]),
            },
        )
        .expect("append output");

    let input_id = Uuid::now_v7();
    writer
        .append_at(
            20,
            Event::InputIntent {
                operation_id: input_id,
                source: InputSource::Agent,
                input_kind: InputKind::Keys,
                descriptor: InputDescriptor::Raw,
                data: ByteString::from(&b"\x03"[..]),
                terminal_modes_hash: None,
                semantic_input: Some("ctrl+c".to_owned()),
                caller_intent: None,
            },
        )
        .expect("append input");
    writer
        .append_at(
            21,
            Event::InputResult {
                operation_id: input_id,
                status: OperationStatus::Applied,
                bytes_written: 1,
                error_code: None,
            },
        )
        .expect("append input result");
    drop(writer);

    let recording = EventLog::read(&path).expect("read valid recording");

    assert_eq!(recording.events.len(), 3);
    assert!(recording.pending_operations.is_empty());

    let encoded = fs_err::read_to_string(&path).expect("event log bytes");
    for (line, record) in encoded.lines().skip(1).zip(&recording.events) {
        assert_eq!(
            line,
            serde_json::to_string(record).expect("canonical event record JSON"),
            "writer must preserve the canonical record wire encoding"
        );
    }

    assert_eq!(recording.events[0].seq, 0);
    assert_eq!(recording.events[1].seq, 1);
    assert!(matches!(
        &recording.events[0].event,
        Event::Output { data } if data.as_bytes() == b"\x1b[31m\xff"
    ));

    let prefix = EventLog::read_prefix(&path, 2).expect("validated live prefix");
    assert_eq!(prefix.events.len(), 2);
    assert_eq!(prefix.pending_operations.len(), 1);
    assert!(EventLog::read_prefix(&path, 4).is_err());

    OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("append simulated concurrent tail")
        .write_all(b"{\"partial\"")
        .expect("write simulated concurrent tail");

    assert_eq!(
        EventLog::read_prefix(&path, 3)
            .expect("prefix ignores later partial append")
            .events
            .len(),
        3
    );
    assert!(EventLog::read(&path).is_err());

    let directory_mode = fs_err::metadata(path.parent().expect("parent"))
        .expect("directory metadata")
        .permissions()
        .mode()
        & 0o777;
    let file_mode = fs_err::metadata(&path)
        .expect("file metadata")
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(directory_mode, OWNER_DIRECTORY_MODE);
    assert_eq!(file_mode, OWNER_FILE_MODE);
}

#[test]
fn live_cursor_reads_each_appended_event_once() {
    let temporary = TempDir::new().expect("temporary directory");
    let path = temporary.path().join("session/events.jsonl");
    secure_directory(path.parent().expect("parent"));

    let clock = SessionClock::now().expect("valid clock");
    let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("create log");
    let mut cursor = EventLogCursor::open(&path).expect("open live cursor");
    assert!(cursor.next_record().expect("empty cursor").is_none());

    writer
        .append_at(
            1,
            Event::Output {
                data: ByteString::from(&b"first"[..]),
            },
        )
        .expect("append first output");

    assert!(matches!(
        cursor.next_record().expect("first cursor event"),
        Some(record)
            if matches!(&record.event, Event::Output { data } if data.as_bytes() == b"first")
    ));
    assert!(cursor.next_record().expect("cursor caught up").is_none());

    writer
        .append_at(
            2,
            Event::Output {
                data: ByteString::from(&b"second"[..]),
            },
        )
        .expect("append second output");

    assert!(matches!(
        cursor.next_record().expect("second cursor event"),
        Some(record)
            if record.seq == 1
                && matches!(&record.event, Event::Output { data } if data.as_bytes() == b"second")
    ));
}

#[test]
fn reports_intent_without_result_as_uncertain() {
    let temporary = TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("session");
    secure_directory(&directory);
    let path = directory.join("events.jsonl");
    let clock = SessionClock::now().expect("valid clock");
    let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("create log");
    let operation_id = Uuid::now_v7();
    writer
        .append_at(
            1,
            Event::InputIntent {
                operation_id,
                source: InputSource::Agent,
                input_kind: InputKind::Text,
                descriptor: InputDescriptor::Text,
                data: ByteString::from(&b"hello"[..]),
                terminal_modes_hash: None,
                semantic_input: Some("text".to_owned()),
                caller_intent: None,
            },
        )
        .expect("append intent");
    drop(writer);

    let recording = EventLog::read(&path).expect("read interrupted recording");

    assert_eq!(
        recording.pending_operations,
        vec![PendingOperation {
            operation_id,
            kind: OperationKind::Input,
            intent_seq: 0,
            expected_bytes: Some(5),
        }]
    );
}

#[test]
fn rejects_result_without_matching_intent() {
    let temporary = TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("session");
    secure_directory(&directory);
    let path = directory.join("events.jsonl");
    let clock = SessionClock::now().expect("valid clock");
    let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("create log");
    writer
        .append_at(
            1,
            Event::InputResult {
                operation_id: Uuid::now_v7(),
                status: OperationStatus::Applied,
                bytes_written: 0,
                error_code: None,
            },
        )
        .expect("append unmatched result");
    drop(writer);

    let error = EventLog::read(&path).expect_err("unmatched result must fail");
    assert!(matches!(error, Error::InvalidEventLog { line: 2, .. }));
}

#[test]
fn rejects_tampered_event_payload() {
    let temporary = TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("session");
    secure_directory(&directory);
    let path = directory.join("events.jsonl");
    let clock = SessionClock::now().expect("valid clock");
    let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("create log");
    writer
        .append_at(
            1,
            Event::Marker {
                operation_id: Uuid::now_v7(),
                label: "before".to_owned(),
            },
        )
        .expect("append marker");
    drop(writer);

    let original = fs_err::read_to_string(&path).expect("read log");
    fs_err::write(&path, original.replace("before", "after!")).expect("tamper log");

    let error = EventLog::read(&path).expect_err("tampering must fail");
    assert!(matches!(error, Error::InvalidEventLog { line: 2, .. }));
}

#[test]
fn rejects_truncated_last_event() {
    let temporary = TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("session");
    secure_directory(&directory);
    let path = directory.join("events.jsonl");
    let clock = SessionClock::now().expect("valid clock");
    let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("create log");
    writer
        .append_at(
            1,
            Event::Marker {
                operation_id: Uuid::now_v7(),
                label: "complete".to_owned(),
            },
        )
        .expect("append marker");
    drop(writer);

    let mut bytes = fs_err::read(&path).expect("read log");
    bytes.pop();
    fs_err::write(&path, bytes).expect("truncate log");

    let error = EventLog::read(&path).expect_err("truncation must fail");
    assert!(matches!(error, Error::InvalidEventLog { line: 2, .. }));
}

#[test]
fn rejects_malformed_base64_with_event_path() {
    let temporary = TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("session");
    secure_directory(&directory);
    let path = directory.join("events.jsonl");
    let clock = SessionClock::now().expect("valid clock");
    let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("create log");
    writer
        .append_at(
            1,
            Event::Output {
                data: ByteString::from(&b"valid"[..]),
            },
        )
        .expect("append output");
    drop(writer);

    let original = fs_err::read_to_string(&path).expect("read log");
    fs_err::write(&path, original.replace("dmFsaWQ=", "%%%=")).expect("write malformed log");

    let error = EventLog::read(&path).expect_err("malformed Base64 must fail");
    assert!(matches!(error, Error::InvalidEventLog { .. }));
}
