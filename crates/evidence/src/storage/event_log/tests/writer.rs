use super::{
    super::segments::SYNC_DELAY,
    support::{header, secure_directory},
};
use crate::{
    event_log::{
        DurabilityMode, DurabilityPolicy, EventLog, EventLogCursor, EventLogSummary,
        EventLogWriter, SessionClock,
    },
    Error,
};
use nix::libc::{EIO, ENOSPC};
use proptest::{collection, prelude::*};
use std::{
    fs::{self, Permissions},
    io,
    os::unix::fs::PermissionsExt as _,
    sync::mpsc,
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

    writer
        .await_durable()
        .expect("requested synchronization completes");

    assert_eq!(writer.durable_boundary(), writer.observed_boundary());

    let record = writer.durability_record();
    assert!(record.maximum_observed_uncommitted_ns >= 1_000_000);
    assert!(record.final_sync_completed);
}

fn policy_writer(temporary: &TempDir, name: &str, policy: DurabilityPolicy) -> EventLogWriter {
    let directory = temporary.path().join(name);
    secure_directory(&directory);
    let path = directory.join("events.jsonl");
    let clock = SessionClock::now().expect("valid clock");

    return EventLogWriter::create_with_policy(&path, &header(&clock), clock, policy)
        .expect("create policy writer");
}

fn marker(label: &str) -> Event {
    return Event::Marker {
        operation_id: Uuid::now_v7(),
        label: label.to_owned(),
    };
}

#[test]
fn deferred_events_become_durable_with_the_next_synchronized_append() {
    let temporary = TempDir::new().expect("temporary directory");
    let mut writer = policy_writer(
        &temporary,
        "deferred",
        DurabilityPolicy {
            mode: DurabilityMode::Throughput,
            maximum_interval_ms: None,
            maximum_uncommitted_bytes: None,
        },
    );
    writer
        .append_deferred(marker("deferred"))
        .expect("deferred marker");

    assert_eq!(writer.observed_boundary().next_event_seq, 1);
    assert_eq!(writer.durable_boundary().next_event_seq, 0);

    writer
        .append(marker("synchronized"))
        .expect("synchronized marker");

    assert_eq!(writer.durable_boundary(), writer.observed_boundary());
    assert_eq!(writer.durable_boundary().next_event_seq, 2);
}

#[test]
fn deferred_events_follow_strict_and_bounded_policies() {
    let temporary = TempDir::new().expect("temporary directory");
    let mut strict = policy_writer(
        &temporary,
        "strict-deferred",
        DurabilityPolicy {
            mode: DurabilityMode::Strict,
            maximum_interval_ms: None,
            maximum_uncommitted_bytes: None,
        },
    );
    strict
        .append_deferred(marker("strict"))
        .expect("strict deferred marker");

    assert_eq!(strict.durable_boundary().next_event_seq, 1);

    let mut bounded = policy_writer(
        &temporary,
        "bounded-deferred",
        DurabilityPolicy {
            mode: DurabilityMode::Bounded,
            maximum_interval_ms: Some(1),
            maximum_uncommitted_bytes: Some(1024),
        },
    );
    bounded
        .append_deferred(marker("bounded"))
        .expect("bounded deferred marker");

    assert_eq!(bounded.durable_boundary().next_event_seq, 0);

    thread::sleep(Duration::from_millis(5));

    assert!(bounded.flush_due().expect("flush due deferred event"));

    bounded
        .await_durable()
        .expect("requested synchronization completes");

    assert_eq!(bounded.durable_boundary(), bounded.observed_boundary());
}

#[test]
fn slow_synchronization_delays_neither_output_timestamps_nor_durable_waits() {
    const DELAY: Duration = Duration::from_millis(400);

    let temporary = TempDir::new().expect("temporary directory");
    SYNC_DELAY.set(DELAY);

    let mut writer = policy_writer(
        &temporary,
        "slow-sync",
        DurabilityPolicy {
            mode: DurabilityMode::Bounded,
            maximum_interval_ms: Some(10),
            maximum_uncommitted_bytes: Some(1024 * 1024),
        },
    );

    SYNC_DELAY.set(Duration::ZERO);

    // Reads keep arriving on their own thread while storage synchronizes, as PTY reads do.
    let (reads, received) = mpsc::channel();
    let reader = thread::spawn(move || {
        for index in 0..8 {
            reads
                .send((Instant::now(), format!("read {index}")))
                .expect("deliver read");

            thread::sleep(Duration::from_millis(25));
        }
    });

    for (observed, text) in received {
        writer
            .append_observed_output(
                Event::Output {
                    data: ByteString::from(text.into_bytes()),
                },
                Some(observed),
            )
            .expect("append read");
        writer.flush_due().expect("request due synchronization");
    }

    reader.join().expect("reader thread");

    let started = Instant::now();
    writer.append(marker("durable")).expect("durable marker");

    assert!(
        started.elapsed() >= DELAY,
        "a durable append returns only after its synchronization"
    );
    assert_eq!(writer.durable_boundary(), writer.observed_boundary());

    let path = writer.path().to_path_buf();
    drop(writer);

    let log = EventLog::read(&path).expect("read slow-sync log");
    let lag = log
        .events
        .iter()
        .filter_map(|record| {
            return record
                .observed_monotonic_ns
                .map(|observed| return record.monotonic_ns - observed);
        })
        .max()
        .expect("output records");
    assert!(
        u128::from(lag) < DELAY.as_nanos() / 2,
        "output was timestamped {lag} ns after it was read"
    );
}

#[test]
fn idle_bounded_flush_keeps_polling_in_flight_and_reports_storage_failure() {
    const DELAY: Duration = Duration::from_millis(400);

    let temporary = TempDir::new().expect("temporary directory");
    SYNC_DELAY.set(DELAY);

    let mut writer = policy_writer(
        &temporary,
        "idle-failed-sync",
        DurabilityPolicy {
            mode: DurabilityMode::Bounded,
            maximum_interval_ms: Some(1),
            maximum_uncommitted_bytes: Some(1024),
        },
    );

    SYNC_DELAY.set(Duration::ZERO);

    writer
        .append_output(Event::Output {
            data: ByteString::from(&b"x"[..]),
        })
        .expect("buffer the last output before the child idles");
    let observed = writer.observed_boundary();
    let durable = writer.durable_boundary();
    let pending_deadline = writer.flush_deadline().expect("pending output deadline");
    thread::sleep(pending_deadline.saturating_duration_since(Instant::now()));

    assert!(writer
        .flush_due()
        .expect("request asynchronous synchronization"));

    // No more output is appended: a Sync request must itself keep the owner awake.
    let in_flight_deadline = writer
        .flush_deadline()
        .expect("in-flight synchronization still has a wakeup deadline");
    assert!(in_flight_deadline > pending_deadline);

    thread::sleep(in_flight_deadline.saturating_duration_since(Instant::now()));

    writer.observe_durable();

    assert_eq!(writer.durable_boundary(), durable);
    assert!(!writer
        .flush_due()
        .expect("poll without requesting another Sync"));

    let next_deadline = writer.flush_deadline().expect("keep polling the same Sync");
    assert!(next_deadline > in_flight_deadline);

    writer.inject_storage_failure(io::Error::from_raw_os_error(ENOSPC));
    // The actor adopts durable state before controls, then calls flush_due. Adoption must
    // not consume a failure, even if the next wakeup deadline has not elapsed yet.
    writer.observe_durable();
    let error = writer
        .flush_due()
        .expect_err("idle storage failure must surface");
    let Error::Filesystem { path, source } = error else {
        panic!("expected the original filesystem failure, got {error:?}");
    };

    assert_eq!(path, writer.path());
    assert_eq!(source.raw_os_error(), Some(ENOSPC));
    assert_eq!(writer.observed_boundary(), observed);
    assert_eq!(writer.durable_boundary(), durable);
    assert!(matches!(
        writer.flush_due(),
        Err(Error::EventLogPoisoned { .. })
    ));
}

#[test]
fn flush_due_reports_storage_failure_even_without_a_deadline() {
    let temporary = TempDir::new().expect("temporary directory");

    for (name, policy) in [
        (
            "strict-failure",
            DurabilityPolicy {
                mode: DurabilityMode::Strict,
                maximum_interval_ms: None,
                maximum_uncommitted_bytes: None,
            },
        ),
        (
            "bounded-failure",
            DurabilityPolicy {
                mode: DurabilityMode::Bounded,
                maximum_interval_ms: Some(1000),
                maximum_uncommitted_bytes: Some(1024),
            },
        ),
        (
            "throughput-failure",
            DurabilityPolicy {
                mode: DurabilityMode::Throughput,
                maximum_interval_ms: None,
                maximum_uncommitted_bytes: None,
            },
        ),
    ] {
        let throughput = policy.mode == DurabilityMode::Throughput;
        let mut writer = policy_writer(&temporary, name, policy);
        if throughput {
            // This exceeds the encoded buffer threshold and queues Write without Sync.
            writer
                .append_output(Event::Output {
                    data: ByteString::from(vec![b'x'; 128 * 1024]),
                })
                .expect("queue an unsynchronized throughput write");
            assert_ne!(writer.observed_boundary(), writer.durable_boundary());
        }

        assert_eq!(writer.flush_deadline(), None);
        assert!(!writer.flush_due().expect("healthy idle writer"));
        writer.inject_storage_failure(io::Error::from_raw_os_error(EIO));
        writer.observe_durable();
        assert_eq!(writer.flush_deadline(), None);
        let error = writer
            .flush_due()
            .expect_err("no deadline must not hide a failure");
        let Error::Filesystem { path, source } = error else {
            panic!("expected the original filesystem failure, got {error:?}");
        };
        assert_eq!(path, writer.path());
        assert_eq!(source.raw_os_error(), Some(EIO));
        assert!(matches!(
            writer.flush_due(),
            Err(Error::EventLogPoisoned { .. })
        ));
    }
}

#[test]
fn durable_boundary_moves_only_when_the_owner_adopts_completed_synchronization() {
    let temporary = TempDir::new().expect("temporary directory");
    let mut writer = policy_writer(
        &temporary,
        "adopted-sync",
        DurabilityPolicy {
            mode: DurabilityMode::Bounded,
            maximum_interval_ms: Some(1),
            maximum_uncommitted_bytes: Some(1024),
        },
    );
    writer
        .append_output(Event::Output {
            data: ByteString::from(&b"x"[..]),
        })
        .expect("append output");
    let before = writer.durable_boundary();

    thread::sleep(Duration::from_millis(5));

    assert!(writer.flush_due().expect("request synchronization"));

    let deadline = Instant::now() + Duration::from_secs(10);

    while writer.durability_record().sync_count == 0 {
        assert!(
            Instant::now() < deadline,
            "requested synchronization completes"
        );

        thread::sleep(Duration::from_millis(5));
    }

    // Reads between adoptions agree even though the storage thread has finished.
    assert_eq!(writer.durable_boundary(), before);
    assert!(writer.flush_deadline().is_some());

    writer.observe_durable();

    assert_eq!(writer.durable_boundary(), writer.observed_boundary());
    assert_eq!(writer.flush_deadline(), None);
}

#[test]
fn deferred_append_rejects_output() {
    let temporary = TempDir::new().expect("temporary directory");
    let mut writer = policy_writer(
        &temporary,
        "deferred-output",
        DurabilityPolicy {
            mode: DurabilityMode::Throughput,
            maximum_interval_ms: None,
            maximum_uncommitted_bytes: None,
        },
    );
    assert!(writer
        .append_deferred(Event::Output {
            data: ByteString::from(&b"x"[..]),
        })
        .is_err());
    assert_eq!(writer.observed_boundary().next_event_seq, 0);
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
