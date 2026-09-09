use super::support::{header, secure_directory};
use crate::{
    event_log::{
        DurabilityMode, DurabilityPolicy, EventLog, EventLogCursor, EventLogSummary,
        EventLogWriter, SessionClock,
    },
    Error,
};
use proptest::{collection, prelude::*};
use std::{
    fs::{self, Permissions},
    os::unix::fs::PermissionsExt as _,
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;
use termtrace_model::{
    execution::event::Event, terminal::state::MAX_TERMINAL_COLUMNS, value::ByteString,
};
use uuid::Uuid;

#[test]
fn canonical_validation_and_live_resume_cross_compressed_segments() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("session");
    secure_directory(&session);
    let path = session.join("events.jsonl");
    let clock = SessionClock::now().expect("clock");
    let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("writer");
    let payload = vec![b'x'; 48 * 1024];

    for _ in 0..620 {
        writer
            .append(Event::Output {
                data: ByteString::from(payload.as_slice()),
            })
            .expect("append across chunks");
    }

    writer
        .append(Event::Exit {
            code: Some(0),
            signal: None,
            signal_name: None,
        })
        .expect("exit");

    assert_eq!(
        fs::metadata(&path).expect("root metadata").len(),
        16 * 1024 * 1024
    );
    assert!(session
        .join("events.jsonl.segments/00000000000000000001.zst")
        .is_file());

    let summary = EventLogSummary::read(&path).expect("validate all chunks");

    assert_eq!(summary.event_count, 621);
    assert_eq!(summary.output_bytes, 620 * 48 * 1024);
    assert_eq!(
        summary.terminal.expect("terminal record").hash,
        writer.current_hash()
    );

    let (start, end) = writer.follow_cursors(510);
    let (device, inode, hash) = writer.source_identity();
    let mut suffix = EventLogCursor::open(&path).expect("suffix cursor");
    suffix
        .resume_live_suffix(&start.expect("retained cursor"), device, inode, &hash)
        .expect("seek compressed segment");

    let mut count = 0;
    while suffix.next_record().expect("next suffix record").is_some() {
        count += 1;
    }

    assert_eq!(count, 111);
    suffix
        .validate_live_boundary(&end)
        .expect("actor-bound logical offset");
}

#[test]
fn observation_time_survives_delayed_append_and_is_integrity_bound() {
    let temporary = TempDir::new().expect("temporary directory");
    let path = temporary.path().join("session/events.jsonl");
    secure_directory(path.parent().expect("parent"));

    let clock = SessionClock::now().expect("clock");
    let observed = Instant::now();
    let expected = clock.observed_ns(observed);
    let mut writer = EventLogWriter::create(&path, &header(&clock), clock).expect("writer");
    writer
        .append_observed_output(
            Event::Output {
                data: ByteString::from(b"observed".as_slice()),
            },
            Some(observed),
        )
        .expect("output");
    writer
        .append(Event::Exit {
            code: Some(0),
            signal: None,
            signal_name: None,
        })
        .expect("exit");

    let log = EventLog::read(&path).expect("verify observation");
    assert_eq!(log.events[0].observed_monotonic_ns, Some(expected));
    assert!(log.events[0].monotonic_ns >= expected);

    let bytes = fs::read_to_string(&path).expect("read");
    fs::write(
        &path,
        bytes.replacen(
            &format!("\"observed_monotonic_ns\":{expected}"),
            "\"observed_monotonic_ns\":0",
            1,
        ),
    )
    .expect("tamper");

    assert!(
        EventLog::read(&path).is_err(),
        "observation time is covered by the event hash"
    );
}

#[test]
fn distinguishes_observed_and_durable_boundaries() {
    let temporary = TempDir::new().expect("temporary directory");
    let path = temporary.path().join("session/events.jsonl");
    secure_directory(path.parent().expect("parent"));

    let clock = SessionClock::now().expect("valid clock");
    let mut writer = EventLogWriter::create_with_policy(
        &path,
        &header(&clock),
        clock,
        DurabilityPolicy {
            mode: DurabilityMode::Throughput,
            maximum_interval_ms: None,
            maximum_uncommitted_bytes: None,
        },
    )
    .expect("create log");
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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn arbitrary_output_bytes_round_trip_with_contiguous_hashes(
        payloads in collection::vec(
            collection::vec(any::<u8>(), 0..64),
            0..16,
        ),
    ) {
        let temporary = TempDir::new().expect("temporary directory");
        let path = temporary.path().join("session/events.jsonl");
        secure_directory(path.parent().expect("parent"));
        let clock = SessionClock::now().expect("valid clock");
        let mut writer =
            EventLogWriter::create(&path, &header(&clock), clock).expect("create event log");
        let header_hash = writer.current_hash();
        for payload in &payloads {
            writer
                .append(Event::Output {
                    data: ByteString::from(payload.as_slice()),
                })
                .expect("append arbitrary output");
        }
        let final_hash = writer.current_hash();
        drop(writer);

        let log = EventLog::read(&path).expect("read arbitrary output log");
        prop_assert_eq!(log.events.len(), payloads.len());
        let mut previous_hash = header_hash;
        for (index, (record, expected)) in log.events.iter().zip(&payloads).enumerate() {
            prop_assert_eq!(record.seq, index as u64);
            prop_assert_eq!(&record.prev_hash, &previous_hash);
            let Event::Output { data } = &record.event else {
                prop_assert!(false, "writer changed an output event variant");
                continue;
            };
            prop_assert_eq!(data.as_bytes(), expected.as_slice());
            previous_hash.clone_from(&record.hash);
        }
        prop_assert_eq!(previous_hash, final_hash);
    }
}
