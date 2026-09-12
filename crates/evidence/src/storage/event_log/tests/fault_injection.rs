use super::support::{header, secure_directory};
use crate::{
    event_log::{EventLog, EventLogWriter, SessionClock},
    Error,
};
use std::{fs::OpenOptions, io::Write as _, path::PathBuf};
use tempfile::TempDir;
use termtrace_model::{
    execution::event::{Event, InputDescriptor, InputKind, InputSource, OperationStatus},
    value::ByteString,
};
use uuid::Uuid;

struct TestFixture {
    _temporary: TempDir,
    source: PathBuf,
    recovered: PathBuf,
}

impl TestFixture {
    fn new() -> Self {
        let temporary = TempDir::new().expect("temporary directory");
        let directory = temporary.path().join("session");
        secure_directory(&directory);
        let source = directory.join("damaged.jsonl");
        let recovered = directory.join("recovered.jsonl");

        return Self {
            _temporary: temporary,
            source,
            recovered,
        };
    }

    fn write_valid_log(&self, event_count: usize) -> Vec<String> {
        let clock = SessionClock::now().expect("valid clock");
        let mut writer =
            EventLogWriter::create(&self.source, &header(&clock), clock).expect("create log");

        for i in 0..event_count {
            let payload = format!("payload-{i}");
            writer
                .append_at(
                    u64::try_from(i.saturating_mul(10)).unwrap_or(0),
                    Event::Output {
                        data: ByteString::from(payload.into_bytes()),
                    },
                )
                .expect("append event");
        }

        drop(writer);

        let content = fs_err::read_to_string(&self.source).expect("read valid log");

        return content.lines().map(ToOwned::to_owned).collect();
    }

    fn write_valid_log_with_timestamps(&self, timestamps: &[u64]) -> Vec<String> {
        let clock = SessionClock::now().expect("valid clock");
        let mut writer =
            EventLogWriter::create(&self.source, &header(&clock), clock).expect("create log");

        for (i, &ts) in timestamps.iter().enumerate() {
            let payload = format!("payload-{i}");
            writer
                .append_at(
                    ts,
                    Event::Output {
                        data: ByteString::from(payload.into_bytes()),
                    },
                )
                .expect("append event");
        }

        drop(writer);

        let content = fs_err::read_to_string(&self.source).expect("read valid log");

        return content.lines().map(ToOwned::to_owned).collect();
    }
}

fn write_lines(path: &PathBuf, lines_to_write: &[&str]) {
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(path)
        .expect("open file for write");
    for line in lines_to_write {
        file.write_all(line.as_bytes()).expect("write line");
        file.write_all(b"\n").expect("write newline");
    }

    file.sync_data().expect("sync file");
}

fn write_lines_with_raw_tail(path: &PathBuf, full_lines: &[&str], raw_tail: &[u8]) {
    let mut file = OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(path)
        .expect("open file for write");
    for line in full_lines {
        file.write_all(line.as_bytes()).expect("write line");
        file.write_all(b"\n").expect("write newline");
    }

    file.write_all(raw_tail).expect("write raw tail");
    file.sync_data().expect("sync file");
}

#[test]
fn mid_line_byte_truncation_on_first_event_at_various_offsets() {
    let fixture = TestFixture::new();
    let lines = fixture.write_valid_log(1);
    let header_line = lines.first().expect("header line");
    let event_line = lines.get(1).expect("event line");

    let offsets = [
        1_usize,
        5,
        15,
        event_line.len().saturating_div(2),
        event_line.len().saturating_sub(1),
        event_line.len(),
    ];

    for &offset in &offsets {
        let sub_slice = event_line
            .as_bytes()
            .get(..offset)
            .expect("subslice within event line");

        write_lines_with_raw_tail(&fixture.source, &[header_line], sub_slice);

        if fixture.recovered.exists() {
            fs_err::remove_file(&fixture.recovered).expect("remove prior recovered");
        }

        let report =
            EventLog::recover_prefix(&fixture.source, &fixture.recovered).expect("recover prefix");
        assert_eq!(report.valid_events, 0);

        let tail = report.discarded_tail.as_ref().expect("discarded tail");
        assert_eq!(tail.line, 2);
        assert_eq!(tail.reason, "truncated line without newline terminator");

        let recovered_log = EventLog::read(&fixture.recovered).expect("read recovered");
        assert_eq!(recovered_log.events.len(), 0);

        let err = EventLog::read(&fixture.source).expect_err("read damaged source");

        match err {
            Error::InvalidEventLog { line, reason, .. } => {
                assert_eq!(line, 2);
                assert_eq!(reason, "truncated line without newline terminator");
            }
            other => panic!("expected InvalidEventLog, got {other:?}"),
        }
    }
}

#[test]
fn mid_line_byte_truncation_on_later_event_recovers_prior_events() {
    let fixture = TestFixture::new();
    let lines = fixture.write_valid_log(3);
    let header_line = lines.first().expect("header line");
    let event_0 = lines.get(1).expect("event 0");
    let event_1 = lines.get(2).expect("event 1");
    let event_2 = lines.get(3).expect("event 2");

    let offsets = [
        1_usize,
        10,
        event_2.len().saturating_div(2),
        event_2.len().saturating_sub(1),
        event_2.len(),
    ];

    for &offset in &offsets {
        let sub_slice = event_2
            .as_bytes()
            .get(..offset)
            .expect("subslice within event 2");

        write_lines_with_raw_tail(&fixture.source, &[header_line, event_0, event_1], sub_slice);

        if fixture.recovered.exists() {
            fs_err::remove_file(&fixture.recovered).expect("remove prior recovered");
        }

        let report =
            EventLog::recover_prefix(&fixture.source, &fixture.recovered).expect("recover prefix");
        assert_eq!(report.valid_events, 2);

        let tail = report.discarded_tail.as_ref().expect("discarded tail");
        assert_eq!(tail.line, 4);
        assert_eq!(tail.reason, "truncated line without newline terminator");

        let recovered_log = EventLog::read(&fixture.recovered).expect("read recovered");
        assert_eq!(recovered_log.events.len(), 2);
        assert_eq!(recovered_log.events.first().expect("first event").seq, 0);
        assert_eq!(recovered_log.events.get(1).expect("second event").seq, 1);

        let err = EventLog::read(&fixture.source).expect_err("read damaged source");

        match err {
            Error::InvalidEventLog { line, reason, .. } => {
                assert_eq!(line, 4);
                assert_eq!(reason, "truncated line without newline terminator");
            }
            other => panic!("expected InvalidEventLog, got {other:?}"),
        }
    }
}

#[test]
fn torn_write_with_trailing_newline_malformed_json() {
    let fixture = TestFixture::new();
    let lines = fixture.write_valid_log(3);
    let header_line = lines.first().expect("header line");
    let event_0 = lines.get(1).expect("event 0");
    let event_1 = lines.get(2).expect("event 1");
    let event_2 = lines.get(3).expect("event 2");
    let sub_len = event_2.len().saturating_div(3);
    let mut raw_tail = event_2
        .as_bytes()
        .get(..sub_len)
        .expect("subslice")
        .to_vec();
    raw_tail.push(b'\n');
    write_lines_with_raw_tail(&fixture.source, &[header_line, event_0, event_1], &raw_tail);

    let report =
        EventLog::recover_prefix(&fixture.source, &fixture.recovered).expect("recover prefix");
    assert_eq!(report.valid_events, 2);

    let tail = report.discarded_tail.as_ref().expect("discarded tail");
    assert_eq!(tail.line, 4);
    assert!(tail.reason.contains("JSON decoding failed"));

    let recovered_log = EventLog::read(&fixture.recovered).expect("read recovered");
    assert_eq!(recovered_log.events.len(), 2);

    let err = EventLog::read(&fixture.source).expect_err("read damaged source");

    match err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 4);
            assert!(reason.contains("JSON decoding failed"));
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }
}

#[test]
fn corrupted_blake3_hash_field_halts_recovery_and_discards_invalid_tail() {
    let fixture = TestFixture::new();
    let lines = fixture.write_valid_log(4);
    let header_line = lines.first().expect("header line");
    let event_0 = lines.get(1).expect("event 0");
    let event_1 = lines.get(2).expect("event 1");
    let event_2 = lines.get(3).expect("event 2");
    let event_3 = lines.get(4).expect("event 3");
    let mut val: serde_json::Value = serde_json::from_str(event_1).expect("parse event 1 JSON");
    let obj = val.as_object_mut().expect("json object");
    obj.insert(
        "hash".to_owned(),
        serde_json::Value::String(
            "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        ),
    );

    let corrupted_event_1 = serde_json::to_string(&val).expect("serialize corrupted event 1");

    write_lines(
        &fixture.source,
        &[header_line, event_0, &corrupted_event_1, event_2, event_3],
    );

    let report =
        EventLog::recover_prefix(&fixture.source, &fixture.recovered).expect("recover prefix");
    assert_eq!(report.valid_events, 1);

    let tail = report.discarded_tail.as_ref().expect("discarded tail");
    assert_eq!(tail.line, 3);
    assert_eq!(tail.reason, "event hash does not match");

    let recovered_log = EventLog::read(&fixture.recovered).expect("read recovered");
    assert_eq!(recovered_log.events.len(), 1);
    assert_eq!(recovered_log.events.first().expect("event 0").seq, 0);

    let err = EventLog::read(&fixture.source).expect_err("read damaged source");

    match err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 3);
            assert_eq!(reason, "event hash does not match");
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }
}

#[test]
fn corrupted_blake3_prev_hash_field_halts_recovery_and_discards_invalid_tail() {
    let fixture = TestFixture::new();
    let lines = fixture.write_valid_log(4);
    let header_line = lines.first().expect("header line");
    let event_0 = lines.get(1).expect("event 0");
    let event_1 = lines.get(2).expect("event 1");
    let event_2 = lines.get(3).expect("event 2");
    let event_3 = lines.get(4).expect("event 3");
    let mut val: serde_json::Value = serde_json::from_str(event_1).expect("parse event 1 JSON");
    let obj = val.as_object_mut().expect("json object");
    obj.insert(
        "prev_hash".to_owned(),
        serde_json::Value::String(
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_owned(),
        ),
    );

    let corrupted_event_1 = serde_json::to_string(&val).expect("serialize corrupted event 1");

    write_lines(
        &fixture.source,
        &[header_line, event_0, &corrupted_event_1, event_2, event_3],
    );

    let report =
        EventLog::recover_prefix(&fixture.source, &fixture.recovered).expect("recover prefix");
    assert_eq!(report.valid_events, 1);

    let tail = report.discarded_tail.as_ref().expect("discarded tail");
    assert_eq!(tail.line, 3);
    assert_eq!(tail.reason, "previous hash does not match");

    let recovered_log = EventLog::read(&fixture.recovered).expect("read recovered");
    assert_eq!(recovered_log.events.len(), 1);
    assert_eq!(recovered_log.events.first().expect("event 0").seq, 0);

    let err = EventLog::read(&fixture.source).expect_err("read damaged source");

    match err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 3);
            assert_eq!(reason, "previous hash does not match");
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }
}

#[test]
fn sequence_jump_halts_recovery_and_reports_diagnostic() {
    let fixture = TestFixture::new();
    let lines = fixture.write_valid_log(3);
    let header_line = lines.first().expect("header line");
    let event_0 = lines.get(1).expect("event 0");
    let event_1 = lines.get(2).expect("event 1");
    let event_2 = lines.get(3).expect("event 2");
    let mut val: serde_json::Value = serde_json::from_str(event_1).expect("parse event 1 JSON");
    let obj = val.as_object_mut().expect("json object");
    obj.insert("seq".to_owned(), serde_json::json!(5));

    let corrupted_event_1 = serde_json::to_string(&val).expect("serialize corrupted event 1");

    write_lines(
        &fixture.source,
        &[header_line, event_0, &corrupted_event_1, event_2],
    );

    let report =
        EventLog::recover_prefix(&fixture.source, &fixture.recovered).expect("recover prefix");
    assert_eq!(report.valid_events, 1);

    let tail = report.discarded_tail.as_ref().expect("discarded tail");
    assert_eq!(tail.line, 3);
    assert_eq!(tail.reason, "expected sequence 1, found 5");

    let recovered_log = EventLog::read(&fixture.recovered).expect("read recovered");
    assert_eq!(recovered_log.events.len(), 1);
    assert_eq!(recovered_log.events.first().expect("event 0").seq, 0);

    let err = EventLog::read(&fixture.source).expect_err("read damaged source");

    match err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 3);
            assert_eq!(reason, "expected sequence 1, found 5");
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }
}

#[test]
fn sequence_regression_halts_recovery_and_reports_diagnostic() {
    let fixture = TestFixture::new();
    let lines = fixture.write_valid_log(3);
    let header_line = lines.first().expect("header line");
    let event_0 = lines.get(1).expect("event 0");
    let event_1 = lines.get(2).expect("event 1");
    let event_2 = lines.get(3).expect("event 2");
    let mut val: serde_json::Value = serde_json::from_str(event_1).expect("parse event 1 JSON");
    let obj = val.as_object_mut().expect("json object");
    obj.insert("seq".to_owned(), serde_json::json!(0));

    let corrupted_event_1 = serde_json::to_string(&val).expect("serialize corrupted event 1");

    write_lines(
        &fixture.source,
        &[header_line, event_0, &corrupted_event_1, event_2],
    );

    let report =
        EventLog::recover_prefix(&fixture.source, &fixture.recovered).expect("recover prefix");
    assert_eq!(report.valid_events, 1);

    let tail = report.discarded_tail.as_ref().expect("discarded tail");
    assert_eq!(tail.line, 3);
    assert_eq!(tail.reason, "expected sequence 1, found 0");

    let recovered_log = EventLog::read(&fixture.recovered).expect("read recovered");
    assert_eq!(recovered_log.events.len(), 1);
    assert_eq!(recovered_log.events.first().expect("event 0").seq, 0);

    let err = EventLog::read(&fixture.source).expect_err("read damaged source");

    match err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 3);
            assert_eq!(reason, "expected sequence 1, found 0");
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }
}

#[test]
fn monotonic_timestamp_regression_halts_recovery() {
    let fixture = TestFixture::new();
    let lines = fixture.write_valid_log_with_timestamps(&[100, 200, 300]);
    let header_line = lines.first().expect("header line");
    let event_0 = lines.get(1).expect("event 0");
    let event_1 = lines.get(2).expect("event 1");
    let event_2 = lines.get(3).expect("event 2");
    let mut val: serde_json::Value = serde_json::from_str(event_1).expect("parse event 1 JSON");
    let obj = val.as_object_mut().expect("json object");
    obj.insert("monotonic_ns".to_owned(), serde_json::json!(50));

    let corrupted_event_1 = serde_json::to_string(&val).expect("serialize corrupted event 1");

    write_lines(
        &fixture.source,
        &[header_line, event_0, &corrupted_event_1, event_2],
    );

    let report =
        EventLog::recover_prefix(&fixture.source, &fixture.recovered).expect("recover prefix");
    assert_eq!(report.valid_events, 1);

    let tail = report.discarded_tail.as_ref().expect("discarded tail");
    assert_eq!(tail.line, 3);
    assert_eq!(tail.reason, "monotonic timestamp moved backwards");

    let recovered_log = EventLog::read(&fixture.recovered).expect("read recovered");
    assert_eq!(recovered_log.events.len(), 1);
    assert_eq!(recovered_log.events.first().expect("event 0").seq, 0);

    let err = EventLog::read(&fixture.source).expect_err("read damaged source");

    match err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 3);
            assert_eq!(reason, "monotonic timestamp moved backwards");
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }
}

#[test]
fn zero_byte_tail_padding_unflushed_block_discarded() {
    let fixture = TestFixture::new();
    let lines = fixture.write_valid_log(3);
    let header_line = lines.first().expect("header line");
    let event_0 = lines.get(1).expect("event 0");
    let event_1 = lines.get(2).expect("event 1");
    let event_2 = lines.get(3).expect("event 2");

    let zeros = vec![0_u8; 512];
    write_lines_with_raw_tail(
        &fixture.source,
        &[header_line, event_0, event_1, event_2],
        &zeros,
    );

    let report =
        EventLog::recover_prefix(&fixture.source, &fixture.recovered).expect("recover prefix");
    assert_eq!(report.valid_events, 3);

    let tail = report.discarded_tail.as_ref().expect("discarded tail");
    assert_eq!(tail.line, 5);
    assert_eq!(tail.reason, "truncated line without newline terminator");

    let recovered_log = EventLog::read(&fixture.recovered).expect("read recovered");
    assert_eq!(recovered_log.events.len(), 3);
    assert_eq!(recovered_log.events.first().expect("first event").seq, 0);
    assert_eq!(recovered_log.events.get(2).expect("third event").seq, 2);

    let err = EventLog::read(&fixture.source).expect_err("read damaged source");

    match err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 5);
            assert_eq!(reason, "truncated line without newline terminator");
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }
}

#[test]
fn zero_byte_tail_padding_with_newline_discarded() {
    let fixture = TestFixture::new();
    let lines = fixture.write_valid_log(3);
    let header_line = lines.first().expect("header line");
    let event_0 = lines.get(1).expect("event 0");
    let event_1 = lines.get(2).expect("event 1");
    let event_2 = lines.get(3).expect("event 2");

    let mut zeros = vec![0_u8; 4096];
    zeros.push(b'\n');
    write_lines_with_raw_tail(
        &fixture.source,
        &[header_line, event_0, event_1, event_2],
        &zeros,
    );

    let report =
        EventLog::recover_prefix(&fixture.source, &fixture.recovered).expect("recover prefix");
    assert_eq!(report.valid_events, 3);

    let tail = report.discarded_tail.as_ref().expect("discarded tail");
    assert_eq!(tail.line, 5);
    assert!(tail.reason.contains("JSON decoding failed"));

    let recovered_log = EventLog::read(&fixture.recovered).expect("read recovered");
    assert_eq!(recovered_log.events.len(), 3);

    let err = EventLog::read(&fixture.source).expect_err("read damaged source");

    match err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 5);
            assert!(reason.contains("JSON decoding failed"));
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }
}

#[test]
fn zero_byte_header_file_fails_recovery_and_read() {
    let fixture = TestFixture::new();
    fs_err::write(&fixture.source, b"").expect("write empty file");

    let recovery_err =
        EventLog::recover_prefix(&fixture.source, &fixture.recovered).expect_err("recover empty");

    match recovery_err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 1);
            assert_eq!(reason, "missing header");
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }

    assert!(!fixture.recovered.exists());

    let read_err = EventLog::read(&fixture.source).expect_err("read empty");

    match read_err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 1);
            assert_eq!(reason, "missing header");
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }
}

#[test]
fn truncated_header_without_newline_fails_recovery_and_read() {
    let fixture = TestFixture::new();
    fs_err::write(&fixture.source, b"{\"version\":1").expect("write truncated header");

    let recovery_err = EventLog::recover_prefix(&fixture.source, &fixture.recovered)
        .expect_err("recover truncated header");

    match recovery_err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 1);
            assert_eq!(reason, "truncated line without newline terminator");
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }

    assert!(!fixture.recovered.exists());

    let read_err = EventLog::read(&fixture.source).expect_err("read truncated header");

    match read_err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 1);
            assert_eq!(reason, "truncated line without newline terminator");
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }
}

#[test]
fn corrupt_json_header_fails_recovery_and_read() {
    let fixture = TestFixture::new();
    fs_err::write(&fixture.source, b"{\"not_valid_json\":\n").expect("write corrupt header");

    let recovery_err = EventLog::recover_prefix(&fixture.source, &fixture.recovered)
        .expect_err("recover corrupt header");

    match recovery_err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 1);
            assert!(reason.contains("JSON decoding failed"));
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }

    assert!(!fixture.recovered.exists());

    let read_err = EventLog::read(&fixture.source).expect_err("read corrupt header");

    match read_err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 1);
            assert!(reason.contains("JSON decoding failed"));
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }
}

#[test]
fn invalid_header_fields_fails_recovery_and_read() {
    let fixture = TestFixture::new();
    let lines = fixture.write_valid_log(1);
    let header_line = lines.first().expect("header line");
    let mut val: serde_json::Value = serde_json::from_str(header_line).expect("parse header JSON");
    let obj = val.as_object_mut().expect("json object");
    obj.insert(
        "format".to_owned(),
        serde_json::Value::String("unsupported_format".to_owned()),
    );

    let corrupted_header = serde_json::to_string(&val).expect("serialize corrupted header");
    write_lines(&fixture.source, &[&corrupted_header]);

    let recovery_err = EventLog::recover_prefix(&fixture.source, &fixture.recovered)
        .expect_err("recover invalid header");

    match recovery_err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 1);
            assert_eq!(reason, "unsupported format identifier");
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }

    assert!(!fixture.recovered.exists());

    let read_err = EventLog::read(&fixture.source).expect_err("read invalid header");

    match read_err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 1);
            assert_eq!(reason, "unsupported format identifier");
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }
}

#[test]
fn truncated_header_with_subsequent_events_fails_at_line_one() {
    let fixture = TestFixture::new();
    let lines = fixture.write_valid_log(2);
    let event_0 = lines.get(1).expect("event 0");
    let event_1 = lines.get(2).expect("event 1");

    write_lines(
        &fixture.source,
        &["{\"corrupted_header\":true}", event_0, event_1],
    );

    let recovery_err = EventLog::recover_prefix(&fixture.source, &fixture.recovered)
        .expect_err("recover bad header with events");

    match recovery_err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 1);
            assert!(reason.contains("JSON decoding failed"));
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }

    assert!(!fixture.recovered.exists());

    let read_err = EventLog::read(&fixture.source).expect_err("read bad header with events");

    match read_err {
        Error::InvalidEventLog { line, reason, .. } => {
            assert_eq!(line, 1);
            assert!(reason.contains("JSON decoding failed"));
        }
        other => panic!("expected InvalidEventLog, got {other:?}"),
    }
}

#[test]
fn recovery_reports_uncertain_input_intent_when_torn_before_result() {
    let fixture = TestFixture::new();
    let clock = SessionClock::now().expect("valid clock");
    let mut writer =
        EventLogWriter::create(&fixture.source, &header(&clock), clock).expect("create log");

    let operation_id = Uuid::now_v7();

    writer
        .append_at(
            1,
            Event::Output {
                data: ByteString::from(&b"first"[..]),
            },
        )
        .expect("append output");
    writer
        .append_at(
            2,
            Event::InputIntent {
                operation_id,
                source: InputSource::Agent,
                input_kind: InputKind::Text,
                descriptor: InputDescriptor::Text,
                data: ByteString::from(&b"command\n"[..]),
                terminal_modes_hash: None,
                semantic_input: Some("command".to_owned()),
                caller_intent: None,
            },
        )
        .expect("append intent");
    drop(writer);

    let mut file = OpenOptions::new()
        .append(true)
        .open(&fixture.source)
        .expect("append damage");
    file.write_all(br#"{"seq":2,"torn":"#)
        .expect("write truncation");
    file.sync_data().expect("sync damage");

    let report =
        EventLog::recover_prefix(&fixture.source, &fixture.recovered).expect("recover prefix");
    assert_eq!(report.valid_events, 2);

    let tail = report.discarded_tail.as_ref().expect("discarded tail");
    assert_eq!(tail.line, 4);
    assert_eq!(tail.reason, "truncated line without newline terminator");
    assert_eq!(report.pending_operations.len(), 1);

    let pending = report
        .pending_operations
        .first()
        .expect("pending operation");
    assert_eq!(pending.operation_id, operation_id);
    assert_eq!(pending.kind, "input");
    assert_eq!(pending.intent_seq, 1);

    let recovered_log = EventLog::read(&fixture.recovered).expect("read recovered");
    assert_eq!(recovered_log.events.len(), 2);
}

#[test]
fn recovery_resolves_completed_operation_when_torn_after_result() {
    let fixture = TestFixture::new();
    let clock = SessionClock::now().expect("valid clock");
    let mut writer =
        EventLogWriter::create(&fixture.source, &header(&clock), clock).expect("create log");

    let operation_id = Uuid::now_v7();
    writer
        .append_at(
            1,
            Event::InputIntent {
                operation_id,
                source: InputSource::Agent,
                input_kind: InputKind::Text,
                descriptor: InputDescriptor::Text,
                data: ByteString::from(&b"hi"[..]),
                terminal_modes_hash: None,
                semantic_input: None,
                caller_intent: None,
            },
        )
        .expect("append intent");
    writer
        .append_at(
            2,
            Event::InputResult {
                operation_id,
                status: OperationStatus::Applied,
                bytes_written: 2,
                error_code: None,
            },
        )
        .expect("append result");
    drop(writer);

    let mut file = OpenOptions::new()
        .append(true)
        .open(&fixture.source)
        .expect("append damage");
    file.write_all(br#"{"seq":2,"broken":"#)
        .expect("write truncation");
    file.sync_data().expect("sync damage");

    let report =
        EventLog::recover_prefix(&fixture.source, &fixture.recovered).expect("recover prefix");
    assert_eq!(report.valid_events, 2);

    let tail = report.discarded_tail.as_ref().expect("discarded tail");
    assert_eq!(tail.line, 4);
    assert_eq!(tail.reason, "truncated line without newline terminator");
    assert!(report.pending_operations.is_empty());

    let recovered_log = EventLog::read(&fixture.recovered).expect("read recovered");
    assert_eq!(recovered_log.events.len(), 2);
    assert!(recovered_log.pending_operations.is_empty());
}
