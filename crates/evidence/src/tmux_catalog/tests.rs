use std::{collections::BTreeMap, fs};

use tempfile::TempDir;
use termtrace_model::{
    artifact::tmux::{
        TMUX_RECORDING_FORMAT, TMUX_RECORDING_VERSION, TmuxCompositeTrack, TmuxLifecycleEvent,
        TmuxRecordingCoverage, TmuxRecordingLifecycle,
    },
    value::ByteString,
};

use super::*;
use crate::artifact::create_private_directory;

fn manifest() -> TmuxRecordingManifest {
    return TmuxRecordingManifest {
        format: TMUX_RECORDING_FORMAT.to_owned(),
        version: TMUX_RECORDING_VERSION,
        recording_id: Uuid::now_v7(),
        name: "test".to_owned(),
        lifecycle: TmuxRecordingLifecycle::Live,
        created_unix_ns: 1,
        updated_unix_ns: 1,
        coverage: TmuxRecordingCoverage::Warn,
        tmux_executable: ByteString::from(b"tmux".as_slice()),
        socket_name: None,
        socket_path: None,
        server_pid: None,
        server_socket_path: None,
        server_socket_device: None,
        server_socket_inode: None,
        composite_track: TmuxCompositeTrack {
            alias: "test".to_owned(),
            session_id: None,
            session: None,
            lifecycle: TmuxRecordingLifecycle::Live,
        },
        panes: Vec::new(),
        pane_tracks: BTreeMap::new(),
        track_history: BTreeMap::new(),
        catalog: None,
        events: Vec::new(),
        lifecycle_journal: None,
        coverage_gaps: Vec::new(),
    };
}

#[test]
fn thousands_of_incarnations_have_bounded_snapshots_and_rebuildable_pages() {
    let temporary = TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("aggregate");
    create_private_directory(&directory).expect("private aggregate");
    let mut snapshot = manifest();
    let first = Uuid::now_v7();
    for index in 0..2500 {
        let id = if index == 0 { first } else { Uuid::now_v7() };
        snapshot.track_history.insert(
            id,
            TmuxPaneTrack {
                pane_id: "%1".to_owned(),
                session_id: id,
                session: ByteString::from(format!("/private/source-{index}").as_bytes()),
                linked_unix_ns: index + 1,
            },
        );
    }
    tmux_journal::append(&directory, &mut snapshot).expect("durable inventory");
    assert_eq!(snapshot.track_history.len(), 128);
    assert_eq!(snapshot.events.len(), 128);
    assert_eq!(snapshot.catalog.as_ref().expect("counts").tracks, 2500);
    let catalog = TmuxCatalog::read(&directory, &snapshot).expect("indexed snapshot");
    assert_eq!(
        catalog
            .track(first)
            .expect("lookup")
            .expect("old incarnation")
            .track
            .session_id,
        first
    );
    let page = catalog.tracks(2498, 3, false).expect("late page");
    assert_eq!(page.len(), 3);
    assert_eq!(page.first().expect("first").sequence, 2498);
    drop(catalog);
    let old = snapshot.clone();
    snapshot.events.push(TmuxLifecycleEvent {
        sequence: 2501,
        observed_unix_ns: 3000,
        kind: "pane_track_completed".to_owned(),
        pane_id: Some("%1".to_owned()),
        session_id: None,
        window_id: None,
        value: Some(first.to_string()),
    });
    tmux_journal::append(&directory, &mut snapshot).expect("append completion");
    assert_eq!(
        snapshot.catalog.as_ref().expect("counts").unfinished_tracks,
        2499
    );
    let older = TmuxCatalog::read(&directory, &old).expect("private older snapshot");
    assert!(
        !older
            .track(first)
            .expect("lookup")
            .expect("track")
            .completed
    );
    drop(older);
    let newer = TmuxCatalog::read(&directory, &snapshot).expect("newer shared index unchanged");
    assert!(
        newer
            .track(first)
            .expect("lookup")
            .expect("track")
            .completed
    );
    drop(newer);
    fs::remove_file(directory.join(CATALOG_NAME)).expect("remove acceleration index");
    let rebuilt = TmuxCatalog::read(&directory, &snapshot).expect("private rebuild from journal");
    assert_eq!(rebuilt.summary().tracks, 2500);
    assert!(
        rebuilt
            .track(first)
            .expect("lookup")
            .expect("track")
            .completed
    );
    drop(rebuilt);
    tmux_journal::append(&directory, &mut snapshot).expect("writer rebuilds missing index");
    assert!(directory.join(CATALOG_NAME).is_file());
}

#[test]
fn resolved_coverage_intervals_leave_snapshot_but_remain_queryable() {
    let temporary = TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("aggregate");
    create_private_directory(&directory).expect("private aggregate");
    let mut snapshot = manifest();
    for index in 0..300_u128 {
        snapshot.coverage_gaps.push(TmuxCoverageGap {
            pane_id: Some("%1".to_owned()),
            kind: "uncovered".to_owned(),
            detail: "process not recorded".to_owned(),
            first_observed_unix_ns: index * 2 + 1,
            last_observed_unix_ns: index * 2 + 2,
            resolved_unix_ns: Some(index * 2 + 2),
        });
    }
    tmux_journal::append(&directory, &mut snapshot).expect("coverage history");
    assert!(snapshot.coverage_gaps.is_empty());
    assert_eq!(snapshot.events.len(), 128);
    let catalog = TmuxCatalog::read(&directory, &snapshot).expect("catalog");
    assert_eq!(catalog.summary().coverage_gaps, 300);
    assert!(catalog.gaps(0, 100, true).expect("active gaps").is_empty());
    let page = catalog.gaps(591, 5, false).expect("last gaps");
    assert_eq!(page.len(), 5);
    assert_eq!(
        page.last().expect("last gap").gap.resolved_unix_ns,
        Some(600)
    );
}
