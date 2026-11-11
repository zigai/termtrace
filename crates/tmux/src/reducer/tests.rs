use super::*;
use crate::artifact::TmuxCompositeTrack;
use std::slice;

fn state(lifecycle: TmuxRecordingLifecycle) -> TmuxRecordingState {
    return TmuxRecordingState {
        lifecycle,
        composite_track: TmuxCompositeTrack {
            alias: "tmux-test-composite".to_owned(),
            link: None,
        },
        server: None,
        panes: Vec::new(),
        pane_tracks: BTreeMap::new(),
        coverage_gaps: permanent_gaps(1),
        updated_unix_ns: 1,
    };
}

fn identity(pid: u32) -> TmuxServerIdentity {
    return TmuxServerIdentity {
        pid,
        socket_path: ByteString::from(b"/tmp/tmux-1000/test".as_slice()),
        socket_device: 1,
        socket_inode: 2,
    };
}

fn pane(pane_id: &str, window_id: &str, layout: &str) -> TmuxPaneSnapshot {
    return TmuxPaneSnapshot {
        session_id: "$0".to_owned(),
        session_name: "work".to_owned(),
        window_id: window_id.to_owned(),
        window_index: 0,
        layout: layout.to_owned(),
        window_active: true,
        zoomed: false,
        pane_id: pane_id.to_owned(),
        pane_active: true,
        copy_mode: false,
        pane_mode: String::new(),
        recording_session: None,
    };
}

fn track(pane_id: &str, session: &str) -> TmuxPaneTrack {
    return TmuxPaneTrack {
        pane_id: pane_id.to_owned(),
        session_id: Uuid::now_v7(),
        session: ByteString::from(session.as_bytes()),
        linked_unix_ns: 5,
    };
}

fn observe(panes: Vec<TmuxPaneSnapshot>, uncovered: &[&str]) -> Input {
    return Input::Observe(Observation {
        identity: Some(identity(10)),
        panes,
        uncovered: uncovered
            .iter()
            .map(|pane| return (*pane).to_owned())
            .collect(),
        links: BTreeMap::new(),
    });
}

fn kinds(events: &[TmuxEvent]) -> Vec<String> {
    return events
        .iter()
        .map(|event| {
            return serde_json::to_value(event).expect("event JSON")["kind"]
                .as_str()
                .expect("kind")
                .to_owned();
        })
        .collect();
}

fn link() -> TmuxSessionLink {
    return TmuxSessionLink {
        session_id: Uuid::now_v7(),
        session: ByteString::from(b"/state/composite".as_slice()),
    };
}

#[test]
fn composite_link_makes_a_starting_recording_live() {
    let step = reduce(
        "test",
        &state(TmuxRecordingLifecycle::Starting),
        Input::CompositeLinked(link()),
        2,
    )
    .expect("link");
    assert_eq!(step.state.lifecycle, TmuxRecordingLifecycle::Live);
    assert!(step.state.composite_track.link.is_some());
    assert_eq!(kinds(&step.events), ["composite_track_linked"]);
    assert!(
        reduce("test", &step.state, Input::CompositeLinked(link()), 3).is_err(),
        "a live composite link is never replaced"
    );
}

#[test]
fn composite_completion_while_live_does_not_complete_the_recording() {
    let live = state(TmuxRecordingLifecycle::Live);
    let step = reduce(
        "test",
        &live,
        Input::TracksSettled {
            completed: Vec::new(),
            remaining: 0,
            composite_completed: true,
        },
        2,
    )
    .expect("settle");
    assert_eq!(step.state, live);
    assert!(step.events.is_empty());
}

#[test]
fn stopping_completes_only_when_every_track_completed() {
    let stopping = reduce(
        "test",
        &state(TmuxRecordingLifecycle::Live),
        Input::StopRequested { identity: None },
        2,
    )
    .expect("stop")
    .state;

    let repeated = reduce(
        "test",
        &stopping,
        Input::StopRequested { identity: None },
        3,
    )
    .expect("repeat stop");
    assert!(repeated.events.is_empty(), "a repeated stop is idempotent");
    let pending = reduce(
        "test",
        &stopping,
        Input::TracksSettled {
            completed: vec![("%1".to_owned(), Uuid::now_v7())],
            remaining: 1,
            composite_completed: true,
        },
        4,
    )
    .expect("partial settle");
    assert_eq!(pending.state.lifecycle, TmuxRecordingLifecycle::Stopping);
    assert_eq!(kinds(&pending.events), ["pane_track_completed"]);

    let done = reduce(
        "test",
        &pending.state,
        Input::TracksSettled {
            completed: Vec::new(),
            remaining: 0,
            composite_completed: true,
        },
        5,
    )
    .expect("settle");
    assert_eq!(done.state.lifecycle, TmuxRecordingLifecycle::Completed);
    assert_eq!(kinds(&done.events), ["recording_completed"]);
}

#[test]
fn late_track_link_cannot_reopen_a_completed_recording() {
    let completed = state(TmuxRecordingLifecycle::Completed);
    let error = reduce(
        "test",
        &completed,
        Input::TrackLinked(track("%1", "/late")),
        2,
    )
    .expect_err("sealed");
    assert!(matches!(error, Error::Sealed(_)));
}

#[test]
fn completed_recordings_accept_no_further_events() {
    let completed = state(TmuxRecordingLifecycle::Completed);

    for input in [
        observe(vec![pane("%1", "@1", "a")], &["%1"]),
        Input::ObserverStarted { pid: 3 },
        Input::StopRequested { identity: None },
        Input::TracksSettled {
            completed: vec![("%1".to_owned(), Uuid::now_v7())],
            remaining: 0,
            composite_completed: true,
        },
    ] {
        let step = reduce("test", &completed, input, 9).expect("sealed input");
        assert!(step.events.is_empty());
        assert_eq!(step.state, completed);
    }

    assert!(reduce("test", &completed, Input::CompositeLinked(link()), 9).is_err());
}

#[test]
fn observations_bind_identity_once_and_reject_another_server() {
    let bound = reduce(
        "test",
        &state(TmuxRecordingLifecycle::Live),
        observe(Vec::new(), &[]),
        2,
    )
    .expect("bind");
    assert_eq!(bound.state.server, Some(identity(10)));
    assert_eq!(kinds(&bound.events), ["server_identity_bound"]);

    let same = reduce("test", &bound.state, observe(Vec::new(), &[]), 3).expect("same server");
    assert!(same.events.is_empty());

    let replacement = Input::Observe(Observation {
        identity: Some(identity(11)),
        ..Observation::default()
    });
    assert!(matches!(
        reduce("test", &bound.state, replacement, 4),
        Err(Error::ServerIdentityChanged { .. })
    ));
}

#[test]
fn stop_binds_the_answering_server_and_rejects_another() {
    let live = state(TmuxRecordingLifecycle::Live);
    let stop = |identity| {
        return Input::StopRequested {
            identity: Some(identity),
        };
    };
    let stopping = reduce("test", &live, stop(identity(10)), 2).expect("stop");
    assert_eq!(stopping.state.server, Some(identity(10)));
    assert_eq!(
        kinds(&stopping.events),
        ["server_identity_bound", "recording_stopping"]
    );

    let bound = reduce("test", &live, observe(Vec::new(), &[]), 2)
        .expect("bind")
        .state;

    assert!(matches!(
        reduce("test", &bound, stop(identity(11)), 3),
        Err(Error::ServerIdentityChanged { .. })
    ));
}

#[test]
fn observations_emit_pane_container_and_coverage_transitions() {
    let live = state(TmuxRecordingLifecycle::Live);
    let first = reduce(
        "test",
        &live,
        observe(vec![pane("%1", "@1", "a")], &["%1"]),
        2,
    )
    .expect("first snapshot");
    assert_eq!(
        kinds(&first.events),
        [
            "server_identity_bound",
            "pane_created",
            "session_created",
            "window_created",
            "coverage_gap_opened"
        ]
    );

    let second = reduce(
        "test",
        &first.state,
        observe(vec![pane("%1", "@1", "b")], &["%1"]),
        3,
    )
    .expect("second snapshot");
    assert_eq!(kinds(&second.events), ["layout_changed"]);

    let gap = second
        .state
        .coverage_gaps
        .iter()
        .find(|gap| return gap.kind == TmuxGapKind::UncoveredPane)
        .expect("open uncovered gap");
    assert_eq!(
        (gap.first_observed_unix_ns, gap.last_observed_unix_ns),
        (2, 3)
    );
    let third = reduce(
        "test",
        &second.state,
        observe(vec![pane("%2", "@2", "c")], &[]),
        4,
    )
    .expect("third snapshot");
    assert_eq!(
        kinds(&third.events),
        [
            "pane_created",
            "pane_destroyed",
            "window_created",
            "window_destroyed",
            "coverage_gap_resolved"
        ]
    );

    let Some(TmuxEvent::CoverageGapResolved { gap }) = third.events.last() else {
        panic!("resolution event");
    };

    assert_eq!(gap.resolved_unix_ns, Some(4));
    assert_eq!(gap.first_observed_unix_ns, 2);
    assert!(
        third
            .state
            .coverage_gaps
            .iter()
            .all(|gap| return gap.kind != TmuxGapKind::UncoveredPane),
        "resolved intervals leave the snapshot"
    );
}

#[test]
fn published_recordings_link_once_and_follow_their_pane() {
    let live = state(TmuxRecordingLifecycle::Live);
    let mut covered = pane("%1", "@1", "a");
    covered.recording_session = Some(ByteString::from(b"/state/pane".as_slice()));
    assert_eq!(
        unlinked_recordings(&live, slice::from_ref(&covered)),
        [("%1".to_owned(), ByteString::from(b"/state/pane".as_slice()))]
    );

    let linked = reduce(
        "test",
        &live,
        Input::Observe(Observation {
            identity: None,
            panes: vec![covered.clone()],
            uncovered: BTreeSet::new(),
            links: BTreeMap::from([("%1".to_owned(), track("%1", "/state/pane"))]),
        }),
        2,
    )
    .expect("link");
    assert!(kinds(&linked.events).contains(&"pane_track_linked".to_owned()));
    assert!(unlinked_recordings(&linked.state, slice::from_ref(&covered)).is_empty());

    let repeated = reduce(
        "test",
        &linked.state,
        Input::TrackLinked(track("%1", "/state/pane")),
        3,
    )
    .expect("wrapper registration after observation");
    assert!(repeated.events.is_empty());
    let gone = reduce("test", &linked.state, observe(Vec::new(), &[]), 4).expect("pane gone");
    assert!(gone.state.pane_tracks.is_empty());
}

#[test]
fn observer_start_records_a_resolved_observation_gap() {
    let step = reduce(
        "test",
        &state(TmuxRecordingLifecycle::Live),
        Input::ObserverStarted { pid: 7 },
        5,
    )
    .expect("observer");
    assert_eq!(
        kinds(&step.events),
        [
            "coverage_gap_opened",
            "coverage_gap_resolved",
            "observer_started"
        ]
    );

    let Some(TmuxEvent::CoverageGapResolved { gap }) = step.events.get(1) else {
        panic!("resolved gap");
    };

    assert_eq!(gap.kind, TmuxGapKind::ObserverStart);
    assert_eq!(
        (gap.first_observed_unix_ns, gap.resolved_unix_ns),
        (1, Some(5))
    );
}
