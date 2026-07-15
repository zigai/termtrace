//! Deterministic, sensitivity-labeled evidence handoff directories.

use std::{
    collections::BTreeSet,
    fs::FileType,
    io::{self, Read as _, Write as _},
    os::unix::fs::FileTypeExt as _,
    path::{Component, Path, PathBuf},
    str,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{
        ArtifactMember, ArtifactSensitivity, collect_artifact_members, commit_directory,
        create_private_directory, finish_writer, secure_file, secure_writer, sync_directory,
        write_secure,
    },
    event::{ByteString, EventProjection},
    event_log::EventLog,
    observation::{StatePoint, observe_session},
    session::{VerificationLevel, read_manifest, validate_completed_log, verify_session},
};

/// Fixed bundle content policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BundleProfile {
    /// Latest screen observation without canonical raw events.
    Screen,
    /// Screen plus metadata-only canonical event projections and verification.
    Debug,
    /// Complete top-level canonical session payload plus verification.
    Forensic,
}

/// One planned member and its sensitivity before publication.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BundlePlanMember {
    /// Relative output path.
    pub path: String,
    /// Explicit sensitivity class.
    pub sensitivity: ArtifactSensitivity,
    /// Why the member is selected.
    pub relationship: String,
}

/// Dry-run selection inventory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BundlePlan {
    /// Stable plan format.
    pub format: String,
    /// Plan schema version.
    pub version: u32,
    /// Selected policy.
    pub profile: BundleProfile,
    /// Whether canonical raw evidence is included.
    pub includes_canonical_evidence: bool,
    /// Planned files.
    pub members: Vec<BundlePlanMember>,
}

/// Top-level whole-file bundle manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BundleManifest {
    /// Stable manifest format.
    pub format: String,
    /// Manifest schema version.
    pub version: u32,
    /// Selected policy.
    pub profile: BundleProfile,
    /// Immutable canonical session identity.
    pub session_id: Uuid,
    /// Exact source session path bytes.
    pub canonical_session: ByteString,
    /// Final canonical event hash.
    pub source_final_event_hash: String,
    /// Whether canonical raw evidence is included.
    pub includes_canonical_evidence: bool,
    /// Whole-file inventory; unmanifested payloads are invalid.
    pub members: Vec<ArtifactMember>,
}

/// Return the deterministic content and sensitivity policy without writing.
#[must_use]
pub fn plan(profile: BundleProfile) -> BundlePlan {
    let mut members = vec![
        planned(
            "screen.txt",
            ArtifactSensitivity::SensitiveTerminalData,
            "selected_screen",
        ),
        planned(
            "observation.json",
            ArtifactSensitivity::SensitiveTerminalData,
            "selected_observation",
        ),
    ];
    if matches!(profile, BundleProfile::Debug | BundleProfile::Forensic) {
        members.push(planned(
            "verification.json",
            ArtifactSensitivity::OperationalMetadata,
            "source_verification",
        ));
    }
    if profile == BundleProfile::Debug {
        members.push(planned(
            "events-metadata.jsonl",
            ArtifactSensitivity::OperationalMetadata,
            "metadata_only_event_projection",
        ));
    }
    if profile == BundleProfile::Forensic {
        members.push(planned(
            "canonical/",
            ArtifactSensitivity::SensitiveTerminalData,
            "complete_top_level_canonical_payload",
        ));
    }
    return BundlePlan {
        format: "termtrace-bundle-plan".to_owned(),
        version: 1,
        profile,
        includes_canonical_evidence: profile == BundleProfile::Forensic,
        members,
    };
}

/// Atomically publish one manifested bundle directory without replacement.
pub fn create(
    session: &Path,
    destination: &Path,
    profile: BundleProfile,
) -> Result<BundleManifest> {
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;
    if destination.starts_with(&canonical) {
        return Err(bundle_error(
            "bundle destination must be outside the canonical session",
        ));
    }
    let log = EventLog::read(canonical.join("events.jsonl"))?;
    let completion = validate_completed_log(&canonical, &log)?;
    let source_manifest = read_manifest(&canonical)?;
    let parent = destination
        .parent()
        .ok_or_else(|| return bundle_error("bundle path has no parent"))?;
    let name = destination
        .file_name()
        .ok_or_else(|| return bundle_error("bundle path has no basename"))?;
    if !parent.is_dir() {
        return Err(bundle_error("bundle parent directory does not exist"));
    }
    let staging = parent.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        Uuid::now_v7()
    ));
    create_private_directory(&staging)?;
    let publish = (|| -> Result<BundleManifest> {
        let observation = observe_session(&canonical, StatePoint::LatestPresented)?;
        write_secure(
            &staging.join("screen.txt"),
            observation.frame.viewport_text.as_bytes(),
        )?;
        write_json(&staging.join("observation.json"), &observation)?;
        if matches!(profile, BundleProfile::Debug | BundleProfile::Forensic) {
            write_json(
                &staging.join("verification.json"),
                &verify_session(&canonical, VerificationLevel::Content),
            )?;
        }
        if profile == BundleProfile::Debug {
            let mut bytes = Vec::new();
            for record in &log.events {
                serde_json::to_writer(&mut bytes, &EventProjection::from_record(record, true))
                    .map_err(|source| return json_error("events-metadata.jsonl", source))?;
                bytes.push(b'\n');
            }
            write_secure(&staging.join("events-metadata.jsonl"), &bytes)?;
        }
        if profile == BundleProfile::Forensic {
            let target = staging.join("canonical");
            create_private_directory(&target)?;
            copy_top_level_files(&canonical, &target)?;
        }
        let mut members = collect_artifact_members(&staging)?;
        for member in &mut members {
            if member.path == "verification.json" || member.path == "events-metadata.jsonl" {
                member.sensitivity = ArtifactSensitivity::OperationalMetadata;
            }
            member.relationship = bundle_relationship(&member.path).to_owned();
        }
        let manifest = BundleManifest {
            format: "termtrace-bundle".to_owned(),
            version: 1,
            profile,
            session_id: source_manifest.session_id,
            canonical_session: ByteString::from(canonical.as_os_str().as_encoded_bytes()),
            source_final_event_hash: completion.final_event_hash,
            includes_canonical_evidence: profile == BundleProfile::Forensic,
            members,
        };
        write_json(&staging.join("manifest.json"), &manifest)?;
        sync_directory(&staging)?;
        return Ok(manifest);
    })();
    let manifest = match publish {
        Ok(value) => value,
        Err(error) => {
            let _cleanup = fs_err::remove_dir_all(&staging);
            return Err(error);
        }
    };
    if let Err(error) = commit_directory(parent, &staging, destination) {
        let _cleanup = fs_err::remove_dir_all(&staging);
        return Err(error);
    }
    return Ok(manifest);
}

/// Verify a bundle manifest, every member, and any included canonical session.
pub fn verify(directory: &Path) -> Result<BundleManifest> {
    let path = directory.join("manifest.json");
    let manifest: BundleManifest =
        serde_json::from_slice(&fs_err::read(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?)
        .map_err(|source| return json_error("manifest.json", source))?;
    if manifest.format != "termtrace-bundle" || manifest.version != 1 {
        return Err(bundle_error("unsupported bundle manifest"));
    }
    verify_bundle_members(directory, &manifest.members)?;
    if manifest.includes_canonical_evidence {
        let report = verify_session(directory.join("canonical"), VerificationLevel::Content);
        if !report.valid || report.session_id != Some(manifest.session_id) {
            return Err(bundle_error(
                "included canonical evidence failed validation",
            ));
        }
    }
    return Ok(manifest);
}

/// Publish one deterministic Zstandard-compressed USTAR bundle archive.
pub fn archive(directory: &Path, destination: &Path) -> Result<()> {
    verify(directory)?;
    if destination.exists() {
        return Err(Error::ArtifactExists {
            path: destination.to_path_buf(),
        });
    }
    let parent = destination
        .parent()
        .ok_or_else(|| return bundle_error("archive path has no parent"))?;
    if !parent.is_dir() {
        return Err(bundle_error("archive parent directory does not exist"));
    }
    let staging = parent.join(format!(".bundle-archive-{}.tmp", Uuid::now_v7()));
    let file = secure_file(&staging)?;
    let mut encoder = zstd::stream::write::Encoder::new(file, 9).map_err(|source| {
        return Error::Filesystem {
            path: staging.clone(),
            source,
        };
    })?;
    encoder.include_checksum(true).map_err(|source| {
        return Error::Filesystem {
            path: staging.clone(),
            source,
        };
    })?;
    let mut files = Vec::new();
    collect_bundle_files(directory, directory, &mut files)?;
    files.sort();
    for relative in files {
        write_tar_file(&mut encoder, directory, &relative)?;
    }
    encoder.write_all(&[0_u8; 1024]).map_err(|source| {
        return Error::Filesystem {
            path: staging.clone(),
            source,
        };
    })?;
    let file = encoder.finish().map_err(|source| {
        return Error::Filesystem {
            path: staging.clone(),
            source,
        };
    })?;
    file.sync_all().map_err(|source| {
        return Error::Filesystem {
            path: staging.clone(),
            source,
        };
    })?;
    if let Err(error) = commit_directory(parent, &staging, destination) {
        let _cleanup = fs_err::remove_file(&staging);
        return Err(error);
    }
    return Ok(());
}

/// Verify a deterministic compressed archive after bounded safe extraction.
pub fn verify_archive(archive: &Path) -> Result<BundleManifest> {
    let parent = archive
        .parent()
        .ok_or_else(|| return bundle_error("archive path has no parent"))?;
    let staging = parent.join(format!(".bundle-verify-{}.tmp", Uuid::now_v7()));
    create_private_directory(&staging)?;
    let result = (|| {
        extract_archive(archive, &staging)?;
        return verify(&staging);
    })();
    let _cleanup = fs_err::remove_dir_all(&staging);
    return result;
}

fn collect_bundle_files(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs_err::read_dir(directory).map_err(|source| {
        return Error::Filesystem {
            path: directory.to_path_buf(),
            source,
        };
    })? {
        let entry = entry.map_err(|source| {
            return Error::Filesystem {
                path: directory.to_path_buf(),
                source,
            };
        })?;
        let file_type = entry.file_type().map_err(|source| {
            return Error::Filesystem {
                path: entry.path(),
                source,
            };
        })?;
        if file_type.is_symlink()
            || file_type.is_block_device()
            || file_type.is_char_device()
            || file_type.is_fifo()
            || file_type.is_socket()
        {
            return Err(bundle_error(
                "bundle archive source contains a special file",
            ));
        }
        if file_type.is_dir() {
            collect_bundle_files(root, &entry.path(), files)?;
        } else {
            files.push(
                entry
                    .path()
                    .strip_prefix(root)
                    .map_err(|_error| return bundle_error("bundle path escaped its root"))?
                    .to_path_buf(),
            );
        }
    }
    return Ok(());
}

fn write_tar_file(writer: &mut impl io::Write, root: &Path, relative: &Path) -> Result<()> {
    let path = relative
        .to_str()
        .ok_or_else(|| return bundle_error("bundle archive paths must be UTF-8"))?;
    let source = root.join(relative);
    let size = fs_err::metadata(&source)
        .map_err(|source_error| {
            return Error::Filesystem {
                path: source.clone(),
                source: source_error,
            };
        })?
        .len();
    let mut header = [0_u8; 512];
    write_tar_path(&mut header, path)?;
    write_octal(&mut header, 100, 8, 0o600)?;
    write_octal(&mut header, 108, 8, 0)?;
    write_octal(&mut header, 116, 8, 0)?;
    write_octal(&mut header, 124, 12, size)?;
    write_octal(&mut header, 136, 12, 0)?;
    header[148..156].fill(b' ');
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum = header.iter().map(|byte| return u64::from(*byte)).sum();
    write_checksum(&mut header, checksum)?;
    writer.write_all(&header).map_err(|source_error| {
        return Error::Filesystem {
            path: source.clone(),
            source: source_error,
        };
    })?;
    let mut input = fs_err::File::open(&source).map_err(|source_error| {
        return Error::Filesystem {
            path: source.clone(),
            source: source_error,
        };
    })?;
    io::copy(&mut input, writer).map_err(|source_error| {
        return Error::Filesystem {
            path: source.clone(),
            source: source_error,
        };
    })?;
    let padding = (512 - size % 512) % 512;
    writer
        .write_all(&vec![0_u8; usize::try_from(padding).unwrap_or(511)])
        .map_err(|source_error| {
            return Error::Filesystem {
                path: source,
                source: source_error,
            };
        })?;
    return Ok(());
}

fn write_tar_path(header: &mut [u8; 512], path: &str) -> Result<()> {
    if path.len() <= 100 {
        header
            .get_mut(..path.len())
            .ok_or_else(|| return bundle_error("tar path header overflowed"))?
            .copy_from_slice(path.as_bytes());
        return Ok(());
    }
    let split = path
        .char_indices()
        .filter(|(_, character)| return *character == '/')
        .map(|(index, _)| return index)
        .rev()
        .find(|index| return *index <= 155 && path.len().saturating_sub(*index + 1) <= 100)
        .ok_or_else(|| return bundle_error("bundle path exceeds USTAR limits"))?;
    let (prefix, name) = path.split_at(split);
    let name = name.trim_start_matches('/');
    header
        .get_mut(..name.len())
        .ok_or_else(|| return bundle_error("tar name header overflowed"))?
        .copy_from_slice(name.as_bytes());
    header
        .get_mut(345..345 + prefix.len())
        .ok_or_else(|| return bundle_error("tar prefix header overflowed"))?
        .copy_from_slice(prefix.as_bytes());
    return Ok(());
}

fn write_octal(header: &mut [u8; 512], offset: usize, width: usize, value: u64) -> Result<()> {
    let encoded = format!("{:0width$o}\0", value, width = width.saturating_sub(1));
    let target = header
        .get_mut(offset..offset.saturating_add(width))
        .ok_or_else(|| return bundle_error("tar numeric header overflowed"))?;
    if encoded.len() != width {
        return Err(bundle_error("tar numeric value exceeds field width"));
    }
    target.copy_from_slice(encoded.as_bytes());
    return Ok(());
}

fn write_checksum(header: &mut [u8; 512], checksum: u64) -> Result<()> {
    let encoded = format!("{checksum:06o}\0 ");
    if encoded.len() != 8 {
        return Err(bundle_error("tar checksum exceeds field width"));
    }
    header[148..156].copy_from_slice(encoded.as_bytes());
    return Ok(());
}

fn extract_archive(archive: &Path, destination: &Path) -> Result<()> {
    let file = fs_err::File::open(archive).map_err(|source| {
        return Error::Filesystem {
            path: archive.to_path_buf(),
            source,
        };
    })?;
    let mut decoder = zstd::stream::read::Decoder::new(file).map_err(|source| {
        return Error::Filesystem {
            path: archive.to_path_buf(),
            source,
        };
    })?;
    let mut paths = BTreeSet::new();
    let mut total_size = 0_u64;
    for entry_count in 0..=100_000_u64 {
        let mut header = [0_u8; 512];
        decoder.read_exact(&mut header).map_err(|source| {
            return Error::Filesystem {
                path: archive.to_path_buf(),
                source,
            };
        })?;
        if header.iter().all(|byte| return *byte == 0) {
            let mut terminator = [0_u8; 512];
            decoder.read_exact(&mut terminator).map_err(|source| {
                return Error::Filesystem {
                    path: archive.to_path_buf(),
                    source,
                };
            })?;
            if !terminator.iter().all(|byte| return *byte == 0) {
                return Err(bundle_error("tar archive has an invalid terminator"));
            }
            return Ok(());
        }
        if entry_count == 100_000 {
            return Err(bundle_error("bundle archive exceeds 100000 members"));
        }
        verify_tar_checksum(&header)?;
        let path = tar_path(&header)?;
        validate_archive_path(&path)?;
        if !paths.insert(path.clone()) {
            return Err(bundle_error("bundle archive contains a duplicate path"));
        }
        let type_flag = *header
            .get(156)
            .ok_or_else(|| return bundle_error("tar type header is absent"))?;
        if type_flag != b'0' && type_flag != 0 {
            return Err(bundle_error("bundle archive contains a non-regular member"));
        }
        let size = parse_tar_octal(
            header
                .get(124..136)
                .ok_or_else(|| return bundle_error("tar size header is absent"))?,
        )?;
        total_size = total_size.saturating_add(size);
        if total_size > 8 * 1024 * 1024 * 1024 {
            return Err(bundle_error("bundle archive expands beyond 8 GiB"));
        }
        let target = destination.join(&path);
        let parent = target
            .parent()
            .ok_or_else(|| return bundle_error("archive member has no parent"))?;
        fs_err::create_dir_all(parent).map_err(|source| {
            return Error::Filesystem {
                path: parent.to_path_buf(),
                source,
            };
        })?;
        let mut writer = secure_writer(&target)?;
        let copied = io::copy(&mut decoder.by_ref().take(size), &mut writer).map_err(|source| {
            return Error::Filesystem {
                path: target.clone(),
                source,
            };
        })?;
        if copied != size {
            return Err(bundle_error("bundle archive member is truncated"));
        }
        finish_writer(writer, &target)?;
        let padding = (512 - size % 512) % 512;
        let mut padding_bytes = vec![0_u8; usize::try_from(padding).unwrap_or(511)];
        decoder.read_exact(&mut padding_bytes).map_err(|source| {
            return Error::Filesystem {
                path: archive.to_path_buf(),
                source,
            };
        })?;
        if !padding_bytes.iter().all(|byte| return *byte == 0) {
            return Err(bundle_error("tar member padding is nonzero"));
        }
    }
    return Err(bundle_error("bundle archive has no terminator"));
}

fn tar_path(header: &[u8; 512]) -> Result<PathBuf> {
    let name = tar_text(
        header
            .get(..100)
            .ok_or_else(|| return bundle_error("tar name header is absent"))?,
    )?;
    let prefix = tar_text(
        header
            .get(345..500)
            .ok_or_else(|| return bundle_error("tar prefix header is absent"))?,
    )?;
    if name.is_empty() {
        return Err(bundle_error("tar member name is empty"));
    }
    return Ok(if prefix.is_empty() {
        PathBuf::from(name)
    } else {
        PathBuf::from(prefix).join(name)
    });
}

fn tar_text(field: &[u8]) -> Result<&str> {
    let length = field
        .iter()
        .position(|byte| return *byte == 0)
        .unwrap_or(field.len());
    return str::from_utf8(
        field
            .get(..length)
            .ok_or_else(|| return bundle_error("tar text field overflowed"))?,
    )
    .map_err(|_error| return bundle_error("tar paths must be UTF-8"));
}

fn validate_archive_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| return !matches!(component, Component::Normal(_)))
    {
        return Err(bundle_error("bundle archive member path is unsafe"));
    }
    return Ok(());
}

fn parse_tar_octal(field: &[u8]) -> Result<u64> {
    let value = tar_text(field)?.trim();
    if value.is_empty() {
        return Ok(0);
    }
    return u64::from_str_radix(value, 8)
        .map_err(|_error| return bundle_error("tar numeric field is invalid"));
}

fn verify_tar_checksum(header: &[u8; 512]) -> Result<()> {
    let expected = parse_tar_octal(
        header
            .get(148..156)
            .ok_or_else(|| return bundle_error("tar checksum header is absent"))?,
    )?;
    let actual = header.iter().enumerate().fold(0_u64, |sum, (index, byte)| {
        return sum.saturating_add(if (148..156).contains(&index) {
            u64::from(b' ')
        } else {
            u64::from(*byte)
        });
    });
    if actual != expected {
        return Err(bundle_error("tar header checksum is invalid"));
    }
    return Ok(());
}

fn verify_bundle_members(directory: &Path, expected: &[ArtifactMember]) -> Result<()> {
    let actual = collect_artifact_members(directory)?;
    if actual.len() != expected.len() {
        return Err(bundle_error(
            "bundle payload members do not match their whole-file manifest",
        ));
    }
    for member in expected {
        let Some(found) = actual
            .iter()
            .find(|candidate| return candidate.path == member.path)
        else {
            return Err(bundle_error(
                "bundle payload members do not match their whole-file manifest",
            ));
        };
        if found.byte_length != member.byte_length
            || found.blake3 != member.blake3
            || found.media_type != member.media_type
        {
            return Err(bundle_error(
                "bundle payload members do not match their whole-file manifest",
            ));
        }
    }
    return Ok(());
}

fn copy_top_level_files(source: &Path, destination: &Path) -> Result<()> {
    for entry in fs_err::read_dir(source).map_err(|source_error| {
        return Error::Filesystem {
            path: source.to_path_buf(),
            source: source_error,
        };
    })? {
        let entry = entry.map_err(|source_error| {
            return Error::Filesystem {
                path: source.to_path_buf(),
                source: source_error,
            };
        })?;
        let file_type: FileType = entry.file_type().map_err(|source_error| {
            return Error::Filesystem {
                path: entry.path(),
                source: source_error,
            };
        })?;
        if file_type.is_symlink() {
            return Err(bundle_error(
                "canonical session contains a top-level symlink",
            ));
        }
        if file_type.is_dir() {
            continue;
        }
        if file_type.is_block_device()
            || file_type.is_char_device()
            || file_type.is_fifo()
            || file_type.is_socket()
        {
            return Err(bundle_error(
                "canonical session contains a top-level special file",
            ));
        }
        write_secure(
            &destination.join(entry.file_name()),
            &fs_err::read(entry.path()).map_err(|source_error| {
                return Error::Filesystem {
                    path: entry.path(),
                    source: source_error,
                };
            })?,
        )?;
    }
    return Ok(());
}

fn planned(path: &str, sensitivity: ArtifactSensitivity, relationship: &str) -> BundlePlanMember {
    return BundlePlanMember {
        path: path.to_owned(),
        sensitivity,
        relationship: relationship.to_owned(),
    };
}

fn bundle_relationship(path: &str) -> &str {
    return match path {
        "screen.txt" => "selected_screen",
        "observation.json" => "selected_observation",
        "verification.json" => "source_verification",
        "events-metadata.jsonl" => "metadata_only_event_projection",
        value if value.starts_with("canonical/") => "canonical_evidence",
        _ => "bundle_payload",
    };
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|source| return json_error(path.to_string_lossy().as_ref(), source))?;
    bytes.push(b'\n');
    return write_secure(path, &bytes);
}

fn json_error(path: &str, source: serde_json::Error) -> Error {
    return Error::InvalidJson {
        path: PathBuf::from(path),
        json_path: "$".to_owned(),
        source,
    };
}

fn bundle_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}
