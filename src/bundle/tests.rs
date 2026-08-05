use tempfile::TempDir;

use super::*;

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
    assert!(direct_error.to_string().contains("byte limit"));

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
fn forensic_payload_must_match_the_complete_trusted_top_level_inventory() {
    let temporary = TempDir::new().expect("temporary directory");
    let source = temporary.path().join("source");
    let bundle = temporary.path().join("bundle");
    let canonical = bundle.join("canonical");
    fs_err::create_dir(&source).expect("source directory");
    fs_err::create_dir(&bundle).expect("bundle directory");
    fs_err::create_dir(&canonical).expect("canonical directory");
    for (name, bytes) in [
        ("session.json", b"session".as_slice()),
        ("events.jsonl", b"events".as_slice()),
    ] {
        fs_err::write(source.join(name), bytes).expect("source member");
        fs_err::write(canonical.join(name), bytes).expect("included member");
    }
    let members = ["canonical/events.jsonl", "canonical/session.json"]
        .into_iter()
        .map(|path| {
            return ArtifactMember {
                path: path.to_owned(),
                byte_length: 0,
                blake3: String::new(),
                media_type: String::new(),
                schema_type: None,
                sensitivity: ArtifactSensitivity::SensitiveTerminalData,
                relationship: "canonical_evidence".to_owned(),
            };
        })
        .collect::<Vec<_>>();
    verify_canonical_payload_matches_source(&bundle, &members, &source)
        .expect("exact canonical inventory");

    fs_err::write(canonical.join("events.jsonl"), b"changed").expect("tamper member");
    let content_error = verify_canonical_payload_matches_source(&bundle, &members, &source)
        .expect_err("canonical content must match");
    assert!(content_error.to_string().contains("does not match"));

    fs_err::write(canonical.join("events.jsonl"), b"events").expect("restore member");
    let mut injected_members = members;
    injected_members.push(ArtifactMember {
        path: "canonical/injected.txt".to_owned(),
        byte_length: 0,
        blake3: String::new(),
        media_type: String::new(),
        schema_type: None,
        sensitivity: ArtifactSensitivity::SensitiveTerminalData,
        relationship: "canonical_evidence".to_owned(),
    });
    let inventory_error =
        verify_canonical_payload_matches_source(&bundle, &injected_members, &source)
            .expect_err("attacker-added canonical member must be rejected");
    assert!(inventory_error.to_string().contains("source inventory"));
}
