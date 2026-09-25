use super::*;
use std::collections::BTreeMap;
use tempfile::TempDir;
use termtrace_tmux::{
    artifact::{
        TmuxCatalogSummary, TmuxCompositeTrack, TmuxEvent, TmuxJournalBoundary, TmuxPaneTrack,
        TmuxRecordingCoverage, TmuxRecordingLifecycle, TmuxRecordingState, TmuxSessionLink,
        TmuxSocket, TMUX_RECORDING_FORMAT, TMUX_RECORDING_VERSION,
    },
    catalog::CATALOG_NAME,
    store::RecordingStore,
};

#[test]
fn source_traversal_reads_complete_history_and_ignores_an_untrusted_index() {
    let temporary = TempDir::new().expect("temporary directory");
    let store = RecordingStore::create(temporary.path(), "many").expect("aggregate directory");

    let aggregate = TmuxRecordingManifest {
        format: TMUX_RECORDING_FORMAT.to_owned(),
        version: TMUX_RECORDING_VERSION,
        recording_id: Uuid::now_v7(),
        name: "many".to_owned(),
        created_unix_ns: 1,
        coverage: TmuxRecordingCoverage::Warn,
        tmux_executable: ByteString::from(b"tmux".as_slice()),
        socket: TmuxSocket::Default,
        state: TmuxRecordingState {
            lifecycle: TmuxRecordingLifecycle::Completed,
            composite_track: TmuxCompositeTrack {
                alias: "many".to_owned(),
                link: Some(TmuxSessionLink {
                    session_id: Uuid::now_v7(),
                    session: ByteString::from(b"/source/composite".as_slice()),
                }),
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
    let events = (0..2500_u64)
        .map(|index| {
            return TmuxEvent::PaneTrackLinked {
                track: TmuxPaneTrack {
                    pane_id: "%1".to_owned(),
                    session_id: Uuid::now_v7(),
                    session: ByteString::from(format!("/source/pane-{index}").as_bytes()),
                    linked_unix_ns: index + 1,
                },
            };
        })
        .collect();
    store
        .initialize(aggregate, events, 2)
        .expect("canonical history");
    let aggregate = store.read().expect("snapshot");

    fs_err::write(
        store.directory().join(CATALOG_NAME),
        b"untrusted derived index",
    )
    .expect("replace cache");
    let mut count = 0;

    visit_source_tracks(&store.manifest_path(), &aggregate, |source| {
        if count == 1 {
            assert_eq!(source.session, Path::new("/source/pane-0"));
        }

        count += 1;

        return Ok(());
    })
    .expect("all sources from authoritative history");

    assert_eq!(count, 2501);
}
