use std::{
    io::Write as _,
    os::unix::fs::{PermissionsExt as _, symlink},
};

use tempfile::TempDir;

use super::*;

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
    let manifest_bytes = serde_json::to_vec(&serde_json::json!({
        "members": members,
        "source_event_hash": "wrong-source",
        "source_range": null
    }))
    .expect("manifest JSON");
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
    assert!(error.to_string().contains("source event hash"));
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
    assert!(
        append_artifact_root(
            &lock_session,
            &lock_session.join("derived"),
            "semantic_index",
            b"{}",
            &"0".repeat(64),
            None,
            None,
        )
        .is_err()
    );
    assert_eq!(
        fs_err::read(&outside_lock).expect("unchanged lock"),
        b"sentinel"
    );

    let catalog_session = temporary.path().join("catalog-session");
    create_private_directory(&catalog_session).expect("catalog session");
    let outside_catalog = outside.join("catalog");
    fs_err::write(&outside_catalog, b"sentinel").expect("outside catalog");
    symlink(&outside_catalog, catalog_session.join(ARTIFACT_ROOT_LOG)).expect("catalog symlink");
    assert!(
        append_artifact_root(
            &catalog_session,
            &catalog_session.join("derived"),
            "semantic_index",
            b"{}",
            &"0".repeat(64),
            None,
            None,
        )
        .is_err()
    );
    assert_eq!(
        fs_err::read(&outside_catalog).expect("unchanged catalog"),
        b"sentinel"
    );
}
