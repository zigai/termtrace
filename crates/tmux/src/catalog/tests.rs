use super::*;
use crate::{
    artifact::{
        TmuxCompositeTrack, TmuxCoverageGap, TmuxGapKind, TmuxRecordingCoverage,
        TmuxRecordingLifecycle, TmuxRecordingState, TmuxSocket, TMUX_RECORDING_FORMAT,
        TMUX_RECORDING_VERSION,
    },
    store::RecordingStore,
};
use std::{collections::BTreeMap, fs};
use tempfile::TempDir;
use termtrace_model::value::ByteString;

fn manifest() -> TmuxRecordingManifest {
    return TmuxRecordingManifest {
        format: TMUX_RECORDING_FORMAT.to_owned(),
        version: TMUX_RECORDING_VERSION,
        recording_id: Uuid::now_v7(),
        name: "test".to_owned(),
        created_unix_ns: 1,
        coverage: TmuxRecordingCoverage::Warn,
        tmux_executable: ByteString::from(b"tmux".as_slice()),
        socket: TmuxSocket::Default,
        state: TmuxRecordingState {
            lifecycle: TmuxRecordingLifecycle::Live,
            composite_track: TmuxCompositeTrack {
                alias: "test".to_owned(),
                link: None,
            },
            server: None,
            panes: Vec::new(),
            pane_tracks: BTreeMap::new(),
            coverage_gaps: Vec::new(),
            updated_unix_ns: 1,
        },
        lifecycle_journal: TmuxJournalBoundary::empty(),
        catalog: TmuxCatalogSummary::default(),
    };
}

fn store(temporary: &TempDir) -> RecordingStore {
    return RecordingStore::create(temporary.path(), "test").expect("private aggregate");
}

#[test]
fn thousands_of_incarnations_have_bounded_snapshots_and_rebuildable_pages() {
    let temporary = TempDir::new().expect("temporary directory");
    let store = store(&temporary);
    let first = Uuid::now_v7();
    let events = (0..2500_u64)
        .map(|index| {
            let session_id = if index == 0 { first } else { Uuid::now_v7() };

            return TmuxEvent::PaneTrackLinked {
                track: TmuxPaneTrack {
                    pane_id: "%1".to_owned(),
                    session_id,
                    session: ByteString::from(format!("/private/source-{index}").as_bytes()),
                    linked_unix_ns: index + 1,
                },
            };
        })
        .collect();
    store
        .initialize(manifest(), events, 2)
        .expect("durable inventory");

    let snapshot = store.read().expect("snapshot");
    assert_eq!(snapshot.catalog.tracks, 2500);
    assert!(
        fs::metadata(store.manifest_path())
            .expect("snapshot size")
            .len()
            < 4096,
        "history never enters the snapshot"
    );

    let catalog = TmuxCatalog::read(store.directory(), &snapshot).expect("indexed snapshot");
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

    let old = snapshot;
    {
        let mut locked = store.lock().expect("writer");
        let state = locked.manifest().state.clone();
        locked
            .commit(
                state,
                vec![TmuxEvent::PaneTrackCompleted {
                    pane_id: "%1".to_owned(),
                    session_id: first,
                }],
                3000,
            )
            .expect("append completion");
    }
    let snapshot = store.read().expect("snapshot");

    assert_eq!(snapshot.catalog.unfinished_tracks, 2499);

    let older = TmuxCatalog::read(store.directory(), &old).expect("private older snapshot");
    assert!(
        !older
            .track(first)
            .expect("lookup")
            .expect("track")
            .completed
    );

    drop(older);

    let newer =
        TmuxCatalog::read(store.directory(), &snapshot).expect("newer shared index unchanged");
    assert!(
        newer
            .track(first)
            .expect("lookup")
            .expect("track")
            .completed
    );

    drop(newer);

    fs::remove_file(store.directory().join(CATALOG_NAME)).expect("remove acceleration index");
    let rebuilt =
        TmuxCatalog::read(store.directory(), &snapshot).expect("private rebuild from journal");
    assert_eq!(rebuilt.summary().tracks, 2500);
    assert!(
        rebuilt
            .track(first)
            .expect("lookup")
            .expect("track")
            .completed
    );

    drop(rebuilt);

    let mut locked = store.lock().expect("writer");
    let state = locked.manifest().state.clone();
    locked
        .commit(state, vec![TmuxEvent::RecordingStopping], 3001)
        .expect("writer rebuilds missing index");

    assert!(store.directory().join(CATALOG_NAME).is_file());
}

#[test]
fn resolved_coverage_intervals_remain_queryable() {
    let temporary = TempDir::new().expect("temporary directory");
    let store = store(&temporary);
    let mut events = Vec::new();

    for index in 0..300_u64 {
        let mut gap = TmuxCoverageGap::open(
            Some("%1".to_owned()),
            TmuxGapKind::UncoveredPane,
            index * 2 + 1,
        );
        events.push(TmuxEvent::CoverageGapOpened { gap: gap.clone() });
        gap.last_observed_unix_ns = index * 2 + 2;
        gap.resolved_unix_ns = Some(index * 2 + 2);
        events.push(TmuxEvent::CoverageGapResolved { gap });
    }

    store
        .initialize(manifest(), events, 2)
        .expect("coverage history");

    let snapshot = store.read().expect("snapshot");
    let catalog = TmuxCatalog::read(store.directory(), &snapshot).expect("catalog");
    assert_eq!(catalog.summary().coverage_gaps, 300);
    assert!(catalog.gaps(0, 100, true).expect("active gaps").is_empty());

    let page = catalog.gaps(591, 5, false).expect("last gaps");
    assert_eq!(page.len(), 5);
    assert_eq!(
        page.last().expect("last gap").gap.resolved_unix_ns,
        Some(600)
    );
}

#[test]
fn an_interrupted_publication_suffix_is_discarded() {
    let temporary = TempDir::new().expect("temporary directory");
    let store = store(&temporary);
    store
        .initialize(manifest(), vec![TmuxEvent::RecordingStopping], 2)
        .expect("history");
    let boundary = store.read().expect("snapshot").lifecycle_journal;

    {
        use std::io::Write as _;

        fs::OpenOptions::new()
            .append(true)
            .open(store.journal_path())
            .expect("journal")
            .write_all(b"interrupted uncommitted suffix")
            .expect("partial write");
    }

    let mut locked = store.lock().expect("writer");
    let state = locked.manifest().state.clone();
    locked.commit(state, Vec::new(), 3).expect("recover");

    assert_eq!(
        fs::metadata(store.journal_path()).expect("journal").len(),
        boundary.bytes
    );
}

#[test]
fn a_released_recording_frees_its_name() {
    let temporary = TempDir::new().expect("temporary directory");
    let created = store(&temporary);

    assert!(matches!(
        RecordingStore::create(temporary.path(), "test"),
        Err(Error::Exists(_))
    ));

    created.release().expect("release");
    store(&temporary);
}
