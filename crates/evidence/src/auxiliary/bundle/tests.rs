use std::io::Write as _;

use tempfile::TempDir;

use super::*;
use super::{
    tar::{extract_archive, write_checksum, write_octal, write_tar_path},
    verify::read_bounded_regular_file,
};

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
    let mut encoder = zstd::stream::write::Encoder::new(file, 1).expect("zstd encoder");
    let mut header = [0_u8; 512];
    write_tar_path(&mut header, "manifest.json").expect("manifest tar path");
    write_octal(&mut header, 100, 8, 0o600).expect("tar mode");
    write_octal(&mut header, 108, 8, 0).expect("tar uid");
    write_octal(&mut header, 116, 8, 0).expect("tar gid");
    write_octal(&mut header, 124, 12, MAX_BUNDLE_MANIFEST_BYTES + 1).expect("tar size");
    write_octal(&mut header, 136, 12, 0).expect("tar time");
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum = header.iter().map(|byte| return u64::from(*byte)).sum();
    write_checksum(&mut header, checksum).expect("tar checksum");
    encoder.write_all(&header).expect("archive header");
    encoder.finish().expect("finish archive");

    let destination = temporary.path().join("extracted");
    create_private_directory(&destination).expect("extraction directory");
    let archive_error = extract_archive(&archive, &destination)
        .expect_err("archive manifest must be bounded before extraction");
    assert!(archive_error.to_string().contains("manifest exceeds"));
    assert!(!destination.join("manifest.json").exists());
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
    assert!(
        policy_error
            .to_string()
            .contains("caller-owned source identity")
    );

    let mut canonical_mismatch = expected.manifest(vec![invalid_member]);
    canonical_mismatch.session_id = Uuid::from_u128(1);
    fs_err::write(
        temporary.path().join(BUNDLE_MANIFEST_NAME),
        serde_json::to_vec(&canonical_mismatch).expect("canonical mismatch manifest"),
    )
    .expect("write canonical mismatch manifest");
    let canonical_error = verify(temporary.path(), Some(&expected))
        .expect_err("canonical mismatch must reject before invalid member metadata");
    assert!(
        canonical_error
            .to_string()
            .contains("canonical source identity")
    );
}
