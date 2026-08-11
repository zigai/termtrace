use std::{fs::OpenOptions, io::Write};

use tempfile::TempDir;
use uuid::Uuid;

use crate::{
    event::{ByteString, Event, InputDescriptor, InputKind, InputSource},
    event_log::{EventLog, EventLogWriter, SessionClock},
};

use super::support::{header, secure_directory};

#[test]
fn recovers_valid_prefix_and_reports_uncertain_intent() {
    let temporary = TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("session");
    secure_directory(&directory);
    let source = directory.join("damaged.jsonl");
    let recovered = directory.join("recovered.jsonl");
    let clock = SessionClock::now().expect("valid clock");
    let mut writer = EventLogWriter::create(&source, &header(&clock), clock).expect("create log");
    let operation_id = Uuid::now_v7();
    writer
        .append_at(
            1,
            Event::Output {
                data: ByteString::from(&b"safe"[..]),
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
                data: ByteString::from(&b"x"[..]),
                terminal_modes_hash: None,
                semantic_input: Some("text".to_owned()),
                caller_intent: None,
            },
        )
        .expect("append intent");
    drop(writer);
    let original_valid = fs_err::read(&source).expect("valid prefix");
    let mut file = OpenOptions::new()
        .append(true)
        .open(&source)
        .expect("append damage");
    file.write_all(br#"{"seq":2"#).expect("write truncation");
    file.sync_data().expect("sync damage");

    let report = EventLog::recover_prefix(&source, &recovered).expect("recover prefix");

    assert_eq!(report.valid_events, 2);
    assert_eq!(
        report
            .discarded_tail
            .as_ref()
            .map(|issue| return issue.line),
        Some(4)
    );
    assert_eq!(report.pending_operations.len(), 1);
    assert_eq!(report.pending_operations[0].operation_id, operation_id);
    assert_eq!(
        fs_err::read(&recovered).expect("recovered bytes"),
        original_valid
    );
    assert!(
        fs_err::read(&source)
            .expect("source retained")
            .ends_with(br#"{"seq":2"#)
    );
}
