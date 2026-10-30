use super::engine::{inspect_latest_presented_fast, inspect_latest_presented_fast_with_limits};
use super::*;
use proptest::{collection, prelude::*};
use std::sync::{atomic::AtomicBool, Arc};
use termtrace_evidence::{
    artifact::create_private_directory,
    event_log::{EventLog, EventLogWriter, SessionClock},
};
use termtrace_model::{
    execution::event::{Event, EventLogHeader, EventRecord, OperationStatus},
    terminal::state::{DirtyState, WideKind},
    value::ByteString,
};
use termtrace_terminal::GhosttyVt;
use uuid::Uuid;

fn output_log(data: &[u8]) -> EventLog {
    return EventLog {
        header: EventLogHeader::new(
            Uuid::now_v7(),
            "0".repeat(64),
            0,
            80,
            24,
            800,
            480,
            "xterm-ghostty",
        ),
        events: vec![EventRecord {
            observed_monotonic_ns: None,
            seq: 0,
            monotonic_ns: 1,
            prev_hash: "0".repeat(64),
            hash: "1".repeat(64),
            event: Event::Output {
                data: ByteString::from(data),
            },
        }],
        pending_operations: Vec::new(),
        source_path: None,
    };
}

fn output_log_with_geometry(data: &[u8], cols: u16, rows: u16) -> EventLog {
    let mut log = output_log(data);
    log.header.cols = cols;
    log.header.rows = rows;
    log.header.pixel_width = cols.saturating_mul(8);
    log.header.pixel_height = rows.saturating_mul(16);

    return log;
}

fn event_log(cols: u16, rows: u16, events: Vec<Event>) -> EventLog {
    let mut log = output_log_with_geometry(b"", cols, rows);
    log.events = events
        .into_iter()
        .zip(0_u64..)
        .map(|(event, seq)| {
            return EventRecord {
                observed_monotonic_ns: None,
                seq,
                monotonic_ns: seq.saturating_add(1),
                prev_hash: "0".repeat(64),
                hash: "1".repeat(64),
                event,
            };
        })
        .collect();

    return log;
}

fn output(data: &[u8]) -> Event {
    return Event::Output {
        data: ByteString::from(data),
    };
}

fn resize(cols: u16, rows: u16) -> [Event; 2] {
    let operation_id = Uuid::now_v7();

    return [
        Event::ResizeIntent {
            operation_id,
            cols,
            rows,
            pixel_width: cols.saturating_mul(8),
            pixel_height: rows.saturating_mul(16),
        },
        Event::ResizeResult {
            operation_id,
            status: OperationStatus::Applied,
            error_code: None,
        },
    ];
}

// Checkpoints every state of a full replay, then resumes from each one. The resumed replay must
// emit exactly the canonical records after that checkpoint and reach the same final state.
fn assert_every_checkpoint_resumes_canonically(log: &EventLog) {
    let mut canonical = Vec::new();
    let mut seeds = Vec::new();
    let artifacts = replay_with(log, None, |record, _, engine| {
        seeds.push(ReplaySeed {
            record: record.clone(),
            checkpoint: engine
                .encode_checkpoint()?
                .expect("replay engines track unfinished input"),
        });

        canonical.push(record);

        return Ok(());
    })
    .expect("canonical replay");

    for seed in seeds {
        let start = usize::try_from(seed.record.state_id).expect("state index");
        let mut resumed = Vec::new();
        let seeded =
            replay_with_seed_and_limits(log, None, seed, DEFAULT_REPLAY_LIMITS, |record, _, _| {
                resumed.push(record);

                return Ok(());
            })
            .expect("checkpoint replay");

        assert_eq!(resumed, canonical[start..], "resumed from state {start}");
        assert_eq!(seeded.final_state_hash, artifacts.final_state_hash);
        assert_eq!(seeded.state_count, artifacts.state_count);
    }
}

#[test]
fn explicit_live_prefix_excludes_later_output_and_hidden_sync_bytes() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let session = directory.path().join("session");
    create_private_directory(&session).expect("private session");
    let path = session.join("events.jsonl");
    let clock = SessionClock::now().expect("clock");
    let mut header = output_log(b"").header;
    header.started_unix_ns = clock.started_unix_ns();
    let mut writer = EventLogWriter::create(&path, &header, clock).expect("canonical writer");
    writer
        .append(Event::Output {
            data: ByteString::from(b"OLD\x1b[?2026hHIDDEN".as_slice()),
        })
        .expect("first output");
    let boundary = writer.durable_boundary();
    writer
        .append(Event::Output {
            data: ByteString::from(b"\x1b[?2026lNEW".as_slice()),
        })
        .expect("later output");

    let (_, endpoint) = inspect_latest_presented_prefix(&path, &boundary, &AtomicBool::new(false))
        .expect("frozen prefix");

    let expected =
        inspect_latest_presented_fast(&output_log(b"OLD")).expect("expected visible state");
    assert_eq!(endpoint.state_hash, expected.state_hash);
    assert_eq!(endpoint.event_seq, Some(0));
    assert_eq!(endpoint.event_byte_offset, Some(2));

    let mut wrong_bytes = boundary.clone();
    wrong_bytes.output_bytes += 1;

    assert!(inspect_latest_presented_prefix(&path, &wrong_bytes, &AtomicBool::new(false)).is_err());

    let mut wrong_hash = boundary.clone();
    wrong_hash.event_hash = "0".repeat(64);

    assert!(inspect_latest_presented_prefix(&path, &wrong_hash, &AtomicBool::new(false)).is_err());
    assert!(inspect_latest_presented_prefix(&path, &boundary, &AtomicBool::new(true)).is_err());
}

#[test]
fn fast_endpoint_replay_charges_only_input_output_bytes() {
    let log = output_log_with_geometry(b"x", 200, 100);

    reset_test_replay_output_bytes();

    let inspection = inspect_latest_presented_fast(&log).expect("fast endpoint replay");
    assert_eq!(inspection.state.cols, 200);
    assert_eq!(inspection.state.rows, 100);
    assert_eq!(
        test_replay_output_bytes(),
        1,
        "fast endpoint replay must charge the input once without state-record replay"
    );
}

#[test]
fn fast_endpoint_budgets_deferred_snapshots_across_many_output_records() {
    let mut log = output_log_with_geometry(b"", 200, 100);
    log.events = (0_u64..256)
        .map(|seq| {
            return EventRecord {
                observed_monotonic_ns: None,
                seq,
                monotonic_ns: seq.saturating_add(1),
                prev_hash: "0".repeat(64),
                hash: "1".repeat(64),
                event: Event::Output {
                    data: ByteString::from(b"x".as_slice()),
                },
            };
        })
        .collect();

    let limits = ReplayLimits {
        output_bytes: 256,
        capture_attempts: 2,
        cell_work: 40_000,
        ..DEFAULT_REPLAY_LIMITS
    };
    let inspection =
        inspect_latest_presented_fast_with_limits(&log, limits).expect("bounded fast replay");
    assert_eq!(inspection.event_seq, Some(255));
    assert_eq!(inspection.event_byte_offset, Some(0));
    assert_eq!(inspection.monotonic_ns, Some(256));

    let error = inspect_latest_presented_fast_with_limits(
        &log,
        ReplayLimits {
            cell_work: 39_999,
            ..limits
        },
    )
    .expect_err("each unavoidable full snapshot must consume viewport cell work");
    assert!(matches!(error, Error::InvalidReplay { .. }));
}

#[test]
fn fast_endpoint_matches_byte_granular_latest_presented_state() {
    let log = output_log(b"before\x1b[?2026hhidden\x1b[?2026l-after\x1b[2;4Hdone");
    let fast = inspect_latest_presented_fast(&log).expect("fast endpoint replay");
    let canonical =
        inspect(&log, StateSelector::LatestPresented).expect("canonical latest presented state");
    assert_eq!(fast.state_hash, canonical.record.state_hash);
    assert_eq!(fast.event_seq, canonical.record.event_seq);
    assert_eq!(fast.event_byte_offset, canonical.record.event_byte_offset);
    assert_eq!(fast.monotonic_ns, Some(canonical.record.monotonic_ns));
}

#[test]
fn fast_endpoint_retains_source_cursor_for_trailing_buffered_output() {
    let log = output_log(b"BUNDLE");
    let fast = inspect_latest_presented_fast(&log).expect("fast endpoint replay");
    let canonical =
        inspect(&log, StateSelector::LatestPresented).expect("canonical latest presented state");
    assert_eq!(fast.state_hash, canonical.record.state_hash);
    assert_eq!(fast.event_seq, canonical.record.event_seq);
    assert_eq!(fast.event_byte_offset, canonical.record.event_byte_offset);
    assert_eq!(fast.monotonic_ns, Some(canonical.record.monotonic_ns));
}

#[test]
fn fast_endpoint_excludes_uncommitted_synchronized_output() {
    let log = output_log(b"visible\x1b[?2026hhidden");
    let fast = inspect_latest_presented_fast(&log).expect("fast endpoint replay");
    let canonical =
        inspect(&log, StateSelector::LatestPresented).expect("canonical latest presented state");
    assert_eq!(fast.state_hash, canonical.record.state_hash);
    assert!(!fast.state.modes.synchronized_output);
}

#[test]
fn fast_endpoint_keeps_the_frame_from_the_start_of_a_hold() {
    let log = output_log(b"shown\x1b[?2026;1049hhidden");
    let fast = inspect_latest_presented_fast(&log).expect("fast endpoint replay");
    assert_eq!(
        termtrace_terminal::semantic_viewport_text(&fast.state).trim_end(),
        "shown"
    );
    assert!(!fast.state.modes.alternate_screen);
    assert_eq!(fast.event_byte_offset, Some(4));
}

#[test]
fn fast_endpoint_presents_again_after_a_reset_ends_a_hold() {
    let log = output_log(b"kept\x1b[?2026hlost\x1bcafter");
    let fast = inspect_latest_presented_fast(&log).expect("fast endpoint replay");
    assert_eq!(
        termtrace_terminal::semantic_viewport_text(&fast.state).trim_end(),
        "after"
    );
    assert!(!fast.state.modes.synchronized_output);
    assert_eq!(fast.event_byte_offset, Some(22));
}

#[test]
fn replay_enforces_output_and_capture_work_budgets() {
    let log = output_log(b"abc");
    let output_error = replay_with_limits(
        &log,
        None,
        ReplayLimits {
            output_bytes: 2,
            ..DEFAULT_REPLAY_LIMITS
        },
        |_, _, _| return Ok(()),
    )
    .expect_err("output budget must reject before byte replay");
    assert!(matches!(output_error, Error::InvalidReplay { .. }));

    let capture_error = replay_with_limits(
        &log,
        None,
        ReplayLimits {
            capture_attempts: 1,
            ..DEFAULT_REPLAY_LIMITS
        },
        |_, _, _| return Ok(()),
    )
    .expect_err("capture budget must reject byte-granular amplification");
    assert!(matches!(capture_error, Error::InvalidReplay { .. }));

    let cell_work_error = replay_with_limits(
        &log,
        None,
        ReplayLimits {
            cell_work: 1_919,
            ..DEFAULT_REPLAY_LIMITS
        },
        |_, _, _| return Ok(()),
    )
    .expect_err("cell work must be charged before snapshot capture");
    assert!(matches!(cell_work_error, Error::InvalidReplay { .. }));
}

#[test]
fn validated_checkpoint_seed_skips_long_session_canonical_output_prefix() {
    let data = vec![b'x'; 2 * 1_024];
    let log = output_log(&data);
    let checkpoint_offset = 1_024;
    let mut seed = None;

    replay_with(&log, None, |record, _, engine| {
        if record.event_byte_offset == Some(checkpoint_offset) {
            seed = Some(ReplaySeed {
                record,
                checkpoint: engine.encode_checkpoint()?.expect("checkpoint available"),
            });
        }

        return Ok(());
    })
    .expect("checkpoint source replay");

    let seed = seed.expect("periodic checkpoint state");
    let remaining = u64::try_from(data.len())
        .expect("output length")
        .saturating_sub(checkpoint_offset.saturating_add(1));

    let mut final_hash = None;
    let artifacts = replay_with_seed_and_limits(
        &log,
        None,
        seed,
        ReplayLimits {
            output_bytes: remaining,
            ..DEFAULT_REPLAY_LIMITS
        },
        |record, _, _| {
            final_hash = Some(record.state_hash);

            return Ok(());
        },
    )
    .expect("checkpoint replay within suffix-only output budget");

    let canonical = replay(&log).expect("canonical replay");
    assert_eq!(
        artifacts.final_state_hash,
        canonical.artifacts.final_state_hash
    );
    assert_eq!(final_hash, Some(canonical.artifacts.final_state_hash));
}

#[test]
fn checkpoints_resume_alternate_screen_pending_wrap_and_unfinished_input() {
    assert_every_checkpoint_resumes_canonically(&output_log_with_geometry(
        b"main-screen\x1b[?1049halternate\x1b[?1049l-after",
        12,
        3,
    ));

    assert_every_checkpoint_resumes_canonically(&event_log(
        4,
        3,
        vec![
            output(b"abcd"),
            output(b"e\x1b[3"),
            output(b"1mred \xe2\x82"),
            output(b"\xac\x1b]2;split"),
            output(b"-title\x07\x1b]4;1;rgb:12/34/56\x07\x1b]10;rgb:aa/bb/cc\x07"),
            output(b"\x1b]11;rgb:01/02/03\x07\x1b[31mX\x1b]110\x07Y"),
        ],
    ));
}

#[test]
fn checkpoints_resume_across_resizes_and_scrollback() {
    let mut events = vec![output(b"one\r\ntwo\r\nthree\r\nfour\r\nfive")];
    events.extend(resize(6, 2));
    events.push(output(b"\r\nsix"));
    events.extend(resize(10, 5));
    events.push(output(b"seven"));
    assert_every_checkpoint_resumes_canonically(&event_log(8, 3, events));
}

#[test]
fn long_index_emits_periodic_validated_checkpoint_entries() {
    let mut data = b"\x1b]2;tmux-title\x1b\\\x1b]7;file:///tmp\x1b\\".to_vec();
    data.extend(vec![b'x'; 1_100]);
    let output_bytes = u64::try_from(data.len()).expect("output length");
    let mut log = output_log(&data);
    let temporary = tempfile::TempDir::new().expect("temporary directory");
    log.source_path = Some(temporary.path().join("events.jsonl"));
    let directory =
        index::write_cataloged(temporary.path(), &log).expect("periodic checkpoint index");

    let entries: Vec<StateIndexEntry> = fs_err::read_to_string(directory.join("state-index.jsonl"))
        .expect("state index")
        .lines()
        .map(|line| return serde_json::from_str(line).expect("state index entry"))
        .collect();

    let checkpoint_ids: Vec<u64> = entries
        .iter()
        .filter(|entry| return entry.checkpoint)
        .map(|entry| return entry.state_id)
        .collect();
    assert!(
        checkpoint_ids.len() > 1,
        "multiple checkpoints must be emitted"
    );
    assert!(
        checkpoint_ids.windows(2).all(|w| return w[0] < w[1]),
        "checkpoint IDs must be ordered"
    );
    assert_eq!(checkpoint_ids.first().copied(), Some(0));
    assert!(
        checkpoint_ids
            .last()
            .copied()
            .is_some_and(|last| return last >= 1_000),
        "must include late resume point"
    );

    for entry in entries.iter().filter(|entry| return entry.checkpoint) {
        assert!(
            entry.timeline_rank.is_some()
                && entry.control_index_offset.is_some()
                && entry.control_rank.is_some(),
            "checkpoint {} must persist combined timeline and control resume metadata",
            entry.state_id
        );
    }

    assert!(directory.join("control-index.jsonl").is_file());
    assert!(directory.join("control-source-index.jsonl").is_file());

    let seed = DerivedIndex::open(&log)
        .expect("trusted index lookup")
        .expect("cataloged index")
        .nearest_seed(&StateSelector::StateId(1_099))
        .expect("validated checkpoint lookup")
        .expect("nearest late checkpoint");
    assert_eq!(
        seed.record.state_id,
        *checkpoint_ids.last().expect("last checkpoint")
    );

    let seed_offset = seed
        .record
        .event_byte_offset
        .expect("output checkpoint offset");

    let remaining_output_bytes = output_bytes.saturating_sub(seed_offset.saturating_add(1));
    let seeded = replay_with_seed_and_limits(
        &log,
        None,
        seed,
        ReplayLimits {
            output_bytes: remaining_output_bytes,
            ..DEFAULT_REPLAY_LIMITS
        },
        |_, _, _| return Ok(()),
    )
    .expect("late replay must fit the suffix-only output budget");
    assert_eq!(
        seeded.final_state_hash,
        replay(&log)
            .expect("canonical replay")
            .artifacts
            .final_state_hash
    );

    let index = DerivedIndex::open(&log)
        .expect("trusted index lookup")
        .expect("cataloged index");

    for state_id in checkpoint_ids {
        let seed = index
            .nearest_seed(&StateSelector::StateId(state_id))
            .expect("checkpoint lookup")
            .expect("checkpoint at its own state");
        assert_eq!(seed.record.state_id, state_id);

        validate_replay_seed(&log, &seed).expect("checkpoint must seed the pinned Ghostty engine");
    }
}

#[test]
fn every_periodic_checkpoint_candidate_is_written() {
    // Long unbroken lines keep the cursor in pending wrap and the alternate screen hides the
    // primary one; neither may prevent a checkpoint.
    let mut data = b"\x1b[?1049h".to_vec();
    for line in 0..40 {
        data.extend(format!("{line:04}").repeat(60).as_bytes());
    }

    let log = output_log(&data);
    let temporary = tempfile::TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("derived");
    create_private_directory(&directory).expect("private derived directory");
    let (artifacts, _) = index::write(&directory, &log, None, true, StateSelection::All)
        .expect("periodic checkpoint index");

    let checkpoint_count = fs_err::read_to_string(directory.join("state-index.jsonl"))
        .expect("state index")
        .lines()
        .map(|line| return serde_json::from_str::<StateIndexEntry>(line).expect("index entry"))
        .filter(|entry| return entry.checkpoint)
        .count();
    assert_eq!(
        u64::try_from(checkpoint_count).expect("checkpoint count"),
        artifacts.state_count.div_ceil(128)
    );
}

#[test]
fn semantic_hash_v2_binds_wrap_and_wide_but_not_dirtiness() {
    let mut terminal = GhosttyVt::new(4, 3, 0).expect("terminal");
    terminal.write("abc界".as_bytes());
    let state = terminal.snapshot().expect("semantic state");
    let baseline = semantic_state_hash(&state).expect("baseline hash");
    let mut wrap_changed = state.clone();
    wrap_changed.grid[0].soft_wrapped = !wrap_changed.grid[0].soft_wrapped;

    assert_ne!(
        baseline,
        semantic_state_hash(&wrap_changed).expect("wrap hash")
    );

    let mut wide_changed = state.clone();
    Arc::make_mut(&mut wide_changed.grid[0].cells)[0].wide_kind = WideKind::SpacerHead;

    assert_ne!(
        baseline,
        semantic_state_hash(&wide_changed).expect("wide hash")
    );

    let mut dirty_changed = state;
    dirty_changed.dirty = DirtyState::Full;
    dirty_changed.dirty_rows = vec![0];
    dirty_changed.grid[0].dirty = !dirty_changed.grid[0].dirty;

    assert_eq!(
        baseline,
        semantic_state_hash(&dirty_changed).expect("dirty hash")
    );
}

#[test]
fn row_delta_reports_only_edited_cells_in_column_order() {
    let mut terminal = GhosttyVt::new(4, 2, 0).expect("terminal");
    terminal.write(b"abc\r\ndef");
    let previous = terminal.snapshot().expect("semantic state");
    let mut first = previous.grid[0].cells[0].clone();
    first.grapheme = "x".into();
    let mut third = previous.grid[0].cells[2].clone();
    third.style.bold = true;
    let mut current = previous.clone();
    let cells = Arc::make_mut(&mut current.grid[0].cells);
    cells[2] = third.clone();
    cells[0] = first.clone();

    let delta = between(Some(&previous), &current);
    assert_eq!(
        delta,
        StateDelta {
            geometry: None,
            cursor: None,
            modes: None,
            colors: None,
            active_style: None,
            metadata: None,
            rows: vec![RowDiff {
                row: 0,
                soft_wrapped: false,
                wrap_continuation: false,
                cells: vec![first, third],
            }],
        }
    );
}

#[test]
fn row_delta_omits_equal_cells_loaded_from_json() {
    let mut terminal = GhosttyVt::new(4, 2, 0).expect("terminal");
    terminal.write("界x".as_bytes());
    let previous = terminal.snapshot().expect("semantic state");
    let bytes = serde_json::to_vec(&previous).expect("serialize semantic state");
    let restored = serde_json::from_slice(&bytes).expect("deserialize semantic state");
    let delta = between(Some(&previous), &restored);
    assert!(delta.rows.is_empty());
}

#[test]
fn row_delta_emits_and_reconstructs_wrap_facts_without_cell_changes() {
    let mut terminal = GhosttyVt::new(2, 2, 0).expect("terminal");
    terminal.write(b"x");
    let previous = terminal.snapshot().expect("semantic state");
    let mut current = previous.clone();
    current.grid[0].soft_wrapped = !current.grid[0].soft_wrapped;
    current.grid[0].wrap_continuation = !current.grid[0].wrap_continuation;
    let delta = between(Some(&previous), &current);
    assert_eq!(delta.rows.len(), 1);
    assert!(delta.rows[0].cells.is_empty());
    assert_eq!(delta.rows[0].soft_wrapped, current.grid[0].soft_wrapped);
    assert_eq!(
        delta.rows[0].wrap_continuation,
        current.grid[0].wrap_continuation
    );

    let mut reconstructed = previous;
    for row_diff in delta.rows {
        let row = &mut reconstructed.grid[usize::from(row_diff.row)];
        row.soft_wrapped = row_diff.soft_wrapped;
        row.wrap_continuation = row_diff.wrap_continuation;

        for cell in row_diff.cells {
            let column = usize::from(cell.column);
            Arc::make_mut(&mut row.cells)[column] = cell;
        }
    }

    assert_eq!(
        semantic_state_hash(&reconstructed).expect("reconstructed hash"),
        semantic_state_hash(&current).expect("current hash")
    );
}

const CHECKPOINT_FRAGMENTS: &[&[u8]] = &[
    b"x",
    b"wide\xe7\x95\x8c",
    b"\r\n",
    b"\xe2\x82",
    b"\xac",
    b"\x1b[3",
    b"1m",
    b"\x1b[?1049h",
    b"\x1b[?1049l",
    b"\x1b]2;title",
    b"\x07",
    b"\x1b",
    b"]10;rgb:11/22/33\x07",
    b"\x1b]4;2;rgb:44/55/66\x07",
    b"\x1bP+q544e",
    b"\x1b\\",
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    #[test]
    fn arbitrary_checkpoint_resumes_the_canonical_state_sequence(
        chunks in collection::vec(
            collection::vec(0..CHECKPOINT_FRAGMENTS.len(), 1..6),
            1..5,
        ),
    ) {
        let events = chunks
            .iter()
            .map(|fragments| {
                return output(&fragments
                    .iter()
                    .flat_map(|&index| return CHECKPOINT_FRAGMENTS[index].iter().copied())
                    .collect::<Vec<_>>());
            })
            .collect();

        assert_every_checkpoint_resumes_canonically(&event_log(6, 3, events));
    }
}
