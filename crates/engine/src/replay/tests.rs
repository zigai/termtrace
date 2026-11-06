use super::engine::{inspect_latest_presented_fast, inspect_latest_presented_fast_with_limits};
use super::*;
use proptest::{collection, prelude::*};
use std::{
    collections::HashMap,
    slice,
    sync::{atomic::AtomicBool, Arc},
};
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

// The hash at every position, from a reference replay that writes one byte at a time and
// captures after each.
fn byte_at_a_time_hashes(log: &EventLog) -> Vec<String> {
    let mut terminal =
        GhosttyVt::new(log.header.cols, log.header.rows, 100_000).expect("reference terminal");
    terminal
        .apply_terminal_colors(&log.header.colors)
        .expect("reference colors");
    terminal
        .set_terminfo_name(&log.header.term)
        .expect("reference terminfo");

    let hash = |terminal: &mut GhosttyVt| {
        return semantic_state_hash(&terminal.snapshot().expect("reference snapshot"))
            .expect("reference hash");
    };
    let mut hashes = vec![hash(&mut terminal)];
    let mut intents = HashMap::new();

    for record in &log.events {
        match &record.event {
            Event::Output { data } => {
                for byte in data.as_bytes() {
                    terminal.write_step(slice::from_ref(byte));
                    hashes.push(hash(&mut terminal));
                }
            }
            Event::ResizeIntent {
                operation_id,
                cols,
                rows,
                ..
            } => {
                intents.insert(*operation_id, (*cols, *rows));
            }
            Event::ResizeResult {
                operation_id,
                status: OperationStatus::Applied,
                ..
            } => {
                let (cols, rows) = intents[operation_id];
                terminal
                    .resize(cols, rows, 8, 16)
                    .expect("reference resize");
                hashes.push(hash(&mut terminal));
            }
            _ => {}
        }
    }

    return hashes;
}

fn position_of(states: &[StateRecord], state_id: u64) -> usize {
    return states
        .iter()
        .position(|record| return record.state_id == state_id)
        .expect("seed is a canonical boundary state");
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
            source_position: None,
        });

        canonical.push(record);

        return Ok(());
    })
    .expect("canonical replay");

    for seed in seeds {
        let start = position_of(&canonical, seed.record.state_id);
        let mut resumed = Vec::new();
        let seeded =
            replay_with_seed_and_limits(log, None, seed, DEFAULT_REPLAY_LIMITS, |record, _, _| {
                resumed.push(record);

                return Ok(());
            })
            .expect("checkpoint replay");

        assert_eq!(resumed, canonical[start..], "resumed from state {start}");
        assert_eq!(seeded.final_state_hash, artifacts.final_state_hash);
        assert_eq!(seeded.final_state_id, artifacts.final_state_id);
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
    assert_eq!(endpoint.state_id, 10);
    assert_eq!(endpoint.event_seq, Some(0));
    assert_eq!(endpoint.event_byte_offset, Some(9));

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
    let log = output_log(b"before\x1b[?2026hhidden\x1b[?2026l-after\x1b[2;4Hdone\x1b[?2026hx");
    let fast = inspect_latest_presented_fast(&log).expect("fast endpoint replay");
    let canonical =
        inspect(&log, StateSelector::LatestPresented).expect("canonical latest presented state");
    assert_eq!(fast.state_hash, canonical.record.state_hash);
    assert_eq!(fast.state_id, canonical.record.state_id);
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
    assert_eq!(fast.state_id, canonical.record.state_id);
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
    assert_eq!(fast.state_id, canonical.record.state_id);
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
    assert_eq!(fast.state_id, 17);
    assert_eq!(fast.event_byte_offset, Some(16));
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
    assert_eq!(fast.state_id, 23);
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
    let seed = seed_at(&log, checkpoint_offset + 1);
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

// Mixed output long enough for several checkpoint segments: control sequences split across
// events, resizes, scrollback, synchronized-output holds, the alternate screen, and a screen
// drawn twice so its state hashes recur.
fn segmented_log() -> EventLog {
    let mut events = vec![output(b"\x1b]2;segmented\x07")];

    for line in 0..250_u32 {
        let mut data = format!("{line:04} {}\r\n", "x".repeat(48)).into_bytes();

        if line % 37 == 0 {
            data.extend_from_slice(b"\x1b[?2026hhidden frame\x1b[?2026l");
        }

        if line % 23 == 0 {
            data.extend_from_slice(b"\x1b[3");
        }

        if line % 23 == 1 {
            data.splice(0..0, b"1m".iter().copied());
        }

        if line == 100 {
            data.extend_from_slice(b"\x1b[?1049halternate");
        }

        if line == 150 {
            data.extend_from_slice(b"\x1b[?1049l");
        }

        if line == 180 || line == 230 {
            data.extend_from_slice(b"\x1b[0m\x1b[2J\x1b[Hsame screen");
        }

        events.push(output(&data));

        if line % 97 == 50 {
            events.extend(resize(if line % 2 == 0 { 30 } else { 40 }, 8));
        }
    }

    return event_log(40, 10, events);
}

fn indexed(log: &EventLog) -> (tempfile::TempDir, EventLog) {
    let temporary = tempfile::TempDir::new().expect("temporary directory");
    let mut log = log.clone();
    log.source_path = Some(temporary.path().join("events.jsonl"));
    index::write_cataloged(temporary.path(), &log).expect("cataloged index");

    return (temporary, log);
}

fn seed_at(log: &EventLog, position: u64) -> ReplaySeed {
    let mut seed = None;
    replay_log(
        log,
        DEFAULT_REPLAY_LIMITS,
        ReplayRequest {
            stop: Some(ReplayStop::Position(position)),
            ..ReplayRequest::default()
        },
        |visit, engine| {
            if let ReplayVisit::State(record, _) = visit
                && record.state_id == position
            {
                seed = Some(ReplaySeed {
                    record,
                    checkpoint: engine.encode_checkpoint()?.expect("checkpoint available"),
                    source_position: None,
                });
            }

            return Ok(true);
        },
    )
    .expect("replay to the seed position");

    return seed.expect("the stop position is captured");
}

#[test]
fn every_position_matches_byte_at_a_time_replay() {
    let log = segmented_log();
    let reference = byte_at_a_time_hashes(&log);
    let last = u64::try_from(reference.len() - 1).expect("final position");
    let canonical = replay(&log).expect("canonical replay");
    assert_eq!(canonical.artifacts.final_state_id, last);

    for record in &canonical.states {
        let position = usize::try_from(record.state_id).expect("position");
        assert_eq!(
            record.state_hash, reference[position],
            "boundary {position}"
        );
    }

    // A hold's presented edge is the position just before the byte that begins it.
    let mut start = 0;
    let mut edges = Vec::new();

    for record in &log.events {
        match &record.event {
            Event::Output { data } => {
                let bytes = data.as_bytes();
                edges.extend(
                    bytes
                        .windows(8)
                        .enumerate()
                        .filter(|(_, window)| return *window == b"\x1b[?2026h")
                        .map(|(offset, _)| return start + offset + 7),
                );

                start += bytes.len();
            }
            Event::ResizeResult { .. } => start += 1,
            _ => {}
        }
    }

    assert!(edges.len() >= 6);
    for edge in edges {
        let edge = u64::try_from(edge).expect("edge position");
        assert!(
            canonical
                .states
                .iter()
                .any(|record| return record.state_id == edge && record.presented),
            "hold edge {edge}"
        );
    }

    for position in (0..=last).step_by(41) {
        let inspection = inspect(&log, StateSelector::StateId(position)).expect("position");
        assert_eq!(
            inspection.record.state_hash,
            reference[usize::try_from(position).expect("position")],
            "position {position}"
        );
        assert_eq!(inspection.record.state_id, position);
    }

    assert!(inspect(&log, StateSelector::StateId(last + 1)).is_err());
}

#[test]
fn indexed_seeks_rebuild_the_canonical_record_and_state() {
    let log = segmented_log();
    let mut canonical = Vec::new();
    replay_with(&log, None, |record, state, _| {
        canonical.push(StateInspection {
            record,
            state: state.clone(),
        });

        return Ok(());
    })
    .expect("canonical replay");

    let (_directory, indexed_log) = indexed(&log);
    let checkpoints = DerivedIndex::open(&indexed_log)
        .expect("trusted index lookup")
        .expect("cataloged index")
        .checkpoints()
        .expect("checkpoint catalog");
    assert!(
        checkpoints.len() >= 3,
        "the log must span several checkpoints"
    );

    for boundary in canonical.iter().step_by(13).chain(canonical.last()) {
        let seek = inspect(
            &indexed_log,
            StateSelector::StateId(boundary.record.state_id),
        )
        .expect("indexed seek");
        assert_eq!(&seek, boundary, "boundary {}", boundary.record.state_id);
    }

    let last = canonical.last().expect("final state").record.state_id;
    let mut positions: Vec<u64> = (0..=last).step_by(97).collect();

    for checkpoint in &checkpoints {
        positions.extend(
            [1, 2]
                .iter()
                .filter_map(|delta| return checkpoint.state_id.checked_sub(*delta)),
        );
        positions.extend((0..3).map(|delta| return checkpoint.state_id + delta));
    }

    for position in positions
        .into_iter()
        .filter(|position| return *position <= last)
    {
        assert_eq!(
            inspect(&indexed_log, StateSelector::StateId(position)).expect("indexed seek"),
            inspect(&log, StateSelector::StateId(position)).expect("canonical seek"),
            "position {position}"
        );
    }

    assert!(inspect(&indexed_log, StateSelector::StateId(last + 1)).is_err());
}

#[test]
fn indexed_selectors_resolve_like_canonical_replay() {
    let log = segmented_log();
    let (_directory, indexed_log) = indexed(&log);
    let canonical = replay(&log).expect("canonical replay");
    let mut selectors = vec![StateSelector::Final, StateSelector::LatestPresented];

    for record in log.events.iter().step_by(7) {
        selectors.push(StateSelector::EventSeq(record.seq));
        selectors.push(StateSelector::MonotonicNs(record.monotonic_ns));
        selectors.push(StateSelector::MonotonicNs(record.monotonic_ns - 1));
    }

    let mut first_occurrence = HashMap::new();
    let mut recurring = Vec::new();

    for state in &canonical.states {
        if first_occurrence
            .insert(state.state_hash.clone(), state.state_id)
            .is_some()
        {
            recurring.push(state.state_hash.clone());
        }
    }

    assert!(!recurring.is_empty(), "the redrawn screen must recur");

    for state_hash in recurring.into_iter().take(2).chain(
        canonical
            .states
            .iter()
            .step_by(5_000)
            .map(|state| return state.state_hash.clone()),
    ) {
        selectors.push(StateSelector::StateHash(state_hash));
    }

    let expected = inspect_many(&log, &selectors).expect("canonical selections");
    for (selector, expected) in selectors.into_iter().zip(expected) {
        let seek = inspect(&indexed_log, selector.clone()).expect("indexed selection");
        assert_eq!(seek, expected, "{selector:?}");
    }

    assert!(inspect(&indexed_log, StateSelector::StateHash("0".repeat(64))).is_err());
}

#[test]
fn hidden_tail_resolves_latest_presented_to_an_earlier_segment() {
    let mut events = vec![output(b"shown")];
    events.push(output(b"\x1b[?2026h"));

    for line in 0..300_u32 {
        events.push(output(
            format!("{line:04} {}\r\n", "y".repeat(48)).as_bytes(),
        ));
    }

    let log = event_log(40, 10, events);
    let (_directory, indexed_log) = indexed(&log);
    let checkpoints = DerivedIndex::open(&indexed_log)
        .expect("trusted index lookup")
        .expect("cataloged index")
        .checkpoints()
        .expect("checkpoint catalog");
    assert!(checkpoints.len() >= 2, "the hidden run must span segments");

    let expected = inspect(&log, StateSelector::LatestPresented).expect("canonical selection");
    assert_eq!(expected.record.state_id, 12);
    assert_eq!(expected.record.event_seq, Some(1));
    assert_eq!(
        inspect(&indexed_log, StateSelector::LatestPresented).expect("indexed selection"),
        expected
    );
}

#[test]
fn boundary_states_are_event_ends_effects_and_hold_edges() {
    let log = event_log(
        20,
        4,
        vec![
            output(b"a\x1b[3"),
            output(b"1mb\x1b]2;ti"),
            output(b"tle\x1b\\c\x1b[?2026hd\x1b[?2026le"),
        ],
    );

    let boundaries: Vec<(u64, Option<u64>, Option<u64>, bool)> = replay(&log)
        .expect("canonical replay")
        .states
        .iter()
        .map(|record| {
            return (
                record.state_id,
                record.event_seq,
                record.event_byte_offset,
                record.presented,
            );
        })
        .collect();
    assert_eq!(
        boundaries,
        [
            (0, None, None, true),
            (4, Some(0), Some(3), true),
            (13, Some(1), Some(8), true),
            (18, Some(2), Some(4), true),
            (26, Some(2), Some(12), true),
            (35, Some(2), Some(21), false),
            (36, Some(2), Some(22), true),
            (37, Some(2), Some(23), true),
        ]
    );
}

#[test]
fn checkpoints_inside_long_wrapped_alternate_screen_output_resume_canonically() {
    // Long unbroken lines keep the cursor in pending wrap and the alternate screen hides the
    // primary one; neither may prevent a checkpoint.
    let mut data = b"\x1b[?1049h".to_vec();
    for line in 0..40 {
        data.extend(format!("{line:04}").repeat(60).as_bytes());
    }

    let log = output_log(&data);
    let (_directory, indexed_log) = indexed(&log);
    let index = DerivedIndex::open(&indexed_log)
        .expect("trusted index lookup")
        .expect("cataloged index");

    let checkpoints = index.checkpoints().expect("checkpoint catalog");
    assert!(checkpoints.len() > 1, "long output must be checkpointed");

    let canonical = replay(&log).expect("canonical replay");

    for checkpoint in checkpoints {
        let seed = index.seed(&checkpoint).expect("checkpoint frame");
        validate_replay_seed(&log, &seed).expect("checkpoint must seed the pinned Ghostty engine");

        let start = position_of(&canonical.states, seed.record.state_id);
        let mut resumed = Vec::new();
        replay_with_seed_and_limits(&log, None, seed, DEFAULT_REPLAY_LIMITS, |record, _, _| {
            resumed.push(record);

            return Ok(());
        })
        .expect("checkpoint replay");

        assert_eq!(resumed, canonical.states[start..]);
    }
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
