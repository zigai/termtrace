use super::validation::{
    validate_completion_format, validate_manifest, validate_replay_profile_file,
};
use super::*;
use crate::{
    event_log::{EventLog, EventLogWriter, SessionClock},
    provenance::{collect_execution_provenance, RecorderBuildIdentity},
    Error,
};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::Permissions,
    os::unix::fs::{symlink, PermissionsExt as _},
    path::Path,
};
use tempfile::TempDir;
use termtrace_model::execution::layout::{self, SessionLayout};
use termtrace_model::{
    execution::environment::ChildEnvironmentPolicy,
    execution::event::{Event, EventLogHeader, InputDescriptor, InputKind, InputSource},
    execution::event_log::DurabilityPolicy,
    execution::provenance::ProvenancePolicy,
    execution::session::*,
    execution::visual_profile::VisualProfile,
    value::ByteString,
};
use uuid::Uuid;

fn completed_fixture(events: Vec<Event>) -> (TempDir, EventLog) {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("session");
    fs_err::create_dir(&session).expect("session directory");
    fs_err::set_permissions(&session, Permissions::from_mode(0o700))
        .expect("secure session directory");

    let clock = SessionClock::now().expect("session clock");
    let session_id = Uuid::now_v7();
    let environment = BTreeMap::new();
    let child_environment = ChildEnvironmentPolicy {
        effective_digest: blake3::hash(b"termtrace-child-environment-v1\0")
            .to_hex()
            .to_string(),
        ..ChildEnvironmentPolicy::default()
    };
    let manifest = SessionManifest {
        failure_policy: Default::default(),
        format: SESSION_MANIFEST_FORMAT.to_owned(),
        version: SESSION_MANIFEST_VERSION,
        session_id,
        creation_alias: None,
        lineage: None,
        import_provenance: None,
        tmux_pane: None,
        started_unix_ns: clock.started_unix_ns(),
        command: vec![ByteString::from(&b"true"[..])],
        working_directory: Some(ByteString::from(&b"/tmp"[..])),
        geometry: SessionGeometry {
            cols: 80,
            rows: 24,
            pixel_width: 720,
            pixel_height: 432,
        },
        geometry_provenance: GeometryProvenance::default(),
        ghostty_visual: None,
        visual_profile: None,
        child_environment,
        execution_provenance: collect_execution_provenance(
            &ProvenancePolicy::default(),
            &environment,
            &[OsString::from("/bin/true")],
            Path::new("/"),
            RecorderBuildIdentity {
                version: env!("CARGO_PKG_VERSION"),
                source_revision: "test",
                semantic_engine_revision: "test",
            },
        ),
        termtrace_version: env!("CARGO_PKG_VERSION").to_owned(),
        operating_system: "linux".to_owned(),
        architecture: "test".to_owned(),
        event_log: "events.jsonl".to_owned(),
        control_socket: format!("termtrace-control-{session_id}"),
        request_journal: "requests.jsonl".to_owned(),
        contains_sensitive_terminal_data: true,
        durability_policy: DurabilityPolicy::default(),
        resource_limits: ResourceLimits::default(),
    };
    let manifest_bytes = canonical_manifest_bytes(&manifest).expect("manifest JSON");
    fs_err::write(session.join("session.json"), &manifest_bytes).expect("write manifest");

    let manifest_hash = manifest_hash(&manifest_bytes);
    let header = EventLogHeader::new(
        session_id,
        &manifest_hash,
        clock.started_unix_ns(),
        80,
        24,
        720,
        432,
        "xterm-ghostty",
    );

    let header_bytes = serde_json::to_vec(&header).expect("header JSON");

    let mut writer =
        EventLogWriter::create(session.join("events.jsonl"), &header, clock).expect("event log");

    for event in events {
        writer.append(event).expect("append event");
    }

    let event_count = writer.next_seq();
    let final_event_hash = writer.current_hash();
    let durability = writer.durability_record();
    drop(writer);

    let completion = SessionCompletion {
        format: SESSION_COMPLETION_FORMAT.to_owned(),
        version: SESSION_COMPLETION_VERSION,
        validation_version: COMPLETION_VALIDATION_VERSION,
        session_id,
        session_manifest_hash: manifest_hash,
        event_header_hash: blake3::hash(&header_bytes).to_hex().to_string(),
        exit_code: None,
        signal_name: None,
        output_bytes: 0,
        final_event_hash,
        event_count,
        terminal_event_seq: event_count.saturating_sub(1),
        durability,
        resource_limit: None,
        failure: None,
    };

    fs_err::write(
        session.join("completion.json"),
        serde_json::to_vec(&completion).expect("completion JSON"),
    )
    .expect("write completion");
    let log = EventLog::read(session.join("events.jsonl")).expect("read event log");

    return (temporary, log);
}

#[test]
fn manifest_validation_rejects_noncanonical_ghostty_profile_contracts() {
    let (temporary, _) = completed_fixture(Vec::new());
    let session = temporary.path().join("session");
    let mut manifest = read_manifest(&session).expect("session manifest");
    manifest.ghostty_visual = Some(GhosttyVisualProvenance {
        source_config: None,
        overrides: GhosttyVisualOverrides::default(),
        replay_profile: layout::GHOSTTY_REPLAY_PROFILE.to_owned(),
        replay_profile_hash: "a".repeat(64),
        effective_config_hash: "0".repeat(64),
    });
    validate_manifest(&manifest).expect("canonical Ghostty profile contract");

    let mut fitted = manifest.clone();
    fitted.geometry_provenance = GeometryProvenance {
        source: GeometrySource::GhosttyFit,
        capture_target: Some(CaptureTarget::new(1280, 720)),
    };
    validate_manifest(&fitted).expect("complete fitted-geometry provenance");

    let mut missing_target = fitted.clone();
    missing_target.geometry_provenance.capture_target = None;

    assert!(matches!(
        validate_manifest(&missing_target),
        Err(Error::InvalidEvidence { .. })
    ));

    let mut missing_profile = fitted;
    missing_profile.ghostty_visual = None;

    assert!(matches!(
        validate_manifest(&missing_profile),
        Err(Error::InvalidEvidence { .. })
    ));

    let mut invalid_path = manifest.clone();
    invalid_path
        .ghostty_visual
        .as_mut()
        .expect("Ghostty provenance")
        .replay_profile = "nested/ghostty-profile.ghostty".to_owned();

    assert!(matches!(
        validate_manifest(&invalid_path),
        Err(Error::InvalidEvidence { .. })
    ));

    let mut invalid_replay_hash = manifest.clone();
    invalid_replay_hash
        .ghostty_visual
        .as_mut()
        .expect("Ghostty provenance")
        .replay_profile_hash = "A".repeat(64);

    assert!(matches!(
        validate_manifest(&invalid_replay_hash),
        Err(Error::InvalidEvidence { .. })
    ));

    manifest
        .ghostty_visual
        .as_mut()
        .expect("Ghostty provenance")
        .effective_config_hash = "f".repeat(63);

    assert!(matches!(
        validate_manifest(&manifest),
        Err(Error::InvalidEvidence { .. })
    ));
}

#[test]
fn manifest_version_dispatch_keeps_profileless_sessions_in_the_earlier_format() {
    let (temporary, _) = completed_fixture(Vec::new());
    let session = temporary.path().join("session");
    let legacy = read_manifest(&session).expect("session manifest");
    assert_eq!(legacy.version, SESSION_MANIFEST_VERSION);
    assert!(!serde_json::to_string(&legacy)
        .expect("manifest JSON")
        .contains("visual_profile"));

    validate_manifest(&legacy).expect("profileless manifest keeps its version");

    let mut semantic = legacy.clone();
    semantic.visual_profile = Some(VisualProfile::semantic_bundled().record().expect("valid"));

    assert!(matches!(
        validate_manifest(&semantic),
        Err(Error::InvalidEvidence { .. })
    ));

    semantic.version = SESSION_MANIFEST_VISUAL_PROFILE_VERSION;
    validate_manifest(&semantic).expect("profiled manifest uses the profile version");

    let mut versioned_without_profile = legacy;
    versioned_without_profile.version = SESSION_MANIFEST_VISUAL_PROFILE_VERSION;

    assert!(matches!(
        validate_manifest(&versioned_without_profile),
        Err(Error::InvalidEvidence { .. })
    ));
}

#[test]
fn manifest_validation_binds_the_visual_profile_to_its_hash_and_ghostty_provenance() {
    let (temporary, _) = completed_fixture(Vec::new());
    let session = temporary.path().join("session");
    let standard = VisualProfile::native_standard();
    let native = standard.native.clone().expect("native section");
    let mut manifest = read_manifest(&session).expect("session manifest");
    manifest.version = SESSION_MANIFEST_VISUAL_PROFILE_VERSION;
    manifest.visual_profile = Some(standard.record().expect("valid"));
    manifest.ghostty_visual = Some(GhosttyVisualProvenance {
        source_config: None,
        overrides: native.overrides(),
        replay_profile: layout::GHOSTTY_REPLAY_PROFILE.to_owned(),
        replay_profile_hash: "a".repeat(64),
        effective_config_hash: "0".repeat(64),
    });
    validate_manifest(&manifest).expect("consistent native profile");

    let mut tampered = manifest.clone();
    if let Some(recorded) = tampered.visual_profile.as_mut() {
        recorded.profile_hash = "f".repeat(64);
    }

    assert!(matches!(
        validate_manifest(&tampered),
        Err(Error::InvalidEvidence { .. })
    ));

    let mut different_overrides = manifest.clone();
    if let Some(provenance) = different_overrides.ghostty_visual.as_mut() {
        provenance.overrides.font_size = Some("16".to_owned());
    }

    assert!(matches!(
        validate_manifest(&different_overrides),
        Err(Error::InvalidEvidence { .. })
    ));

    let mut missing_provenance = manifest;
    missing_provenance.ghostty_visual = None;

    assert!(matches!(
        validate_manifest(&missing_provenance),
        Err(Error::InvalidEvidence { .. })
    ));
}

#[test]
fn completed_content_validation_binds_the_replay_profile_file() {
    let (temporary, _) = completed_fixture(Vec::new());
    let session = temporary.path().join("session");
    let profile_path = session.join(layout::GHOSTTY_REPLAY_PROFILE);
    let profile_bytes = b"font-size = 13\n";
    fs_err::write(&profile_path, profile_bytes).expect("replay profile");

    let mut manifest = read_manifest(&session).expect("session manifest");
    manifest.ghostty_visual = Some(GhosttyVisualProvenance {
        source_config: None,
        overrides: GhosttyVisualOverrides::default(),
        replay_profile: layout::GHOSTTY_REPLAY_PROFILE.to_owned(),
        replay_profile_hash: blake3::hash(profile_bytes).to_hex().to_string(),
        effective_config_hash: "0".repeat(64),
    });
    validate_replay_profile_file(&session, &manifest).expect("matching replay profile");

    fs_err::write(&profile_path, b"font-size = 14\n").expect("tampered replay profile");

    let error =
        validate_replay_profile_file(&session, &manifest).expect_err("profile hash must bind");
    assert!(matches!(error, Error::InvalidEvidence { .. }));
}

#[test]
fn completed_validation_requires_terminal_exit() {
    let (temporary, _) = completed_fixture(vec![Event::Marker {
        operation_id: Uuid::now_v7(),
        label: "not-an-exit".to_owned(),
    }]);

    let error = validate_completed_session(&temporary.path().join("session"))
        .expect_err("nonterminal completion must fail");
    assert!(matches!(error, Error::InvalidEvidence { .. }));
    let report = verify_session(temporary.path().join("session"), VerificationLevel::Content);
    assert!(!report.valid);
    assert_eq!(report.findings[0].code, "terminal_exit_missing");
}

#[test]
fn completed_validation_rejects_pending_mutation() {
    let operation_id = Uuid::now_v7();
    let (temporary, _) = completed_fixture(vec![
        Event::InputIntent {
            operation_id,
            source: InputSource::Agent,
            input_kind: InputKind::Text,
            descriptor: InputDescriptor::Text,
            terminal_modes_hash: None,
            data: ByteString::from(&b"x"[..]),
            semantic_input: Some("text".to_owned()),
            caller_intent: None,
        },
        Event::Exit {
            code: None,
            signal: None,
            signal_name: None,
        },
    ]);

    let error = validate_completed_session(&temporary.path().join("session"))
        .expect_err("pending mutation must fail");
    assert!(matches!(error, Error::InvalidEvidence { .. }));
    let report = verify_session(temporary.path().join("session"), VerificationLevel::Content);
    assert!(!report.valid);
    assert_eq!(report.findings[0].code, "pending_mutation_intents");
}

#[test]
fn session_readers_reject_a_second_json_document() {
    for (filename, is_manifest) in [("session.json", true), ("completion.json", false)] {
        let (temporary, _) = completed_fixture(vec![Event::Exit {
            code: None,
            signal: None,
            signal_name: None,
        }]);

        let session = temporary.path().join("session");
        assert!(verify_session(&session, VerificationLevel::Content).valid);
        let path = session.join(filename);
        let mut bytes = fs_err::read(&path).expect("session document");
        bytes.extend_from_slice(b"{}");
        fs_err::write(path, bytes).expect("append second document");

        let result = if is_manifest {
            read_manifest(&session).map(drop)
        } else {
            read_completion(&session).map(drop)
        };
        assert!(
            matches!(result, Err(Error::InvalidJson { source, .. })
            if source.is_syntax()),
            "{filename} accepted a second document"
        );
        let report = verify_session(&session, VerificationLevel::Content);
        assert!(!report.valid);
        assert_eq!(
            report.findings[0].code,
            if is_manifest {
                "session_manifest_invalid"
            } else {
                "completion_invalid_or_missing"
            }
        );
    }
}

#[derive(serde::Deserialize)]
struct RejectedVersionCorpus {
    format: String,
    version: u32,
    cases: Vec<RejectedVersionCase>,
}

#[derive(serde::Deserialize)]
struct RejectedVersionCase {
    surface: String,
    rejected_version: u32,
    expected_error: String,
}

#[test]
fn persisted_format_readers_reject_version_corpus_without_compatibility_fallbacks() {
    let corpus: RejectedVersionCorpus = serde_json::from_str(include_str!(
        "../../../../../fixtures/conformance/protocol/rejected-versions.json"
    ))
    .expect("rejected version corpus");
    assert_eq!(corpus.format, "termtrace-rejected-version-corpus");
    assert_eq!(corpus.version, 1);

    for case in &corpus.cases {
        let surface = case.surface.as_str();
        let rejected_version = case.rejected_version;
        let expected_error = case.expected_error.as_str();
        let (temporary, _) = completed_fixture(Vec::new());
        let session = temporary.path().join("session");
        let error = match surface {
            "event_log" => {
                let path = session.join("events.jsonl");
                let encoded = fs_err::read_to_string(&path).expect("event log");
                let mut header: serde_json::Value =
                    serde_json::from_str(encoded.trim_end()).expect("event header");
                header["version"] = serde_json::json!(rejected_version);
                let mut rejected = serde_json::to_vec(&header).expect("rejected event header");
                rejected.push(b'\n');
                fs_err::write(&path, rejected).expect("write rejected event header");

                EventLog::read(&path).expect_err("old event version must be rejected")
            }
            "session_manifest" => {
                let mut manifest = read_manifest(&session).expect("session manifest");
                manifest.version = rejected_version;

                validate_manifest(&manifest).expect_err("old session version must be rejected")
            }
            "completion_manifest" => {
                let mut completion = read_completion(&session).expect("completion manifest");
                completion.version = rejected_version;

                validate_completion_format(&completion)
                    .expect_err("old completion version must be rejected")
            }
            "completion_validation" => {
                let mut completion = read_completion(&session).expect("completion manifest");
                completion.validation_version = rejected_version;

                validate_completion_format(&completion)
                    .expect_err("old completion validation version must be rejected")
            }
            other => panic!("unknown rejected-version surface `{other}`"),
        };
        assert!(
            error.to_string().contains(expected_error),
            "{surface} emitted unexpected rejection: {error}"
        );
    }
}

fn recovery_source_hashes(source: &Path) -> BTreeMap<OsString, String> {
    return fs_err::read_dir(source)
        .expect("source directory")
        .map(|entry| {
            let entry = entry.expect("source member");
            let bytes = fs_err::read(entry.path()).expect("immutable source member");

            return (entry.file_name(), blake3::hash(&bytes).to_hex().to_string());
        })
        .collect();
}

#[test]
fn recovery_utf8_preserves_historical_report_and_diagnostic_bytes_and_hashes() {
    let (temporary, _) = completed_fixture(vec![Event::Exit {
        code: Some(0),
        signal: None,
        signal_name: None,
    }]);

    let source = temporary.path().join("source-é-\"-\\");
    fs_err::rename(temporary.path().join("session"), &source).expect("UTF8 escaping fixture");
    let destination = temporary.path().join("recovered-λ-\"-\\");
    let before = recovery_source_hashes(&source);
    let report = recover_session(&source, &destination).expect("recover UTF8 source");
    let legacy_report = serde_json::json!({
        "source": source.join("events.jsonl"),
        "recovered": destination.join("events.jsonl"),
        "valid_events": report.valid_events,
        "discarded_tail": report.discarded_tail,
        "pending_operations": report.pending_operations,
    });

    // Direct report serialization retains historical struct field order too.
    let expected_direct = format!(
        "{{\"source\":{},\"recovered\":{},\"valid_events\":{},\"discarded_tail\":null,\"pending_operations\":[]}}",
        serde_json::to_string(&source.join("events.jsonl")).expect("legacy source"),
        serde_json::to_string(&destination.join("events.jsonl")).expect("legacy destination"),
        report.valid_events,
    );

    assert_eq!(
        serde_json::to_vec(&report).expect("report"),
        expected_direct.as_bytes()
    );
    let manifest = read_manifest(&source).expect("manifest");
    let expected = serde_json::to_vec(&serde_json::json!({
        "format": "termtrace-session-recovery", "version": 1, "complete_evidence": false,
        "source_session": source, "session_id": manifest.session_id,
        "session_manifest_hash": manifest_hash(&fs_err::read(source.join("session.json")).expect("source manifest")),
        "recovery": legacy_report,
    })).expect("legacy diagnostic bytes");

    let actual =
        fs_err::read(SessionLayout::new(&destination).recovery_diagnostic()).expect("diagnostic");
    assert_eq!(blake3::hash(&actual), blake3::hash(&expected));
    assert_eq!(actual, expected);

    let mut wide = report;
    wide.valid_events = u64::MAX;
    let encoded = serde_json::to_string(&wide).expect("wide report");
    assert!(encoded.contains("\"valid_events\":18446744073709551615"));
    assert_eq!(before, recovery_source_hashes(&source));
    assert!(!destination.join("completion.json").exists());

    EventLog::read(destination.join("events.jsonl")).expect("validated recovered prefix");
}

#[test]
fn recovery_non_utf8_binds_exact_bytes_for_both_paths_without_rewriting_source() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt as _};
    let (temporary, _) = completed_fixture(vec![Event::Exit {
        code: Some(0),
        signal: None,
        signal_name: None,
    }]);

    let source = temporary.path().join(OsStr::from_bytes(b"source-\xff"));
    fs_err::rename(temporary.path().join("session"), &source).expect("nonUTF8 source");
    let destination = temporary.path().join(OsStr::from_bytes(b"recovered-\xfe"));
    let before = recovery_source_hashes(&source);
    let report =
        recover_session(&source, &destination).expect("recover nonUTF8 source and destination");
    assert_eq!(report.source, source.join("events.jsonl"));
    assert_eq!(report.recovered, destination.join("events.jsonl"));

    let serialized = serde_json::to_value(&report).expect("byte-bound report");
    let diagnostic: serde_json::Value = serde_json::from_slice(
        &fs_err::read(SessionLayout::new(&destination).recovery_diagnostic()).expect("diagnostic"),
    )
    .expect("byte-bound diagnostic");

    for (value, path) in [
        (&diagnostic["source_session"], source.as_path()),
        (&serialized["source"], report.source.as_path()),
        (&serialized["recovered"], report.recovered.as_path()),
    ] {
        assert_eq!(*value, serde_json::json!(path.as_os_str().as_bytes()));
    }

    assert_eq!(diagnostic["recovery"], serialized);
    assert_eq!(before, recovery_source_hashes(&source));

    EventLog::read(destination.join("events.jsonl")).expect("validated recovered prefix");

    assert!(!destination.join("completion.json").exists());

    let published = recovery_source_hashes(&destination);

    assert!(recover_session(&source, &destination).is_err());
    assert_eq!(published, recovery_source_hashes(&destination));
    assert_eq!(before, recovery_source_hashes(&source));
    assert!(recover_session(&source, &source).is_err());
    assert!(recover_session(&source, &source.join("nested-recovery")).is_err());
    assert!(!source.join("nested-recovery").exists());

    let alias = temporary.path().join("source-link");
    symlink(&source, &alias).expect("private source alias");

    assert!(recover_session(&source, &alias.join("nested-recovery")).is_err());
    assert_eq!(before, recovery_source_hashes(&source));
}

#[test]
fn recovery_non_utf8_failure_leaves_destination_absent_and_source_unchanged() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt as _};
    let (temporary, _) = completed_fixture(Vec::new());
    let source = temporary.path().join(OsStr::from_bytes(b"broken-\xff"));
    fs_err::rename(temporary.path().join("session"), &source).expect("nonUTF8 source");
    fs_err::write(source.join("events.jsonl"), b"invalid header\n")
        .expect("corrupt private fixture");

    let before = recovery_source_hashes(&source);
    let destination = temporary.path().join(OsStr::from_bytes(b"absent-\xfe"));
    assert!(recover_session(&source, &destination).is_err());
    assert!(!destination.exists());
    assert_eq!(before, recovery_source_hashes(&source));
    assert!(fs_err::read_dir(temporary.path())
        .expect("private root")
        .all(|entry| {
            return !entry
                .expect("private member")
                .file_name()
                .as_bytes()
                .starts_with(b".termtrace-recovery-");
        }));
}
