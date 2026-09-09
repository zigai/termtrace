use super::*;
use super::{tar::extract_archive, verify::read_bounded_regular_file};
use std::{io, path::Path};
use tar_archive::{Builder, EntryType, Header};
use tempfile::TempDir;
use zstd::stream::encode_all;

#[test]
fn generation_budget_is_shared_by_members_and_manifest() {
    use std::io::Write as _;

    let temporary = TempDir::new().expect("temporary directory");
    let destination = temporary.path().join("bounded");
    let identity = BundleIdentity {
        profile: BundleProfile::Screen,
        session_id: Uuid::nil(),
        canonical_session: ByteString::from(b"/trusted/session".as_slice()),
        source_final_event_hash: "a".repeat(64),
        includes_canonical_evidence: false,
    };

    let metadata = BundleMemberMetadata {
        media_type: "text/plain".to_owned(),
        schema_type: None,
        sensitivity: ArtifactSensitivity::SensitiveTerminalData,
        relationship: "selected_screen".to_owned(),
    };

    let mut draft = BundleDraft::create(&destination, identity, Some(5)).expect("draft");
    draft
        .add_member_bytes("first.txt", metadata.clone(), b"abc")
        .expect("first member");
    let staging = draft.staging_path().to_path_buf();

    {
        let mut writer = draft
            .start_member("second.txt", metadata)
            .expect("second member");
        assert!(writer.write_all(b"def").is_err());

        writer.write_all(b"de").expect("remaining shared budget");
        writer.finish().expect("finished second member");
    }

    assert_eq!(
        fs_err::metadata(staging.join("first.txt"))
            .expect("first metadata")
            .len(),
        3
    );
    assert_eq!(
        fs_err::metadata(staging.join("second.txt"))
            .expect("second metadata")
            .len(),
        2
    );
    assert!(
        draft.commit().is_err(),
        "manifest must consume the same generation budget"
    );
    assert!(!destination.exists());
    assert!(!staging.exists());
}

#[test]
fn direct_and_archived_manifests_share_the_byte_limit() {
    let temporary = TempDir::new().expect("temporary directory");
    let direct = temporary.path().join("manifest.json");
    fs_err::write(
        &direct,
        vec![b' '; usize::try_from(MAX_BUNDLE_MANIFEST_BYTES + 1).expect("test size")],
    )
    .expect("oversized direct manifest");

    let direct_error = read_bounded_regular_file(&direct, MAX_BUNDLE_MANIFEST_BYTES)
        .expect_err("direct manifest must be bounded");
    assert!(direct_error.to_string().contains("exceeds"));

    let archive = temporary.path().join("oversized.tar.zst");
    let file = fs_err::File::create(&archive).expect("archive file");
    let encoder = zstd::stream::write::Encoder::new(file, 1).expect("zstd encoder");
    let mut builder = Builder::new(encoder);
    let mut header = Header::new_ustar();
    header.set_path("manifest.json").expect("manifest tar path");
    header.set_mode(0o600);
    header.set_uid(0);
    header.set_gid(0);
    header.set_size(MAX_BUNDLE_MANIFEST_BYTES + 1);
    header.set_mtime(0);
    header.set_entry_type(EntryType::Regular);
    header.set_cksum();

    builder
        .append(&header, io::empty())
        .expect("archive member");
    builder.finish().expect("finish tar archive");
    let encoder = builder.into_inner().expect("finish tar builder");
    encoder.finish().expect("finish compressed archive");

    let destination = temporary.path().join("extracted");
    create_private_directory(&destination).expect("extraction directory");
    let archive_error = extract_archive(&archive, &destination)
        .expect_err("archive manifest must be bounded before extraction");
    assert!(archive_error.to_string().contains("manifest exceeds"));
    assert!(!destination.join("manifest.json").exists());
}

#[test]
fn raw_tar_iteration_rejects_extension_headers_before_their_payload() {
    let temporary = TempDir::new().expect("temporary directory");
    let extension_types = [
        EntryType::GNULongName,
        EntryType::GNULongLink,
        EntryType::GNUSparse,
        EntryType::XHeader,
        EntryType::XGlobalHeader,
    ];

    for (index, entry_type) in extension_types.into_iter().enumerate() {
        let mut builder = Builder::new(Vec::new());
        let mut header = Header::new_gnu();
        header.set_path("extension").expect("extension tar path");
        header.set_mode(0o600);
        header.set_uid(0);
        header.set_gid(0);
        header.set_size(1);
        header.set_mtime(0);
        header.set_entry_type(entry_type);
        header.set_cksum();

        builder
            .append(&header, b"xy".as_slice())
            .expect("extension member");
        builder.finish().expect("finish tar archive");
        let tar_bytes = builder.into_inner().expect("finish tar builder");
        let archive = temporary.path().join(format!("extension-{index}.tar.zst"));
        write_compressed_tar(&archive, &tar_bytes);
        let destination = temporary.path().join(format!("extension-{index}"));
        create_private_directory(&destination).expect("extraction directory");
        let error = extract_archive(&archive, &destination)
            .expect_err("extension member must be rejected before its payload");
        assert!(error.to_string().contains("non-regular member"));
    }
}

#[test]
fn archive_rejects_nonzero_member_padding() {
    let temporary = TempDir::new().expect("temporary directory");
    let mut builder = Builder::new(Vec::new());
    let mut header = Header::new_ustar();
    header.set_path("member").expect("member tar path");
    header.set_mode(0o600);
    header.set_uid(0);
    header.set_gid(0);
    header.set_size(1);
    header.set_mtime(0);
    header.set_entry_type(EntryType::Regular);
    header.set_cksum();

    builder
        .append(&header, b"xy".as_slice())
        .expect("archive member with nonzero padding");
    builder.finish().expect("finish tar archive");
    let tar_bytes = builder.into_inner().expect("finish tar builder");
    let archive = temporary.path().join("nonzero-padding.tar.zst");
    write_compressed_tar(&archive, &tar_bytes);

    let destination = temporary.path().join("nonzero-padding");
    create_private_directory(&destination).expect("extraction directory");
    let error = extract_archive(&archive, &destination)
        .expect_err("nonzero member padding must be rejected");
    assert!(error.to_string().contains("nonzero padding"));
}

fn write_compressed_tar(path: &Path, tar_bytes: &[u8]) {
    let compressed = encode_all(tar_bytes, 1).expect("compress public tar builder output");
    fs_err::write(path, compressed).expect("write compressed tar archive");
}

#[test]
fn identity_mismatch_rejects_before_member_validation() {
    let temporary = TempDir::new().expect("temporary directory");
    let expected = BundleIdentity {
        profile: BundleProfile::Screen,
        session_id: Uuid::nil(),
        canonical_session: ByteString::from(b"/trusted/session".as_slice()),
        source_final_event_hash: "a".repeat(64),
        includes_canonical_evidence: false,
    };

    let invalid_member = ArtifactMember {
        path: "../attacker-controlled".to_owned(),
        byte_length: u64::MAX,
        blake3: "invalid".to_owned(),
        media_type: "application/octet-stream".to_owned(),
        schema_type: None,
        sensitivity: ArtifactSensitivity::OperationalMetadata,
        relationship: "attacker_controlled".to_owned(),
    };
    let mut policy_mismatch = expected.manifest(vec![invalid_member.clone()]);
    policy_mismatch.profile = BundleProfile::Debug;
    fs_err::write(
        temporary.path().join(BUNDLE_MANIFEST_NAME),
        serde_json::to_vec(&policy_mismatch).expect("policy mismatch manifest"),
    )
    .expect("write policy mismatch manifest");
    let policy_error = verify(temporary.path(), Some(&expected))
        .expect_err("profile mismatch must reject before invalid member metadata");
    assert!(policy_error
        .to_string()
        .contains("caller-owned source identity"));

    let mut canonical_mismatch = expected.manifest(vec![invalid_member]);
    canonical_mismatch.session_id = Uuid::from_u128(1);
    fs_err::write(
        temporary.path().join(BUNDLE_MANIFEST_NAME),
        serde_json::to_vec(&canonical_mismatch).expect("canonical mismatch manifest"),
    )
    .expect("write canonical mismatch manifest");
    let canonical_error = verify(temporary.path(), Some(&expected))
        .expect_err("canonical mismatch must reject before invalid member metadata");
    assert!(canonical_error
        .to_string()
        .contains("canonical source identity"));
}
