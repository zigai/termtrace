use tempfile::TempDir;

use crate::{
    event::{ByteString, Event},
    event_log::{EventLogWriter, SessionClock},
};

use super::{
    super::reader::{DEFAULT_EVENT_LOG_LIMITS, EventLogLimits, parse_event_log_with_limits},
    support::{header, secure_directory},
};

#[test]
fn enforces_aggregate_event_log_limits_before_retention() {
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
                data: ByteString::from(&b"abc"[..]),
            },
        )
        .expect("append first output");
    writer
        .append_at(
            2,
            Event::Output {
                data: ByteString::from(&b"def"[..]),
            },
        )
        .expect("append second output");
    drop(writer);

    let count_error = parse_event_log_with_limits(
        &path,
        false,
        None,
        EventLogLimits {
            event_count: 1,
            ..DEFAULT_EVENT_LOG_LIMITS
        },
    )
    .expect_err("aggregate event count must be bounded");
    assert!(count_error.to_string().contains("event limit"));

    let payload_error = parse_event_log_with_limits(
        &path,
        false,
        None,
        EventLogLimits {
            decoded_payload_bytes: 5,
            ..DEFAULT_EVENT_LOG_LIMITS
        },
    )
    .expect_err("aggregate decoded payload must be bounded");
    assert!(payload_error.to_string().contains("decoded-payload-byte"));

    let encoded_error = parse_event_log_with_limits(
        &path,
        false,
        None,
        EventLogLimits {
            encoded_bytes: 1,
            ..DEFAULT_EVENT_LOG_LIMITS
        },
    )
    .expect_err("aggregate encoded bytes must be bounded");
    assert!(encoded_error.to_string().contains("encoded-byte"));

    let prefix_error = parse_event_log_with_limits(
        &path,
        false,
        Some(2),
        EventLogLimits {
            event_count: 1,
            ..DEFAULT_EVENT_LOG_LIMITS
        },
    )
    .expect_err("prefix readers must share the count policy");
    assert!(prefix_error.to_string().contains("prefix exceeds"));
}
