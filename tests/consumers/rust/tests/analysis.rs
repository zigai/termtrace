mod support;

use serde_json::{json, Value};
use std::{collections::BTreeMap, fs};
use support::{enter, events, shell, stop, terminal, wait_text, Launch, ECHO, TIMEOUT};
use termtrace_sdk::{
    analysis::{AnalysisService, DiagnosticCheck},
    artifacts::ArtifactService,
    events::{Event, EventProjection, FollowPosition},
    input::{ActOutcome, ActRequest, MutationGuard},
    instrumentation::{ApplicationEventKind, Instrumentation},
    screen::ScreenPredicate,
    session::{LaunchSpec, Session},
    BundleProfile, Client, StatePoint,
};

const WIPING: &str = "printf READY; while IFS= read -r line; do if [ \"$line\" = wipe ]; then printf '\\033[2J\\033[HWIPED'; else printf '\\r\\nGOT:%s' \"$line\"; fi; done";

fn act(session: &Session, text: &str, expect: &str) -> ActOutcome {
    return session
        .act(ActRequest::new().text(text).key(enter()).until_screen(
            ScreenPredicate::Contains {
                text: expect.to_owned(),
            },
            TIMEOUT,
        ))
        .expect("act");
}

fn latest_hash(session: &Session) -> String {
    return session
        .observe(StatePoint::LatestPresented, false)
        .expect("latest observation")
        .state_hash;
}

#[test]
fn completed_session_analysis_matches_what_the_session_did() {
    let terminal = terminal("analysis-completed");
    let mut session = terminal
        .launch(LaunchSpec::new(shell(WIPING)))
        .expect("launch");
    wait_text(&session, "READY");
    act(&session, "alpha", "GOT:alpha");
    let mark = session
        .mark("checkpoint", &MutationGuard::default())
        .expect("mark");
    act(&session, "wipe", "WIPED");
    let last = act(&session, "omega", "GOT:omega");
    stop(&mut session);
    let path = session.path();
    let final_hash = latest_hash(&session);

    let timeline = AnalysisService::timeline_page(path, false, true, None, 0, 4096)
        .expect("timeline")
        .entries;
    let items: Vec<Value> = timeline
        .iter()
        .map(|entry| return serde_json::to_value(&entry.item).expect("item"))
        .collect();
    let marker = timeline
        .iter()
        .zip(&items)
        .find(|(_, item)| return item["event"]["label"] == "checkpoint")
        .expect("marker entry");
    assert_eq!(marker.0.event_seq, Some(mark.event_seq));
    let states: Vec<&Value> = items
        .iter()
        .filter(|item| return item["kind"] == "semantic_state")
        .collect();
    assert_eq!(states.last().expect("states")["state_hash"], final_hash);
    let page = AnalysisService::timeline_page(path, false, true, None, 0, 2).expect("page");
    assert_eq!(page.entries, timeline[..2]);
    assert_eq!(page.next_offset, Some(2));

    let diagnosis =
        AnalysisService::diagnose(path, vec![DiagnosticCheck::FullRedraw], 50).expect("diagnose");
    let clear = diagnosis
        .findings
        .iter()
        .find(|finding| return finding.code == "full_viewport_clear")
        .expect("the wipe is reported as a full viewport clear");
    let wiped_state = *clear.state_ids.first().expect("clear state");
    let wiped = session
        .observe(
            StatePoint::StateId {
                state_id: wiped_state,
            },
            true,
        )
        .expect("cleared state");
    assert!(!wiped.frame.viewport_text.contains("GOT:alpha"));

    let explained = AnalysisService::explain(
        path,
        StatePoint::StateId {
            state_id: wiped_state,
        },
        4,
    )
    .expect("explain");
    assert_eq!(explained.target_state.state_id, wiped_state);
    assert!(explained
        .tokens
        .iter()
        .any(|token| return token.description == "erase display (all)"
            && token.raw.as_bytes() == b"\x1b[2J"));

    let analysis = AnalysisService::last_action(path, 50).expect("last action");
    assert_eq!(analysis.inspection.action.request_id, Some(last.request_id));
    assert_eq!(
        analysis.inspection.action.bytes_written,
        last.input.bytes_written
    );
    assert!(analysis.inspection.output_generated.byte_count >= "\r\nGOT:omega".len() as u64);
    assert!(!analysis.inspection.rows_changed.is_empty());

    let markers = AnalysisService::markers(path, 30).expect("markers");
    assert_eq!(markers.len(), 1);
    assert_eq!(markers[0].label, "checkpoint");
    assert_eq!(markers[0].event_seq, mark.event_seq);

    let summary = AnalysisService::review_summary(path).expect("summary");
    assert_eq!(summary.canonical_session, path);
    assert_eq!(summary.marker_count, 1);
    assert_eq!(summary.final_state_hash, final_hash);
    assert!(summary.presented_state_count >= 4);

    let found = AnalysisService::review_search(path, "GOT:alpha").expect("search");
    assert!(!found.matches.is_empty());
    assert!(AnalysisService::review_search(path, "never-printed-text")
        .expect("empty search")
        .matches
        .is_empty());

    let state_id = found.matches[0].state_id;
    let destination = terminal.evidence_root().with_file_name("review-export");
    let exported =
        AnalysisService::review_export_state(path, state_id, &destination).expect("review export");
    assert_eq!(exported.state_id, state_id);
    assert_eq!(exported.state_hash, found.matches[0].state_hash);
    let screen = fs::read_to_string(destination.join("screen.txt")).expect("exported screen");
    assert!(screen.contains("GOT:alpha"), "{screen}");
    assert!(
        AnalysisService::review_export_state(path, state_id, &destination).is_err(),
        "export never replaces"
    );
}

#[test]
fn replay_cache_status_counts_published_checkpoints() {
    let terminal = terminal("analysis-cache");
    let mut session = terminal
        .launch(LaunchSpec::new(shell(ECHO)))
        .expect("launch");
    wait_text(&session, "READY");
    stop(&mut session);
    let path = session.path();
    AnalysisService::cache_prune(path).expect("prune");

    let empty = AnalysisService::cache_status(path).expect("status");
    latest_hash(&session);
    let filled = AnalysisService::cache_status(path).expect("status");
    let pruned = AnalysisService::cache_prune(path).expect("prune");

    assert_eq!((empty.entries, empty.compressed_bytes), (0, 0));
    assert_eq!(filled.entries, 1);
    assert!(filled.compressed_bytes > 0);
    assert_eq!(pruned.removed_entries, 1);
    assert_eq!(
        AnalysisService::cache_status(path).expect("status").entries,
        0
    );
}

#[test]
fn asciicast_export_is_asciinema_v2_with_the_printed_output() {
    let terminal = terminal("analysis-asciicast");
    let mut session = terminal
        .launch(LaunchSpec::new(shell(ECHO)).geometry(72, 9))
        .expect("launch");
    wait_text(&session, "READY");
    act(&session, "hello cast", "GOT:hello cast");
    stop(&mut session);
    let destination = terminal.evidence_root().with_file_name("session.cast");

    ArtifactService::export_asciicast(session.path(), &destination).expect("export");

    let text = fs::read_to_string(&destination).expect("cast");
    let mut lines = text.lines();
    let header: Value = serde_json::from_str(lines.next().expect("header")).expect("header JSON");
    assert_eq!(header["version"], 2);
    assert_eq!(
        (header["width"].as_u64(), header["height"].as_u64()),
        (Some(72), Some(9))
    );
    let mut output = String::new();
    let mut previous = 0.0;
    for line in lines {
        let event: Value = serde_json::from_str(line).expect("event JSON");
        let time = event[0].as_f64().expect("event time");
        assert!(time >= previous, "{line}");
        previous = time;
        if event[1] == "o" {
            output.push_str(event[2].as_str().expect("output text"));
        }
    }
    assert!(output.starts_with("READY"), "{output}");
    assert!(output.contains("GOT:hello cast"), "{output}");
}

#[test]
fn bundle_members_classify_terminal_content_as_sensitive() {
    let terminal = terminal("analysis-sensitivity");
    let mut session = terminal
        .launch(LaunchSpec::new(shell(ECHO)))
        .expect("launch");
    wait_text(&session, "READY");
    stop(&mut session);
    let destination = terminal.evidence_root().with_file_name("bundle");

    let manifest =
        ArtifactService::create_bundle(session.path(), &destination, BundleProfile::Forensic)
            .expect("bundle");

    let sensitivity: BTreeMap<String, Value> = manifest
        .members
        .iter()
        .map(|member| {
            return (
                member.path.clone(),
                serde_json::to_value(member.sensitivity).expect("sensitivity"),
            );
        })
        .collect();
    assert_eq!(
        sensitivity["screen.txt"], "sensitive_terminal_data",
        "{sensitivity:?}"
    );
    assert!(
        sensitivity
            .values()
            .any(|value| return value == "operational_metadata"),
        "{sensitivity:?}"
    );
    for member in &manifest.members {
        assert!(destination.join(&member.path).is_file(), "{}", member.path);
    }
}

#[test]
fn instrumentation_acknowledgements_land_in_the_event_log() {
    let terminal = terminal("analysis-instrumentation");
    let mut session = terminal
        .launch(LaunchSpec::new(shell(ECHO)))
        .expect("launch");
    wait_text(&session, "READY");
    let instrumentation = Instrumentation::new(Client::new(session.path()));
    let attributes = BTreeMap::from([("items".to_owned(), json!(3))]);

    let event = instrumentation
        .event_ack("checkout", attributes.clone())
        .expect("event");
    let started = instrumentation
        .start_span_ack("load-page", None, BTreeMap::new())
        .expect("span start");
    let span_id = started.span_id.expect("span id");
    let ended = instrumentation
        .end_span_ack("load-page", span_id, BTreeMap::new())
        .expect("span end");
    stop(&mut session);

    assert!(event.acknowledged && started.acknowledged && ended.acknowledged);
    assert!(event.span_id.is_none());
    assert_eq!(ended.span_id, Some(span_id));
    let recorded: Vec<_> = events(&session)
        .into_iter()
        .filter_map(|event| {
            return match event {
                Event::Application { data, .. } => Some(data),
                _ => None,
            };
        })
        .collect();
    assert_eq!(recorded.len(), 3);
    assert_eq!(recorded[0].kind, ApplicationEventKind::Event);
    assert_eq!(recorded[0].name, "checkout");
    assert_eq!(recorded[0].attributes, attributes);
    assert_eq!(recorded[1].kind, ApplicationEventKind::SpanStart);
    assert_eq!(recorded[1].span_id, Some(span_id));
    assert_eq!(recorded[2].kind, ApplicationEventKind::SpanEnd);
    assert_eq!(recorded[2].span_id, Some(span_id));
    for data in &recorded {
        assert_eq!(
            data.process_instance_id,
            instrumentation.process_instance_id()
        );
    }
    assert!(started.boundaries.durable.next_event_seq > event.boundaries.durable.next_event_seq);
}

fn output_text(records: &[EventProjection]) -> String {
    let bytes: Vec<u8> = records
        .iter()
        .filter_map(|record| {
            return match &record.event {
                Some(Event::Output { data }) => Some(data.as_bytes().to_vec()),
                _ => None,
            };
        })
        .flatten()
        .collect();

    return String::from_utf8_lossy(&bytes).into_owned();
}

#[test]
fn follow_yields_events_written_after_the_cursor() {
    let terminal = terminal("analysis-follow");
    let mut session = terminal
        .launch(LaunchSpec::new(shell(ECHO)))
        .expect("launch");
    wait_text(&session, "READY");

    let FollowPosition::Live { durable, .. } = session.events().follow(0).expect("follow") else {
        panic!("a live session follows live");
    };
    act(&session, "after-cursor", "GOT:after-cursor");
    session
        .output()
        .wait_for(b"GOT:after-cursor", TIMEOUT)
        .expect("logged output");
    let (older, _) = session.events().page(0, 4096, false).expect("all events");
    let (newer, _) = session
        .events()
        .page(durable.next_event_seq, 4096, false)
        .expect("events after the cursor");
    let before_cursor: Vec<EventProjection> = older
        .into_iter()
        .filter(|record| return record.seq < durable.next_event_seq)
        .collect();
    assert!(!output_text(&before_cursor).contains("GOT:after-cursor"));
    let output = output_text(&newer);
    assert!(output.contains("GOT:after-cursor"), "{output}");
    assert!(newer
        .iter()
        .all(|record| return record.seq >= durable.next_event_seq));

    let FollowPosition::Live {
        start,
        durable: later,
        ..
    } = session
        .events()
        .follow(durable.next_event_seq)
        .expect("follow again")
    else {
        panic!("still live");
    };
    assert_eq!(
        start.expect("start cursor").next_event_seq,
        durable.next_event_seq
    );
    assert!(later.next_event_seq > durable.next_event_seq);

    let completion = stop(&mut session);
    let FollowPosition::Completed(completed) = session
        .events()
        .follow(later.next_event_seq)
        .expect("follow completed")
    else {
        panic!("a completed session reports completion");
    };
    assert_eq!(completed.final_event_hash, completion.final_event_hash);
}
