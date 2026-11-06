use super::*;
use crate::replay::{reset_test_replay_output_bytes, test_replay_output_bytes, write_cataloged};
use termtrace_model::execution::event::{EventLogHeader, EventRecord};
use termtrace_model::terminal::effect::{ProgressState, PromptKind, TerminalEffect};
use termtrace_model::value::ByteString;
use uuid::Uuid;

fn ranged_timeline_log() -> EventLog {
    let session_id = Uuid::now_v7();
    let events = vec![
        EventRecord {
            observed_monotonic_ns: None,
            seq: 0,
            monotonic_ns: 1,
            prev_hash: "0".repeat(64),
            hash: "1".repeat(64),
            event: Event::Output {
                data: ByteString::from(b"a".as_slice()),
            },
        },
        EventRecord {
            observed_monotonic_ns: None,
            seq: 1,
            monotonic_ns: 2,
            prev_hash: "1".repeat(64),
            hash: "2".repeat(64),
            event: Event::Marker {
                operation_id: Uuid::now_v7(),
                label: "range-start".to_owned(),
            },
        },
        EventRecord {
            observed_monotonic_ns: None,
            seq: 2,
            monotonic_ns: 3,
            prev_hash: "2".repeat(64),
            hash: "3".repeat(64),
            event: Event::Output {
                data: ByteString::from(b"b".as_slice()),
            },
        },
    ];

    return EventLog {
        header: EventLogHeader::new(
            session_id,
            "0".repeat(64),
            0,
            80,
            24,
            800,
            480,
            "xterm-ghostty",
        ),
        events,
        pending_operations: Vec::new(),
        source_path: None,
    };
}

#[test]
fn ranged_timeline_pages_from_a_replay_baseline() {
    let log = ranged_timeline_log();
    let range = Some(SourceRange {
        start_monotonic_ns: 2,
        end_monotonic_ns: 4,
    });

    let first = build_timeline_page(&log, false, false, range, 0, 1).expect("first timeline page");
    assert_eq!(first.entries.len(), 1);
    assert_eq!(first.next_offset, Some(1));

    let second =
        build_timeline_page(&log, false, false, range, 1, 8).expect("remaining timeline page");
    assert!(!second.entries.is_empty());
    assert_eq!(second.next_offset, None);
    assert!(first
        .entries
        .iter()
        .chain(&second.entries)
        .all(|entry| return (2..4).contains(&entry.monotonic_ns)));
}

fn indexed_control_log(temporary: &tempfile::TempDir) -> EventLog {
    let session_id = Uuid::now_v7();
    let mut events: Vec<EventRecord> = Vec::new();

    for seq in 0..400_u64 {
        events.push(EventRecord {
            observed_monotonic_ns: None,
            seq,
            monotonic_ns: seq.saturating_add(1),
            prev_hash: format!("{seq:064x}"),
            hash: format!("{:064x}", seq.saturating_add(1)),
            event: Event::Output {
                data: ByteString::from(format!("{seq:04}\x1b[H").into_bytes()),
            },
        });
    }

    let log = EventLog {
        header: EventLogHeader::new(
            session_id,
            "0".repeat(64),
            0,
            80,
            24,
            800,
            480,
            "xterm-ghostty",
        ),
        events,
        pending_operations: Vec::new(),
        source_path: Some(temporary.path().join("events.jsonl")),
    };

    write_cataloged(temporary.path(), &log).expect("control-rich checkpoint index");

    return log;
}

#[test]
fn late_control_pages_and_ranges_seek_auxiliary_output() {
    let temporary = tempfile::TempDir::new().expect("temporary directory");
    let log = indexed_control_log(&temporary);
    let mut canonical = log.clone();
    canonical.source_path = None;
    let full = build_timeline(&canonical, true, true, None).expect("full control timeline");
    let offset = full.len().saturating_sub(20);

    reset_test_auxiliary_scan();

    let page = build_timeline_page(
        &log,
        true,
        true,
        None,
        u64::try_from(offset).expect("late offset"),
        5,
    )
    .expect("late control page");

    let (offset_bytes, offset_events, offset_first_event) = test_auxiliary_scan();
    assert!(
        offset_first_event > 0,
        "late page scanned controls from event zero"
    );
    assert!(
        offset_bytes < 200,
        "late page scanned {offset_bytes} control bytes"
    );
    assert!(
        offset_events < 30,
        "late page scanned {offset_events} control events"
    );
    assert_eq!(
        page.entries,
        full.iter()
            .skip(offset)
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
    );

    let range = Some(SourceRange {
        start_monotonic_ns: 350,
        end_monotonic_ns: 361,
    });

    let expected =
        build_timeline(&canonical, true, true, range).expect("full ranged control timeline");

    reset_test_auxiliary_scan();

    let ranged =
        build_timeline_page(&log, true, true, range, 0, 10_000).expect("late ranged control page");

    let (range_bytes, range_events, range_first_event) = test_auxiliary_scan();
    assert!(
        range_first_event >= 348,
        "late range began auxiliary framing at event {range_first_event}"
    );
    assert!(
        range_bytes < 200,
        "late range scanned {range_bytes} control bytes"
    );
    assert!(
        range_events < 30,
        "late range scanned {range_events} control events"
    );
    assert_eq!(ranged.entries, expected);
    assert_eq!(ranged.next_offset, None);
}

#[test]
fn indexed_range_preserves_control_spanning_output_events() {
    let template = tempfile::TempDir::new().expect("template directory");
    let mut log = indexed_control_log(&template);

    for (seq, data) in [
        (348_usize, b"\x1b]2;".as_slice()),
        (349_usize, b"spanning-title".as_slice()),
        (350_usize, b"\x07".as_slice()),
    ] {
        let Event::Output { data: output } = &mut log.events[seq].event else {
            panic!("control fixture output event");
        };
        *output = ByteString::from(data);
    }

    let temporary = tempfile::TempDir::new().expect("indexed directory");
    log.source_path = Some(temporary.path().join("events.jsonl"));
    write_cataloged(temporary.path(), &log).expect("spanning-control index");

    let range = Some(SourceRange {
        start_monotonic_ns: 350,
        end_monotonic_ns: 351,
    });

    let mut canonical = log.clone();
    canonical.source_path = None;
    let expected = build_timeline(&canonical, true, true, range).expect("canonical spanning range");

    reset_test_auxiliary_scan();

    let page =
        build_timeline_page(&log, true, true, range, 0, 10_000).expect("indexed spanning range");

    let (bytes, events, first_event) = test_auxiliary_scan();

    assert_eq!(page.entries, expected);
    assert_eq!(page.next_offset, None);
    assert_eq!(first_event, 348);
    assert!(bytes < 100, "spanning range read {bytes} control bytes");
    assert!(events < 10, "spanning range read {events} control events");
}

#[test]
fn unindexed_range_preserves_csi_spanning_output_events() {
    let mut log = ranged_timeline_log();
    for (event, data) in
        log.events
            .iter_mut()
            .zip([b"\x1b".as_slice(), b"[".as_slice(), b"31m".as_slice()])
    {
        event.event = Event::Output {
            data: ByteString::from(data),
        };
    }

    let range = Some(SourceRange {
        start_monotonic_ns: 3,
        end_monotonic_ns: 4,
    });

    let expected =
        build_timeline(&log, true, true, range).expect("canonical unindexed spanning range");

    let page =
        build_timeline_page(&log, true, true, range, 0, 10_000).expect("unindexed spanning range");

    assert!(expected.iter().any(|entry| {
        return matches!(
            &entry.item,
            TimelineItem::VtToken { token }
                if token.raw.as_bytes() == b"\x1b[31m" && token.source.len() == 3
        );
    }));
    assert_eq!(page.entries, expected);
    assert_eq!(page.next_offset, None);
}

#[test]
fn streaming_pages_match_full_timeline_with_controls() {
    let log = ranged_timeline_log();
    let full = build_timeline(&log, true, true, None).expect("full timeline");

    for offset in 0..=full.len() {
        let page = build_timeline_page(
            &log,
            true,
            true,
            None,
            u64::try_from(offset).expect("offset"),
            2,
        )
        .expect("streamed page");
        assert_eq!(
            page.entries,
            full.iter()
                .skip(offset)
                .take(2)
                .cloned()
                .collect::<Vec<_>>()
        );
        assert_eq!(
            page.next_offset,
            (offset.saturating_add(2) < full.len())
                .then(|| return u64::try_from(offset.saturating_add(2)).expect("next offset"))
        );
    }
}

#[test]
fn late_page_uses_checkpoint_and_retains_only_requested_entries() {
    let temporary = tempfile::TempDir::new().expect("temporary directory");
    let mut log = EventLog {
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
        events: (0..1_100)
            .map(|seq| {
                return EventRecord {
                    observed_monotonic_ns: None,
                    seq,
                    monotonic_ns: seq + 1,
                    prev_hash: "0".repeat(64),
                    hash: "1".repeat(64),
                    event: Event::Output {
                        data: ByteString::from(b"x".as_slice()),
                    },
                };
            })
            .collect(),
        pending_operations: Vec::new(),
        source_path: Some(temporary.path().join("events.jsonl")),
    };

    write_cataloged(temporary.path(), &log).expect("checkpointed index");

    reset_test_replay_output_bytes();

    reset_test_max_page_entries();

    let page = build_timeline_page(&log, true, false, None, 1_090, 3).expect("late timeline page");
    let replayed_bytes = test_replay_output_bytes();
    assert!(
        replayed_bytes < 100,
        "late page replayed {replayed_bytes} bytes instead of a checkpoint suffix"
    );
    assert_eq!(page.entries.len(), 3);
    assert_eq!(test_max_page_entries(), 3);
    assert_eq!(page.next_offset, Some(1_093));

    log.source_path = None;
    let full = build_timeline(&log, true, false, None).expect("canonical full timeline");
    assert_eq!(
        page.entries,
        full.into_iter().skip(1_090).take(3).collect::<Vec<_>>()
    );
}

fn output_log(outputs: &[&[u8]], source_path: Option<PathBuf>) -> EventLog {
    let events = outputs
        .iter()
        .enumerate()
        .map(|(index, data)| {
            let seq = u64::try_from(index).expect("event sequence");

            return EventRecord {
                observed_monotonic_ns: None,
                seq,
                monotonic_ns: seq.saturating_add(1),
                prev_hash: format!("{seq:064x}"),
                hash: format!("{:064x}", seq.saturating_add(1)),
                event: Event::Output {
                    data: ByteString::from(*data),
                },
            };
        })
        .collect();

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
        events,
        pending_operations: Vec::new(),
        source_path,
    };
}

fn effects(entries: &[TimelineEntry]) -> Vec<(u64, u64, TerminalEffect)> {
    return entries
        .iter()
        .filter_map(|entry| match &entry.item {
            TimelineItem::TerminalEffect {
                event_byte_offset,
                effect,
            } => return Some((entry.event_seq?, *event_byte_offset, effect.clone())),
            _ => return None,
        })
        .collect();
}

#[test]
fn timeline_reports_effects_at_the_byte_that_completes_them() {
    let log = output_log(
        &[
            b"\x1b]133;A\x07$ \x1b]133;B\x07",
            b"\x1b]133;C\x07out\x1b]2;t\x07\x07\x1b]133;D;",
            b"3\x07\x1b[?2026hhidden\x1b]133;D\x07\x1b]9;4;3\x07\x1b[?2026l",
        ],
        None,
    );

    let timeline = build_timeline(&log, false, false, None).expect("timeline");
    assert_eq!(
        effects(&timeline),
        vec![
            (
                0,
                7,
                TerminalEffect::PromptStart {
                    prompt: PromptKind::Primary
                }
            ),
            (0, 17, TerminalEffect::InputStart),
            (1, 7, TerminalEffect::CommandStart { command: None }),
            (
                1,
                16,
                TerminalEffect::TitleChanged {
                    title: ByteString::from(b"t".as_slice())
                }
            ),
            (1, 17, TerminalEffect::Bell),
            (
                2,
                1,
                TerminalEffect::CommandEnd {
                    exit_code: Some(3),
                    error: None
                }
            ),
            (
                2,
                23,
                TerminalEffect::CommandEnd {
                    exit_code: None,
                    error: None
                }
            ),
            (
                2,
                31,
                TerminalEffect::ProgressReport {
                    state: ProgressState::Indeterminate,
                    percent: None
                }
            ),
        ]
    );
    assert!(timeline
        .windows(2)
        .all(|pair| return timeline_key(&pair[0]) <= timeline_key(&pair[1])));

    // The title's state follows its effect, which shares the state's timeline key.
    let title = timeline
        .iter()
        .position(|entry| {
            return matches!(
                entry.item,
                TimelineItem::TerminalEffect {
                    effect: TerminalEffect::TitleChanged { .. },
                    ..
                }
            );
        })
        .expect("title effect");
    assert!(matches!(
        timeline[title + 1].item,
        TimelineItem::SemanticState {
            event_byte_offset: Some(16),
            metadata_changed: true,
            ..
        }
    ));
}

#[test]
fn checkpointed_pages_report_the_same_effects_as_full_replay() {
    let temporary = tempfile::TempDir::new().expect("temporary directory");
    let outputs = (0..300_u32)
        .map(|seq| {
            // Every third event leaves an unknown OSC open for the next one to finish.
            let tail: &[u8] = if seq % 3 == 0 { b"\x1b]7400;a" } else { b"" };
            let mut bytes = format!("{seq:04}\x07\x1b]133;D;{seq}\x07").into_bytes();
            if seq % 3 == 1 {
                bytes.splice(0..0, *b"b\x07");
            }

            bytes.extend_from_slice(tail);

            return bytes;
        })
        .collect::<Vec<_>>();

    let slices = outputs.iter().map(Vec::as_slice).collect::<Vec<_>>();
    let mut log = output_log(&slices, Some(temporary.path().join("events.jsonl")));
    write_cataloged(temporary.path(), &log).expect("checkpointed index");
    let indexed = log.clone();
    log.source_path = None;
    let full = build_timeline(&log, true, false, None).expect("full timeline");
    assert_eq!(
        effects(&full)
            .iter()
            .filter(|(_, _, effect)| return matches!(
                effect,
                TerminalEffect::UnknownSequence { .. }
            ))
            .count(),
        100
    );

    for offset in [0, 700, 1_000, 1_597, full.len() - 3] {
        reset_test_replay_output_bytes();

        let page = build_timeline_page(
            &indexed,
            true,
            false,
            None,
            u64::try_from(offset).expect("offset"),
            3,
        )
        .expect("checkpointed page");

        if offset >= 1_000 {
            assert!(
                test_replay_output_bytes() < 4_000,
                "page at {offset} did not start from a checkpoint"
            );
        }

        assert_eq!(
            page.entries,
            full.iter()
                .skip(offset)
                .take(3)
                .cloned()
                .collect::<Vec<_>>(),
            "page at {offset}"
        );
    }
}
