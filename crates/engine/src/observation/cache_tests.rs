use super::*;
use crate::{
    observation::observe_session,
    recording::{record_with_output, RecordOptions},
};
use std::{collections::BTreeMap, ffi::OsString, io};
use tempfile::TempDir;
use termtrace_evidence::session::read_completion;
use termtrace_model::{
    execution::{event::TerminalColors, session::RecordingFailurePolicy},
    terminal::observation::Observation,
};
use zstd::stream::decode_all;

fn fixture(script: &str) -> TempDir {
    let root = TempDir::new().expect("private fixture");
    let options = RecordOptions {
        failure_policy: RecordingFailurePolicy::Stop,
        output_directory: root.path().join("source"),
        creation_alias: None,
        lineage: None,
        import_provenance: None,
        tmux_pane: None,
        command: ["/bin/sh", "-c", script]
            .into_iter()
            .map(OsString::from)
            .collect(),
        working_directory: None,
        cols: 80,
        rows: 24,
        cell_width_px: 9,
        cell_height_px: 18,
        geometry_provenance: Default::default(),
        ghostty_profile: None,
        visual_profile: None,
        term: "xterm-ghostty".into(),
        environment: Default::default(),
        colors: TerminalColors::default(),
        forward_stdin: false,
        continue_after_output_mirror_closes: false,
        durability_policy: Default::default(),
        provenance_policy: Default::default(),
        resource_limits: Default::default(),
    };

    record_with_output(&options, &mut io::sink()).expect("actual completed PTY recording");

    return root;
}

fn hashes(source: &Path) -> BTreeMap<String, String> {
    return fs_err::read_dir(source)
        .expect("source directory")
        .filter_map(|item| {
            let item = item.expect("source member");
            if item.file_type().expect("member type").is_dir() {
                return None;
            }
            return Some((
                item.file_name().to_string_lossy().into_owned(),
                blake3::hash(&fs_err::read(item.path()).expect("source bytes"))
                    .to_hex()
                    .to_string(),
            ));
        })
        .collect();
}

fn normalized(mut observation: Observation) -> Observation {
    observation.resolution = super::super::ObservationResolution::CanonicalReplay;

    return observation;
}

fn observe(source: &Path) -> Observation {
    return observe_session(source, StatePoint::LatestPresented).expect("completed observation");
}

#[test]
fn cold_warm_late_and_pruned_cache_preserve_exact_observation_facts() {
    let root = fixture("printf 'CACHE\\n'");
    let source = root.path().join("source");
    let before = hashes(&source);
    prune_replay_cache(&source).expect("discard optional recorder checkpoint");
    let expected = normalized(observe(&source));
    assert!(expected.event_seq.is_some());
    assert!(expected.monotonic_ns.is_some());
    assert_eq!(expected, normalized(observe(&source)));

    let completion = read_completion(&source).expect("completion");
    let mut cached = load_checkpoint(&source, &StatePoint::LatestPresented, &completion)
        .expect("warm cache")
        .expect("position-bound cache");
    cached.event_seq = None;
    cached.event_byte_offset = None;
    cached.monotonic_ns = None;
    prune_replay_cache(&source).expect("simulate cold query before late publisher wins");
    let validated = ValidatedSession::open(&source).expect("source authority");
    publish_cached_checkpoint(&validated, &StatePoint::LatestPresented, &cached)
        .expect("late authenticated recorder-style endpoint");

    assert!(
        load_checkpoint(&source, &StatePoint::LatestPresented, &completion)
            .expect("conservative miss")
            .is_none()
    );
    assert_eq!(expected, normalized(observe(&source)));
    assert_eq!(expected, normalized(observe(&source)));

    prune_replay_cache(&source).expect("explicit prune");

    assert_eq!(expected, normalized(observe(&source)));
    assert_eq!(before, hashes(&source));
}

#[test]
fn either_missing_position_rejects_authenticated_older_style_endpoints() {
    let root = fixture("printf POSITION");
    let source = root.path().join("source");
    prune_replay_cache(&source).expect("cold fixture");
    let expected = normalized(observe(&source));
    let completion = read_completion(&source).expect("completion");
    let cached = load_checkpoint(&source, &StatePoint::LatestPresented, &completion)
        .expect("cache")
        .expect("bound endpoint");

    let validated = ValidatedSession::open(&source).expect("authority");

    for missing_event in [true, false] {
        let mut unbound = cached.clone();
        if missing_event {
            unbound.event_seq = None;
        } else {
            unbound.monotonic_ns = None;
        }

        prune_replay_cache(&source).expect("private cache replacement");

        publish_cached_checkpoint(&validated, &StatePoint::LatestPresented, &unbound)
            .expect("authenticated legacy style");

        assert_eq!(expected, normalized(observe(&source)));
    }
}

#[test]
fn older_version_and_tampered_authentication_still_require_canonical_replay() {
    let root = fixture("printf AUTHENTICATED");
    let source = root.path().join("source");
    let before = hashes(&source);
    prune_replay_cache(&source).expect("cold fixture");
    let expected = normalized(observe(&source));
    let completion = read_completion(&source).expect("completion");
    let path = checkpoint_path(&source, &StatePoint::LatestPresented, &completion).expect("path");
    let compressed = fs_err::read(&path).expect("cache bytes");
    let bytes = decode_all(compressed.as_slice()).expect("cache JSON");
    let mut checkpoint: ReplayCheckpoint =
        trust::decode(&source, &bytes).expect("authenticated payload");
    checkpoint.version = REPLAY_CHECKPOINT_VERSION - 1;
    let validated = ValidatedSession::open(&source).expect("authority");
    let older = trust::encode(&validated, checkpoint).expect("authenticated older version");
    prune_replay_cache(&source).expect("replace disposable fixture only");

    write_secure(
        &path,
        &encode_all(older.as_slice(), 3).expect("compression"),
    )
    .expect("older cache");

    assert_eq!(expected, normalized(observe(&source)));

    let mut tampered: serde_json::Value = serde_json::from_slice(&bytes).expect("fixture envelope");
    tampered["mac"] = serde_json::Value::String("0".repeat(64));
    let tampered = serde_json::to_vec(&tampered).expect("tampered envelope");
    prune_replay_cache(&source).expect("replace disposable fixture only");

    write_secure(
        &path,
        &encode_all(tampered.as_slice(), 3).expect("compression"),
    )
    .expect("tampered cache");

    assert_eq!(expected, normalized(observe(&source)));
    assert_eq!(before, hashes(&source));
}

#[test]
fn genuinely_absent_initial_positions_remain_absent_after_repeated_replay() {
    let root = fixture("true");
    let source = root.path().join("source");
    let before = hashes(&source);

    for _ in 0..3 {
        let observation = observe(&source);
        assert_eq!(observation.event_seq, None);
        assert_eq!(observation.monotonic_ns, None);
        assert_eq!(observation.state_id, None);
    }

    prune_replay_cache(&source).expect("empty-state prune");

    assert_eq!(observe(&source).event_seq, None);
    assert_eq!(before, hashes(&source));
}
