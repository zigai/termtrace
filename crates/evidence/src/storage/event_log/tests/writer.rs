use std::{fs::Permissions, os::unix::fs::PermissionsExt as _, thread, time::Duration};

use tempfile::TempDir;
use uuid::Uuid;

use termtrace_model::{
    execution::event::Event, terminal::state::MAX_TERMINAL_COLUMNS, value::ByteString,
};

use crate::{
    Error,
    event_log::{DurabilityMode, DurabilityPolicy, EventLogWriter, SessionClock},
};

use super::support::{header, secure_directory};

#[test]
fn distinguishes_observed_and_durable_boundaries() {
    let temporary = TempDir::new().expect("temporary directory");
    let path = temporary.path().join("session/events.jsonl");
    secure_directory(path.parent().expect("parent"));
    let clock = SessionClock::now().expect("valid clock");
    let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("create log");
    assert_eq!(writer.observed_boundary(), writer.durable_boundary());

    writer
        .append_output(Event::Output {
            data: ByteString::from(&b"x"[..]),
        })
        .expect("buffer output");
    assert_eq!(writer.observed_boundary().next_event_seq, 1);
    assert_eq!(writer.observed_boundary().output_bytes, 1);
    assert_eq!(writer.durable_boundary().next_event_seq, 0);
    assert_eq!(writer.durable_boundary().output_bytes, 0);

    writer
        .append(Event::Marker {
            operation_id: Uuid::now_v7(),
            label: "flush".to_owned(),
        })
        .expect("durable marker");
    assert_eq!(writer.observed_boundary(), writer.durable_boundary());
    assert_eq!(writer.durable_boundary().next_event_seq, 2);
    assert_eq!(writer.durable_boundary().output_bytes, 1);
}

#[test]
fn applies_strict_bounded_and_throughput_output_policies() {
    let temporary = TempDir::new().expect("temporary directory");
    for (name, policy, expected_durable) in [
        (
            "strict",
            DurabilityPolicy {
                mode: DurabilityMode::Strict,
                maximum_interval_ms: None,
                maximum_uncommitted_bytes: None,
            },
            1,
        ),
        (
            "bounded",
            DurabilityPolicy {
                mode: DurabilityMode::Bounded,
                maximum_interval_ms: Some(1000),
                maximum_uncommitted_bytes: Some(1),
            },
            1,
        ),
        (
            "throughput",
            DurabilityPolicy {
                mode: DurabilityMode::Throughput,
                maximum_interval_ms: None,
                maximum_uncommitted_bytes: None,
            },
            0,
        ),
    ] {
        let directory = temporary.path().join(name);
        secure_directory(&directory);
        let path = directory.join("events.jsonl");
        let clock = SessionClock::now().expect("valid clock");
        let mut writer = EventLogWriter::create_with_policy(&path, &header(&clock), clock, policy)
            .expect("create policy writer");
        writer
            .append_output(Event::Output {
                data: ByteString::from(&b"x"[..]),
            })
            .expect("append output");
        assert_eq!(writer.durable_boundary().next_event_seq, expected_durable);
    }
}

#[test]
fn bounded_policy_flushes_an_idle_pending_prefix() {
    let temporary = TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("bounded-time");
    secure_directory(&directory);
    let path = directory.join("events.jsonl");
    let clock = SessionClock::now().expect("valid clock");
    let mut writer = EventLogWriter::create_with_policy(
        &path,
        &header(&clock),
        clock,
        DurabilityPolicy {
            mode: DurabilityMode::Bounded,
            maximum_interval_ms: Some(1),
            maximum_uncommitted_bytes: Some(1024),
        },
    )
    .expect("create bounded writer");
    writer
        .append_output(Event::Output {
            data: ByteString::from(&b"x"[..]),
        })
        .expect("append buffered output");
    thread::sleep(Duration::from_millis(5));
    assert!(writer.flush_due().expect("flush due prefix"));
    assert_eq!(writer.durable_boundary(), writer.observed_boundary());
    let record = writer.durability_record();
    assert!(record.maximum_observed_uncommitted_ns >= 1_000_000);
    assert!(record.final_sync_completed);
}

#[test]
fn refuses_group_readable_artifact_directory() {
    let temporary = TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("session");
    fs_err::create_dir(&directory).expect("create directory");
    fs_err::set_permissions(&directory, Permissions::from_mode(0o750))
        .expect("set insecure permissions");
    let path = directory.join("events.jsonl");
    let clock = SessionClock::now().expect("valid clock");

    let error = EventLogWriter::create(&path, &header(&clock), clock)
        .expect_err("insecure directory must fail");
    assert!(matches!(error, Error::InsecureArtifactDirectory { .. }));
    assert!(!path.exists());
}

#[test]
fn rejects_geometry_outside_the_shared_terminal_policy() {
    let temporary = TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("session");
    secure_directory(&directory);
    let path = directory.join("events.jsonl");
    let clock = SessionClock::now().expect("valid clock");
    let mut oversized = header(&clock);
    oversized.cols = MAX_TERMINAL_COLUMNS.saturating_add(1);
    let error = EventLogWriter::create(&path, &oversized, clock)
        .expect_err("oversized header must fail before file creation");
    assert!(matches!(error, Error::InvalidEventLog { line: 1, .. }));
    assert!(!path.exists());

    let clock = SessionClock::now().expect("valid clock");
    let mut writer =
        EventLogWriter::create(&path, &header(&clock), clock).expect("create valid log");
    let error = writer
        .append_at(
            1,
            Event::ResizeIntent {
                operation_id: Uuid::now_v7(),
                cols: MAX_TERMINAL_COLUMNS.saturating_add(1),
                rows: 1,
                pixel_width: 1,
                pixel_height: 1,
            },
        )
        .expect_err("oversized live resize must fail before persistence");
    assert!(matches!(error, Error::InvalidEventLog { line: 2, .. }));
}
