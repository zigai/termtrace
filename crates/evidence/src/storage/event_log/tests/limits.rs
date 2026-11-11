use super::{
    super::reader::{parse_event_log_with_limits, EventLogLimits, DEFAULT_EVENT_LOG_LIMITS},
    support::{header, secure_directory},
};
use crate::event_log::{EventLogSummary, EventLogWriter, SessionClock};
use crate::storage::Error;
use tempfile::TempDir;
use termtrace_model::execution::event::Event;
use termtrace_model::value::ByteString;

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
        None,
        EventLogLimits {
            event_count: 1,
            ..DEFAULT_EVENT_LOG_LIMITS
        },
    )
    .expect_err("aggregate event count must be bounded");
    assert!(matches!(count_error, Error::InvalidEventLog { .. }));

    let payload_error = parse_event_log_with_limits(
        &path,
        None,
        EventLogLimits {
            decoded_payload_bytes: 5,
            ..DEFAULT_EVENT_LOG_LIMITS
        },
    )
    .expect_err("aggregate decoded payload must be bounded");
    assert!(matches!(payload_error, Error::InvalidEventLog { .. }));
    let summary =
        EventLogSummary::read(&path).expect("streaming validation is not a retention allocation");
    assert_eq!(summary.output_bytes, 6);
    assert_eq!(summary.event_count, 2);

    let mut checks = 0;
    let cancelled = EventLogSummary::read_with_check(&path, || {
        checks += 1;

        if checks == 2 {
            return Err(Error::InvalidEvidence {
                reason: "cancelled".to_owned(),
                source: None,
            });
        }
        return Ok(());
    })
    .expect_err("streaming validation must observe cancellation between records");
    assert!(matches!(cancelled, Error::InvalidEvidence { .. }));

    let encoded_error = parse_event_log_with_limits(
        &path,
        None,
        EventLogLimits {
            encoded_bytes: 1,
            ..DEFAULT_EVENT_LOG_LIMITS
        },
    )
    .expect_err("aggregate encoded bytes must be bounded");
    assert!(matches!(encoded_error, Error::InvalidEventLog { .. }));

    let prefix_error = parse_event_log_with_limits(
        &path,
        Some(2),
        EventLogLimits {
            event_count: 1,
            ..DEFAULT_EVENT_LOG_LIMITS
        },
    )
    .expect_err("prefix readers must share the count policy");
    assert!(matches!(prefix_error, Error::InvalidEventLog { .. }));
    let mut truncated = fs_err::read(&path).expect("canonical bytes");
    truncated.pop();
    fs_err::write(&path, truncated).expect("unterminated terminal line");
    assert!(matches!(
        EventLogSummary::read(&path),
        Err(Error::InvalidEventLog { .. })
    ));
}
