use super::secure_fs::{
    open_file_at, open_parent_directory, write_secure_with_failure, SecureWriteStage,
};
use super::*;
use crate::event_log::{CanonicalBoundary, EventLogWriter, SessionClock};
use nix::{fcntl::OFlag, sys::stat::Mode};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::ErrorKind,
    os::unix::fs::{symlink, PermissionsExt as _},
};
use tempfile::TempDir;
use termtrace_model::execution::event::{Event, EventLogHeader};
use termtrace_model::execution::layout::{ARTIFACT_ROOT_LOCK, ARTIFACT_ROOT_LOG};
use termtrace_model::value::ByteString;
use uuid::Uuid;

#[test]
fn failed_publication_preserves_existing_presentation() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let destination = temporary.path().join("presentation");
    fs_err::create_dir(&destination).expect("existing presentation");
    let previous = destination.join("presentation.mp4");
    fs_err::write(&previous, b"previous complete video").expect("previous output");
    let staging = temporary.path().join("missing-staging");

    let error = commit_directory(temporary.path(), &staging, &destination, true)
        .expect_err("missing staged output must fail publication");

    assert!(matches!(error, Error::Filesystem { source, .. }
        if source.kind() == ErrorKind::NotFound));
    assert_eq!(
        fs_err::read(previous).expect("previous output remains"),
        b"previous complete video"
    );
    assert!(!staging.exists());
}

#[test]
fn publication_replaces_an_existing_presentation_with_complete_output() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let destination = temporary.path().join("presentation");
    let staging = temporary.path().join("staging");
    fs_err::create_dir(&destination).expect("previous directory");
    fs_err::create_dir(&staging).expect("staged directory");
    fs_err::write(destination.join("old.mp4"), b"old").expect("previous output");
    fs_err::write(staging.join("presentation.mp4"), b"new complete video").expect("staged output");

    commit_directory(temporary.path(), &staging, &destination, true).expect("replace presentation");

    assert_eq!(
        fs_err::read(destination.join("presentation.mp4")).expect("published video"),
        b"new complete video"
    );
    assert!(!destination.join("old.mp4").exists());
}

#[test]
fn prefix_catalog_authenticates_old_source_after_growth_and_rejects_wrong_byte_count() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("session");
    create_private_directory(&session).expect("private session");

    let clock = SessionClock::now().expect("clock");
    let manifest_hash = blake3::hash(b"test manifest").to_hex().to_string();
    let header = EventLogHeader::new(
        Uuid::now_v7(),
        &manifest_hash,
        clock.started_unix_ns(),
        80,
        24,
        640,
        480,
        "xterm-ghostty",
    );

    let mut writer =
        EventLogWriter::create(session.join("events.jsonl"), &header, clock).expect("writer");
    writer
        .append(Event::Output {
            data: ByteString::from(b"OLD".as_slice()),
        })
        .expect("prefix output");
    let prefix = CanonicalBoundary {
        next_event_seq: writer.next_seq(),
        event_hash: writer.current_hash(),
        output_bytes: 3,
    };
    writer
        .append(Event::Output {
            data: ByteString::from(b"NEW".as_slice()),
        })
        .expect("later output");
    let final_hash = writer.current_hash();

    for (index, count) in [3_u64, 4].into_iter().enumerate() {
        let artifact = session.join(format!("artifact-{index}"));
        create_private_directory(&artifact).expect("artifact directory");
        write_secure(&artifact.join("image.txt"), b"pixels").expect("payload");
        let manifest = ArtifactMemberEnvelope {
            members: collect_artifact_members(&artifact).expect("members"),
            source_event_hash: Some(prefix.event_hash.clone()),
            source_range: None,
        };
        let bytes = serde_json::to_vec(&manifest).expect("manifest JSON");
        write_secure(&artifact.join("manifest.json"), &bytes).expect("manifest");

        let mut commitment = prefix.clone();
        commitment.output_bytes = count;
        append_artifact_prefix_root(
            &session,
            &artifact,
            "prefix-test",
            &bytes,
            &manifest_hash,
            &commitment,
        )
        .expect("prefix root");

        let verified = verify_session_artifacts(&session, &manifest_hash, Some(&final_hash));

        if index == 0 {
            assert_eq!(verified.expect("old prefix verifies"), 1);
        } else {
            assert!(verified.is_err(), "wrong prefix byte count was accepted");
        }
    }
}

#[test]
fn generation_budget_rejects_before_writing_excess_bytes() {
    let mut bytes = Vec::new();

    {
        let mut writer = BudgetWriter::new(&mut bytes, 5);
        writer.write_all(b"abc").expect("first bounded write");
        assert!(writer.write_all(b"def").is_err());
        writer.write_all(b"de").expect("exact remaining allowance");
        assert!(writer.write_all(b"f").is_err());
    }

    assert_eq!(bytes, b"abcde");
}

#[test]
fn member_verification_rejects_extra_and_changed_payloads() {
    let temporary = TempDir::new().expect("temporary directory");
    let artifact = temporary.path().join("artifact");
    fs_err::create_dir(&artifact).expect("artifact directory");
    fs_err::write(artifact.join("final.txt"), b"original").expect("artifact member");
    let members = collect_artifact_members(&artifact).expect("collect members");
    verify_artifact_members(&artifact, &members).expect("members should verify");

    fs_err::write(artifact.join("extra.log"), b"unmanifested").expect("extra member");

    assert!(verify_artifact_members(&artifact, &members).is_err());

    fs_err::remove_file(artifact.join("extra.log")).expect("remove extra member");
    fs_err::write(artifact.join("final.txt"), b"changed").expect("change member");

    assert!(verify_artifact_members(&artifact, &members).is_err());
}

#[test]
fn parallel_member_hashing_preserves_canonical_path_order() {
    let temporary = TempDir::new().expect("temporary directory");
    let artifact = temporary.path().join("artifact");
    fs_err::create_dir(&artifact).expect("artifact directory");

    for name in ["z.txt", "a.txt", "m.txt", "b.txt", "y.txt", "c.txt"] {
        fs_err::write(artifact.join(name), name.as_bytes()).expect("artifact member");
    }

    let members = collect_artifact_members(&artifact).expect("collect members");
    assert_eq!(
        members
            .iter()
            .map(|member| return member.path.as_str())
            .collect::<Vec<_>>(),
        vec!["a.txt", "b.txt", "c.txt", "m.txt", "y.txt", "z.txt"]
    );
    verify_artifact_members(&artifact, &members).expect("ordered members should verify");
}

#[test]
fn session_artifact_verification_binds_manifest_source_event_hash() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("session");
    let artifact = session.join("derived");
    create_private_directory(&artifact).expect("artifact directory");
    write_secure(&artifact.join("final.txt"), b"state").expect("artifact member");
    let members = collect_artifact_members(&artifact).expect("collect members");
    let manifest = ArtifactMemberEnvelope {
        members,
        source_event_hash: Some("wrong-source".to_owned()),
        source_range: None,
    };
    let manifest_bytes = serde_json::to_vec(&manifest).expect("manifest JSON");
    write_secure(&artifact.join("manifest.json"), &manifest_bytes).expect("artifact manifest");
    append_artifact_root(
        &session,
        &artifact,
        "semantic_index",
        &manifest_bytes,
        "session-manifest",
        Some("canonical-source"),
        None,
    )
    .expect("artifact root");

    let error = verify_session_artifacts(&session, "session-manifest", Some("canonical-source"))
        .expect_err("manifest source event hash must bind its catalog");
    assert!(matches!(error, Error::InvalidEvidence { .. }));
}

#[test]
fn member_collection_rejects_symlinks() {
    let temporary = TempDir::new().expect("temporary directory");
    let artifact = temporary.path().join("artifact");
    fs_err::create_dir(&artifact).expect("artifact directory");

    fs_err::write(temporary.path().join("outside"), b"secret").expect("outside file");
    symlink(temporary.path().join("outside"), artifact.join("linked")).expect("payload symlink");

    assert!(collect_artifact_members(&artifact).is_err());
}

#[test]
fn secure_outputs_create_missing_owner_only_parent_directories() {
    let temporary = TempDir::new().expect("temporary directory");
    let nested = temporary.path().join("missing/private/artifact");
    create_private_directory(&nested).expect("nested artifact directory");
    write_secure(&nested.join("report.json"), b"{}\n").expect("nested artifact file");

    for path in [
        temporary.path().join("missing"),
        temporary.path().join("missing/private"),
        nested,
    ] {
        let mode = fs_err::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(mode, 0o700);
    }
}

#[test]
fn secure_outputs_do_not_follow_a_missing_parent_symlink() {
    let temporary = TempDir::new().expect("temporary directory");
    let outside = temporary.path().join("outside");
    fs_err::create_dir(&outside).expect("outside directory");

    let link = temporary.path().join("linked-parent");
    symlink(&outside, &link).expect("parent symlink");

    assert!(write_secure(&link.join("nested/report.json"), b"{}\n").is_err());
    assert!(!outside.join("nested").exists());
}

#[test]
fn secure_outputs_reject_symlinks_before_existing_parent_components() {
    let temporary = TempDir::new().expect("temporary directory");
    let outside = temporary.path().join("outside");
    fs_err::create_dir(&outside).expect("outside directory");
    fs_err::create_dir(outside.join("existing")).expect("existing outside descendant");

    let link = temporary.path().join("linked-parent");
    symlink(&outside, &link).expect("parent symlink");

    assert!(write_secure(&link.join("existing/nested/report.json"), b"{}\n").is_err());
    assert!(!outside.join("existing/nested").exists());
}

#[test]
fn retained_parent_descriptor_prevents_create_redirection() {
    let temporary = TempDir::new().expect("temporary directory");
    let parent = temporary.path().join("parent");
    let moved = temporary.path().join("moved-parent");
    let outside = temporary.path().join("outside");

    create_private_directory(&parent).expect("private parent");

    create_private_directory(&outside).expect("outside directory");

    let target = parent.join("artifact.json");
    let parent_fd = open_parent_directory(&target, false).expect("retain parent descriptor");

    fs_err::rename(&parent, &moved).expect("replace pathname after validation");
    symlink(&outside, &parent).expect("redirect original pathname");

    let fd = open_file_at(
        &parent_fd,
        OsStr::new("artifact.json"),
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW,
        Mode::from_bits_truncate(0o600),
        &target,
    )
    .expect("descriptor-relative create");

    File::from(fd).write_all(b"safe").expect("write artifact");

    assert_eq!(
        fs_err::read(moved.join("artifact.json")).expect("bound output"),
        b"safe"
    );
    assert!(!outside.join("artifact.json").exists());
}

#[test]
fn secure_write_is_never_partially_published_at_injected_failure_boundaries() {
    let temporary = TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("atomic-writes");
    create_private_directory(&directory).expect("private output directory");

    let payload = b"complete authoritative payload";

    for stage in [SecureWriteStage::Created, SecureWriteStage::DataSynced] {
        let destination = directory.join(format!("before-rename-{stage:?}.json"));
        write_secure_with_failure(&destination, payload, stage)
            .expect_err("injected pre-rename failure");

        assert!(
            !destination.exists(),
            "pre-rename failure exposed a destination at {stage:?}"
        );
    }

    let destination = directory.join("after-rename.json");
    write_secure_with_failure(&destination, payload, SecureWriteStage::Renamed)
        .expect_err("injected directory-sync failure");

    assert_eq!(
        fs_err::read(&destination).expect("fully renamed payload"),
        payload
    );

    let entries: Vec<_> = fs_err::read_dir(&directory)
        .expect("atomic write directory")
        .map(|entry| return entry.expect("atomic write entry").file_name())
        .collect();
    assert_eq!(entries, [OsString::from("after-rename.json")]);
}

#[test]
fn artifact_catalog_rejects_session_lock_and_catalog_symlinks() {
    let temporary = TempDir::new().expect("temporary directory");
    let outside = temporary.path().join("outside");
    create_private_directory(&outside).expect("outside directory");

    let real_session = temporary.path().join("real-session");
    create_private_directory(&real_session).expect("real session");

    let linked_session = temporary.path().join("linked-session");
    symlink(&real_session, &linked_session).expect("session symlink");
    let error = append_artifact_root(
        &linked_session,
        &linked_session.join("derived"),
        "semantic_index",
        b"{}",
        &"0".repeat(64),
        None,
        None,
    )
    .expect_err("session symlink must be rejected");
    assert!(matches!(error, Error::Filesystem { .. }));

    let lock_session = temporary.path().join("lock-session");
    create_private_directory(&lock_session).expect("lock session");

    let outside_lock = outside.join("lock");
    fs_err::write(&outside_lock, b"sentinel").expect("outside lock");
    symlink(&outside_lock, lock_session.join(ARTIFACT_ROOT_LOCK)).expect("lock symlink");

    assert!(append_artifact_root(
        &lock_session,
        &lock_session.join("derived"),
        "semantic_index",
        b"{}",
        &"0".repeat(64),
        None,
        None,
    )
    .is_err());
    assert_eq!(
        fs_err::read(&outside_lock).expect("unchanged lock"),
        b"sentinel"
    );

    let catalog_session = temporary.path().join("catalog-session");
    create_private_directory(&catalog_session).expect("catalog session");

    let outside_catalog = outside.join("catalog");
    fs_err::write(&outside_catalog, b"sentinel").expect("outside catalog");
    symlink(&outside_catalog, catalog_session.join(ARTIFACT_ROOT_LOG)).expect("catalog symlink");

    assert!(append_artifact_root(
        &catalog_session,
        &catalog_session.join("derived"),
        "semantic_index",
        b"{}",
        &"0".repeat(64),
        None,
        None,
    )
    .is_err());
    assert_eq!(
        fs_err::read(&outside_catalog).expect("unchanged catalog"),
        b"sentinel"
    );
}
