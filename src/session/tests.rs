use std::ffi::OsString;
use std::{fs::Permissions, os::unix::fs::PermissionsExt as _};

use tempfile::TempDir;

use super::validation::validate_replay_profile_file;
use super::*;
use crate::environment::EnvironmentSpec;
use crate::provenance::{ProvenancePolicy, collect_execution_provenance};
use crate::{
    control::control_socket_name,
    event::{EventLogHeader, InputDescriptor, InputKind, InputSource},
    event_log::{EventLogWriter, SessionClock},
};

fn completed_fixture(events: Vec<Event>) -> (TempDir, EventLog) {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("session");
    fs_err::create_dir(&session).expect("session directory");
    fs_err::set_permissions(&session, Permissions::from_mode(0o700))
        .expect("secure session directory");
    let clock = SessionClock::now().expect("session clock");
    let session_id = Uuid::now_v7();
    let environment = EnvironmentSpec::default()
        .resolve("xterm-ghostty")
        .expect("environment");
    let manifest = SessionManifest {
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
        child_environment: environment.policy().clone(),
        execution_provenance: collect_execution_provenance(
            &ProvenancePolicy::default(),
            &environment,
            &[OsString::from("/bin/true")],
            Path::new("/"),
        ),
        termtrace_version: env!("CARGO_PKG_VERSION").to_owned(),
        operating_system: "linux".to_owned(),
        architecture: "test".to_owned(),
        event_log: "events.jsonl".to_owned(),
        control_socket: control_socket_name(session_id),
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
fn ghostty_profile_provenance_retains_the_split_hash_contract() {
    assert_eq!(SESSION_MANIFEST_VERSION, 12);
    assert_eq!(GHOSTTY_REPLAY_PROFILE_NAME, "ghostty-profile.ghostty");

    let provenance = GhosttyVisualProvenance {
        source_config: None,
        overrides: GhosttyVisualOverrides::default(),
        replay_profile: GHOSTTY_REPLAY_PROFILE_NAME.to_owned(),
        replay_profile_hash: "replay-hash".to_owned(),
        effective_config_hash: "effective-hash".to_owned(),
    };
    let encoded = serde_json::to_value(&provenance).expect("profile provenance JSON");
    assert_eq!(
        encoded.get("replay_profile"),
        Some(&serde_json::json!(GHOSTTY_REPLAY_PROFILE_NAME))
    );
    assert_eq!(
        encoded.get("replay_profile_hash"),
        Some(&serde_json::json!("replay-hash"))
    );
    assert_eq!(
        encoded.get("effective_config_hash"),
        Some(&serde_json::json!("effective-hash"))
    );
    assert!(encoded.get("effective_config").is_none());

    let obsolete = serde_json::json!({
        "source_config": null,
        "overrides": GhosttyVisualOverrides::default(),
        "effective_config": "ghostty-config.txt",
        "effective_config_hash": "obsolete-overloaded-hash"
    });
    assert!(serde_json::from_value::<GhosttyVisualProvenance>(obsolete).is_err());

    let misspelled_override = serde_json::json!({
        "source_config": null,
        "overrides": {
            "font_family": null,
            "font_size": null,
            "padding_x": null,
            "padding_y": null,
            "foreground": null,
            "backround": "#000000"
        },
        "replay_profile": GHOSTTY_REPLAY_PROFILE_NAME,
        "replay_profile_hash": "a".repeat(64),
        "effective_config_hash": "b".repeat(64)
    });
    assert!(serde_json::from_value::<GhosttyVisualProvenance>(misspelled_override).is_err());
}

#[test]
fn geometry_provenance_has_no_visual_hash_authority() {
    let provenance = GeometryProvenance {
        source: GeometrySource::GhosttyFit,
        capture_target: Some(CaptureTarget {
            width: 1280,
            height: 720,
        }),
    };
    let encoded = serde_json::to_value(&provenance).expect("geometry provenance JSON");
    assert!(encoded.get("effective_config_hash").is_none());
    assert!(encoded.get("ghostty_config_hash").is_none());

    for obsolete_field in ["ghostty_config_hash", "effective_config_hash"] {
        let mut obsolete = serde_json::json!({
            "source": "ghostty_fit",
            "capture_target": null
        });
        obsolete.as_object_mut().expect("geometry object").insert(
            obsolete_field.to_owned(),
            serde_json::json!("obsolete-hash"),
        );
        assert!(serde_json::from_value::<GeometryProvenance>(obsolete).is_err());
    }
}

#[test]
fn manifest_validation_rejects_noncanonical_ghostty_profile_contracts() {
    let (temporary, _) = completed_fixture(Vec::new());
    let session = temporary.path().join("session");
    let mut manifest = read_manifest(&session).expect("session manifest");
    manifest.ghostty_visual = Some(GhosttyVisualProvenance {
        source_config: None,
        overrides: GhosttyVisualOverrides::default(),
        replay_profile: GHOSTTY_REPLAY_PROFILE_NAME.to_owned(),
        replay_profile_hash: "a".repeat(64),
        effective_config_hash: "0".repeat(64),
    });
    validate_manifest(&manifest).expect("canonical Ghostty profile contract");

    let mut fitted = manifest.clone();
    fitted.geometry_provenance = GeometryProvenance {
        source: GeometrySource::GhosttyFit,
        capture_target: Some(CaptureTarget {
            width: 1280,
            height: 720,
        }),
    };
    validate_manifest(&fitted).expect("complete fitted-geometry provenance");

    let mut missing_target = fitted.clone();
    missing_target.geometry_provenance.capture_target = None;
    assert!(matches!(
        validate_manifest(&missing_target),
        Err(Error::InvalidReplay { reason }) if reason.contains("capture target")
    ));

    let mut missing_profile = fitted;
    missing_profile.ghostty_visual = None;
    assert!(matches!(
        validate_manifest(&missing_profile),
        Err(Error::InvalidReplay { reason }) if reason.contains("profile provenance")
    ));

    let mut invalid_path = manifest.clone();
    invalid_path
        .ghostty_visual
        .as_mut()
        .expect("Ghostty provenance")
        .replay_profile = "nested/ghostty-profile.ghostty".to_owned();
    assert!(matches!(
        validate_manifest(&invalid_path),
        Err(Error::InvalidReplay { reason })
            if reason.contains("canonical root artifact")
    ));

    let mut invalid_replay_hash = manifest.clone();
    invalid_replay_hash
        .ghostty_visual
        .as_mut()
        .expect("Ghostty provenance")
        .replay_profile_hash = "A".repeat(64);
    assert!(matches!(
        validate_manifest(&invalid_replay_hash),
        Err(Error::InvalidReplay { reason })
            if reason.contains("replay profile hash")
    ));

    manifest
        .ghostty_visual
        .as_mut()
        .expect("Ghostty provenance")
        .effective_config_hash = "f".repeat(63);
    assert!(matches!(
        validate_manifest(&manifest),
        Err(Error::InvalidReplay { reason })
            if reason.contains("effective configuration hash")
    ));
}

#[test]
fn completed_content_validation_binds_the_replay_profile_file() {
    let (temporary, _) = completed_fixture(Vec::new());
    let session = temporary.path().join("session");
    let profile_path = session.join(GHOSTTY_REPLAY_PROFILE_NAME);
    let profile_bytes = b"font-size = 13\n";
    fs_err::write(&profile_path, profile_bytes).expect("replay profile");
    let mut manifest = read_manifest(&session).expect("session manifest");
    manifest.ghostty_visual = Some(GhosttyVisualProvenance {
        source_config: None,
        overrides: GhosttyVisualOverrides::default(),
        replay_profile: GHOSTTY_REPLAY_PROFILE_NAME.to_owned(),
        replay_profile_hash: blake3::hash(profile_bytes).to_hex().to_string(),
        effective_config_hash: "0".repeat(64),
    });

    validate_replay_profile_file(&session, &manifest).expect("matching replay profile");
    fs_err::write(&profile_path, b"font-size = 14\n").expect("tampered replay profile");
    let error =
        validate_replay_profile_file(&session, &manifest).expect_err("profile hash must bind");
    assert!(matches!(
        error,
        Error::InvalidReplay { reason } if reason.contains("replay profile")
    ));
}

#[test]
fn completed_validation_requires_terminal_exit() {
    let (temporary, log) = completed_fixture(vec![Event::Marker {
        operation_id: Uuid::now_v7(),
        label: "not-an-exit".to_owned(),
    }]);
    let error = validate_completed_log(&temporary.path().join("session"), &log)
        .expect_err("nonterminal completion must fail");
    assert!(matches!(
        error,
        Error::InvalidReplay { reason } if reason.contains("does not end with an Exit")
    ));
}

#[test]
fn completed_validation_rejects_pending_mutation() {
    let operation_id = Uuid::now_v7();
    let (temporary, log) = completed_fixture(vec![
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
    let error = validate_completed_log(&temporary.path().join("session"), &log)
        .expect_err("pending mutation must fail");
    assert!(matches!(
        error,
        Error::InvalidReplay { reason } if reason.contains("pending mutation")
    ));
}
