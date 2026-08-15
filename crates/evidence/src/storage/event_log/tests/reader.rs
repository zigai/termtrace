use std::{fs::OpenOptions, io::Write, os::unix::fs::PermissionsExt as _};

use tempfile::TempDir;
use uuid::Uuid;

use termtrace_model::execution::event::{
    Event, InputDescriptor, InputKind, InputSource, OperationStatus,
};
use termtrace_model::value::ByteString;

use crate::{
    Error,
    event_log::{
        EventLog, EventLogCursor, EventLogWriter, OperationKind, PendingOperation, SessionClock,
    },
};

use super::{
    super::validation::{OWNER_DIRECTORY_MODE, OWNER_FILE_MODE},
    support::{header, secure_directory},
};

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
    match error {
        Error::InvalidEventLog { reason, .. } => {
            assert!(reason.contains("`event`"), "unexpected reason: {reason}");
        }
        other => panic!("unexpected error: {other}"),
    }
}
