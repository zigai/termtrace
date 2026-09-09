use super::*;
use std::collections::BTreeMap;
use tempfile::TempDir;
use termtrace_evidence::tmux_catalog::CATALOG_NAME;
use termtrace_model::artifact::tmux::{
    TmuxCompositeTrack, TmuxPaneTrack, TmuxRecordingCoverage, TmuxRecordingLifecycle,
    TMUX_RECORDING_FORMAT, TMUX_RECORDING_VERSION,
};

#[test]
fn source_traversal_exceeds_the_legacy_limit_and_ignores_an_untrusted_index() {
    let temporary = TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("aggregate");
    create_private_directory(&directory).expect("aggregate directory");

    let mut aggregate = TmuxRecordingManifest {
        format: TMUX_RECORDING_FORMAT.to_owned(),
        version: TMUX_RECORDING_VERSION,
        recording_id: Uuid::now_v7(),
        name: "many".to_owned(),
        lifecycle: TmuxRecordingLifecycle::Completed,
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
            alias: "many".to_owned(),
            session_id: Some(Uuid::now_v7()),
            session: Some(ByteString::from(b"/source/composite".as_slice())),
            lifecycle: TmuxRecordingLifecycle::Completed,
        },
        panes: Vec::new(),
        pane_tracks: BTreeMap::new(),
        track_history: BTreeMap::new(),
        catalog: None,
        events: Vec::new(),
        lifecycle_journal: None,
        coverage_gaps: Vec::new(),
    };

    for index in 0..2500 {
        let id = Uuid::now_v7();
        aggregate.track_history.insert(
            id,
            TmuxPaneTrack {
                pane_id: "%1".to_owned(),
                session_id: id,
                session: ByteString::from(format!("/source/pane-{index}").as_bytes()),
                linked_unix_ns: index + 1,
            },
        );
    }

    tmux_journal::append(&directory, &mut aggregate).expect("canonical history");
    assert_eq!(aggregate.track_history.len(), 128);

    fs_err::write(directory.join(CATALOG_NAME), b"untrusted derived index").expect("replace cache");

    let path = directory.join("manifest.json");
    let mut count = 0;

    visit_source_tracks(&path, &aggregate, |source| {
        if count == 1 {
            assert_eq!(source.session, Path::new("/source/pane-0"));
        }

        count += 1;
        return Ok(());
    })
    .expect("all sources from authoritative history");

    assert_eq!(count, 2501);
}
