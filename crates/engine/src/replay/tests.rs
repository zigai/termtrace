use std::str::from_utf8;

use proptest::{collection, prelude::*};
use uuid::Uuid;
use zstd::stream::decode_all;

use super::engine::{inspect_latest_presented_fast, inspect_latest_presented_fast_with_limits};
use super::*;
use termtrace_evidence::{artifact::create_private_directory, event_log::EventLog};
use termtrace_model::{
    execution::event::{Event, EventLogHeader, EventRecord},
    terminal::state::{DirtyState, WideKind},
    value::ByteString,
};
use termtrace_terminal::GhosttyVt;

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
    replay_with(&log, None, |record, state, engine| {
        if record.event_byte_offset == Some(checkpoint_offset) {
            seed = Some(ReplaySeed {
                inspection: StateInspection {
                    record,
                    state: state.clone(),
                },
                engine_vt: engine
                    .format_replay_seed()?
                    .expect("serializable primary-screen checkpoint"),
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
fn alternate_screen_checkpoint_seed_is_rejected_before_replay() {
    let data = b"main-screen\x1b[?1049halternate\x1b[?1049l-after";
    let log = output_log(data);
    let checkpoint_offset = u64::try_from(
        data.windows(b"alternate".len())
            .position(|window| return window == b"alternate")
            .expect("alternate payload"),
    )
    .expect("checkpoint offset")
    .saturating_add(u64::try_from(b"alternate".len()).expect("payload length"))
    .saturating_sub(1);
    let mut seed = None;
    replay_with(&log, None, |record, state, engine| {
        if record.event_byte_offset == Some(checkpoint_offset) {
            seed = Some(ReplaySeed {
                inspection: StateInspection {
                    record,
                    state: state.clone(),
                },
                engine_vt: engine
                    .format_replay_seed()?
                    .expect("serializable alternate-screen checkpoint"),
            });
        }
        return Ok(());
    })
    .expect("alternate-screen checkpoint source replay");
    let error = replay_with_seed_and_limits(
        &log,
        None,
        seed.expect("alternate-screen checkpoint"),
        DEFAULT_REPLAY_LIMITS,
        |_, _, _| return Ok(()),
    )
    .expect_err("alternate-screen seed cannot reconstruct hidden primary-screen state");
    assert!(matches!(error, Error::CheckpointReconstruction { .. }));
}

#[test]
fn long_index_emits_periodic_validated_checkpoint_entries() {
    let mut data = b"\x1b]2;tmux-title\x1b\\\x1b]7;file:///tmp\x1b\\".to_vec();
    data.extend(vec![b'x'; 1_100]);
    let output_bytes = u64::try_from(data.len()).expect("output length");
    let log = output_log(&data);
    let temporary = tempfile::TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("derived");
    create_private_directory(&directory).expect("private derived directory");
    index::write_index(&directory, &log, None, true, StateSelection::All)
        .expect("periodic checkpoint index");
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
    let seed = load_nearest_replay_seed(&directory, &log, &StateSelector::StateId(1_099))
        .expect("validated checkpoint lookup")
        .expect("nearest late checkpoint");
    assert_eq!(
        seed.inspection.record.state_id,
        *checkpoint_ids.last().expect("last checkpoint")
    );
    let seed_offset = seed
        .inspection
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
    let compressed =
        fs_err::read(directory.join("checkpoints.jsonl.zst")).expect("compressed checkpoints");
    let checkpoint_text = decode_all(compressed.as_slice()).expect("checkpoint stream");
    let checkpoints: Vec<SemanticReplayCheckpoint> = from_utf8(&checkpoint_text)
        .expect("checkpoint UTF-8")
        .lines()
        .map(|line| return serde_json::from_str(line).expect("semantic checkpoint"))
        .collect();
    for checkpoint in checkpoints {
        validate_replay_seed(
            &log,
            &ReplaySeed {
                inspection: checkpoint.inspection,
                engine_vt: checkpoint.engine_vt.into_bytes(),
            },
        )
        .expect("checkpoint must seed the pinned Ghostty engine");
    }
}

#[test]
fn periodic_checkpoint_reconstructs_scrolling_grid() {
    let mut data: Vec<u8> = Vec::new();
    for line in 0..400 {
        data.extend(format!("scrolling-line-{line:04}\r\n").as_bytes());
    }
    let log = output_log(&data);
    let temporary = tempfile::TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("derived");
    create_private_directory(&directory).expect("private derived directory");
    index::write_index(&directory, &log, None, true, StateSelection::All)
        .expect("scrolling grids must produce validated periodic checkpoints");
    let checkpoint_count = fs_err::read_to_string(directory.join("state-index.jsonl"))
        .expect("state index")
        .lines()
        .map(|line| return serde_json::from_str::<StateIndexEntry>(line).expect("index entry"))
        .filter(|entry| return entry.checkpoint)
        .count();
    assert!(
        checkpoint_count > 1,
        "scrolling replay must retain usable checkpoints beyond the initial state"
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
    wide_changed.grid[0].cells[0].wide_kind = WideKind::SpacerHead;
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
fn row_delta_emits_and_reconstructs_wrap_facts_without_cell_changes() {
    let mut terminal = GhosttyVt::new(2, 2, 0).expect("terminal");
    terminal.write(b"x");
    let previous = terminal.snapshot().expect("semantic state");
    let mut current = previous.clone();
    current.grid[0].soft_wrapped = !current.grid[0].soft_wrapped;
    current.grid[0].wrap_continuation = !current.grid[0].wrap_continuation;
    let delta = state_delta(Some(&previous), &current);
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
            row.cells[column] = cell;
        }
    }
    assert_eq!(
        semantic_state_hash(&reconstructed).expect("reconstructed hash"),
        semantic_state_hash(&current).expect("current hash")
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn arbitrary_replay_checkpoint_is_lossless_or_explicitly_unavailable(
        data in collection::vec(0x20_u8..0x7f, 1..128),
        checkpoint_selector in any::<usize>(),
    ) {
        let log = output_log(&data);
        let checkpoint_index = checkpoint_selector % data.len();
        let checkpoint_offset = checkpoint_index as u64;
        let mut seed = None;
        let mut unserialized_checkpoint = None;
        replay_with(&log, None, |record, state, engine| {
            if record.event_byte_offset == Some(checkpoint_offset) {
                if let Some(engine_vt) = engine.format_replay_seed()? {
                    seed = Some(ReplaySeed {
                        inspection: StateInspection {
                            record,
                            state: state.clone(),
                        },
                        engine_vt,
                    });
                } else {
                    unserialized_checkpoint = Some(state.clone());
                }
            }
            return Ok(());
        })
        .expect("checkpoint source replay");

        let Some(seed) = seed else {
            prop_assert!(unserialized_checkpoint.is_some(), "unserialized checkpoint state must be captured");
            return Ok(());
        };
        let remaining = data.len().saturating_sub(checkpoint_index.saturating_add(1)) as u64;
        let seeded = replay_with_seed_and_limits(
            &log,
            None,
            seed,
            ReplayLimits {
                output_bytes: remaining,
                ..DEFAULT_REPLAY_LIMITS
            },
            |_, _, _| return Ok(()),
        )
        .expect("checkpoint replay");
        let canonical = replay(&log).expect("canonical replay");
        prop_assert_eq!(
            seeded.final_state_hash,
            canonical.artifacts.final_state_hash
        );
        prop_assert_eq!(seeded.state_count, canonical.artifacts.state_count);
    }
}
