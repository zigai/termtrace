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
        ArtifactMember, ArtifactSensitivity, collect_artifact_members_bounded, commit_directory,
        create_private_directory, finish_writer, hash_regular_file, open_regular_file_nofollow,
        secure_file, secure_writer, sync_directory, write_secure,
    },
    event::{ByteString, EventProjection},
    event_log::EventLog,
    observation::{
        CaptureQuality, Observation, ObservationIntegrity, ObservationResolution,
        ObservationSourceKind, StatePoint, TruthLayer, frame_from_state, frame_hash,
        observe_session,
    },
    replay::{StateSelector, inspect},
    session::{
        SessionCompletion, SessionManifest, VerificationLevel, VerificationReport, read_manifest,
        validate_completed_log, verify_session,
    },
};

const MAX_BUNDLE_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_BUNDLE_STRUCTURED_MEMBER_BYTES: u64 = 64 * 1024 * 1024;
const MAX_BUNDLE_MEMBERS: usize = 10_000;
const MAX_BUNDLE_TREE_ENTRIES: usize = MAX_BUNDLE_MEMBERS + 2;

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

/// Caller-owned source and policy expectations for untrusted bundle verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleVerificationPolicy {
    /// Trusted canonical source session selected by the verifier.
    pub source_session: PathBuf,
    /// Exact bundle profile the verifier expects.
    pub profile: BundleProfile,
}

impl BundleVerificationPolicy {
    /// Construct explicit verification expectations.
    #[must_use]
    pub fn new(source_session: impl Into<PathBuf>, profile: BundleProfile) -> Self {
        return Self {
            source_session: source_session.into(),
            profile,
        };
    }
}

struct TrustedBundleSource {
    canonical: PathBuf,
    manifest: SessionManifest,
    completion: SessionCompletion,
    log: EventLog,
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
            let verification = verify_session(&canonical, VerificationLevel::Content);
            if !verification.valid {
                return Err(bundle_error("canonical source failed content verification"));
            }
            write_json(&staging.join("verification.json"), &verification)?;
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
        let mut members = collect_artifact_members_bounded(&staging, MAX_BUNDLE_TREE_ENTRIES)?;
        for member in &mut members {
            let semantics = bundle_member_semantics(profile, &member.path).ok_or_else(|| {
                return bundle_error(format!(
                    "created bundle member `{}` is outside the selected profile",
                    member.path
                ));
            })?;
            member.sensitivity = semantics.sensitivity;
            member.relationship = semantics.relationship.to_owned();
            member.schema_type = semantics.schema_type.map(str::to_owned);
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

/// Verify a bundle against caller-owned source and profile expectations.
pub fn verify(directory: &Path, policy: &BundleVerificationPolicy) -> Result<BundleManifest> {
    let trusted = resolve_trusted_source(policy)?;
    let path = directory.join("manifest.json");
    let manifest_bytes = read_bounded_regular_file(&path, MAX_BUNDLE_MANIFEST_BYTES)?;
    let manifest: BundleManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|source| return json_error("manifest.json", source))?;
    if manifest.format != "termtrace-bundle" || manifest.version != 1 {
        return Err(bundle_error("unsupported bundle manifest"));
    }
    if manifest.members.len() > MAX_BUNDLE_MEMBERS {
        return Err(bundle_error(format!(
            "bundle manifest exceeds {MAX_BUNDLE_MEMBERS} members"
        )));
    }
    let expected_canonical = trusted.canonical.as_os_str().as_encoded_bytes();
    if manifest.profile != policy.profile
        || manifest.session_id != trusted.manifest.session_id
        || manifest.canonical_session.as_bytes() != expected_canonical
        || manifest.source_final_event_hash != trusted.completion.final_event_hash
        || manifest.includes_canonical_evidence != (policy.profile == BundleProfile::Forensic)
    {
        return Err(bundle_error(
            "bundle manifest does not match the expected profile or canonical source identity",
        ));
    }
    verify_bundle_members(directory, policy.profile, &manifest.members)?;
    verify_bound_payloads(directory, &manifest, &trusted)?;
    if manifest.includes_canonical_evidence {
        let report = verify_session(directory.join("canonical"), VerificationLevel::Content);
        if !report.valid
            || report.session_id != Some(manifest.session_id)
            || report.final_event_hash.as_deref() != Some(manifest.source_final_event_hash.as_str())
        {
            return Err(bundle_error(
                "included canonical evidence failed validation",
            ));
        }
        verify_canonical_payload_matches_source(directory, &manifest.members, &trusted.canonical)?;
    }
    return Ok(manifest);
}

/// Publish one deterministic Zstandard-compressed USTAR bundle archive.
pub fn archive(
    directory: &Path,
    destination: &Path,
    policy: &BundleVerificationPolicy,
) -> Result<()> {
    verify(directory, policy)?;
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
pub fn verify_archive(archive: &Path, policy: &BundleVerificationPolicy) -> Result<BundleManifest> {
    let parent = archive
        .parent()
        .ok_or_else(|| return bundle_error("archive path has no parent"))?;
    let staging = parent.join(format!(".bundle-verify-{}.tmp", Uuid::now_v7()));
    create_private_directory(&staging)?;
    let result = (|| {
        extract_archive(archive, &staging)?;
        return verify(&staging, policy);
    })();
    let _cleanup = fs_err::remove_dir_all(&staging);
    return result;
}

fn resolve_trusted_source(policy: &BundleVerificationPolicy) -> Result<TrustedBundleSource> {
    let canonical = policy.source_session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: policy.source_session.clone(),
            source,
        };
    })?;
    let log = EventLog::read(canonical.join("events.jsonl"))?;
    let completion = validate_completed_log(&canonical, &log)?;
    let manifest = read_manifest(&canonical)?;
    return Ok(TrustedBundleSource {
        canonical,
        manifest,
        completion,
        log,
    });
}

fn verify_bound_payloads(
    directory: &Path,
    manifest: &BundleManifest,
    trusted: &TrustedBundleSource,
) -> Result<()> {
    let observation_path = directory.join("observation.json");
    let observation: Observation = serde_json::from_slice(&read_bounded_regular_file(
        &observation_path,
        MAX_BUNDLE_STRUCTURED_MEMBER_BYTES,
    )?)
    .map_err(|source| return json_error("observation.json", source))?;
    let inspection = inspect(&trusted.log, StateSelector::LatestPresented)?;
    let expected_frame = frame_from_state(&inspection.state);
    let expected_frame_hash = frame_hash(&expected_frame)?;
    if observation.session_id != Some(trusted.manifest.session_id)
        || observation.source_kind != ObservationSourceKind::CompletedSession
        || observation
            .canonical_session
            .as_ref()
            .map(ByteString::as_bytes)
            != Some(trusted.canonical.as_os_str().as_encoded_bytes())
        || !observation.source_completed
        || observation.state_point != StatePoint::LatestPresented
        || observation.state_id != Some(inspection.record.state_id)
        || observation.state_hash != inspection.record.state_hash
        || observation.event_seq != inspection.record.event_seq
        || observation.monotonic_ns != Some(inspection.record.monotonic_ns)
        || !observation.presented
        || observation.boundary.next_event_seq != trusted.completion.event_count
        || observation.boundary.event_hash != trusted.completion.final_event_hash
        || observation.boundary.output_bytes != trusted.completion.output_bytes
        || observation.capture_quality != CaptureQuality::Completed
        || observation.truth_layer != TruthLayer::Semantic
        || observation.integrity != ObservationIntegrity::CanonicalChained
        || !matches!(
            observation.resolution,
            ObservationResolution::CanonicalReplay | ObservationResolution::ValidatedCheckpoint
        )
        || observation.frame != expected_frame
        || observation.payload_hash != expected_frame_hash
    {
        return Err(bundle_error(
            "bundle observation does not derive from the expected canonical source",
        ));
    }
    let screen = read_bounded_regular_file(
        &directory.join("screen.txt"),
        MAX_BUNDLE_STRUCTURED_MEMBER_BYTES,
    )?;
    if screen != observation.frame.viewport_text.as_bytes() {
        return Err(bundle_error(
            "bundle screen text does not match its source-bound observation",
        ));
    }

    if matches!(
        manifest.profile,
        BundleProfile::Debug | BundleProfile::Forensic
    ) {
        let verification: VerificationReport = serde_json::from_slice(&read_bounded_regular_file(
            &directory.join("verification.json"),
            MAX_BUNDLE_STRUCTURED_MEMBER_BYTES,
        )?)
        .map_err(|source| return json_error("verification.json", source))?;
        let expected_verification = verify_session(&trusted.canonical, VerificationLevel::Content);
        if !expected_verification.valid || verification != expected_verification {
            return Err(bundle_error(
                "bundle verification report does not bind the expected canonical source",
            ));
        }
    }

    if manifest.profile == BundleProfile::Debug {
        let metadata = manifest_member(manifest, "events-metadata.jsonl")?;
        let mut expected = HashingWriter::default();
        for record in &trusted.log.events {
            serde_json::to_writer(&mut expected, &EventProjection::from_record(record, true))
                .map_err(|source| return json_error("events-metadata.jsonl", source))?;
            expected.write_all(b"\n").map_err(|source| {
                return Error::Filesystem {
                    path: PathBuf::from("events-metadata.jsonl"),
                    source,
                };
            })?;
        }
        if metadata.byte_length != expected.bytes
            || metadata.blake3 != expected.hasher.finalize().to_hex().as_str()
        {
            return Err(bundle_error(
                "metadata-only event projection does not match the expected canonical source",
            ));
        }
    }
    return Ok(());
}

fn verify_canonical_payload_matches_source(
    directory: &Path,
    members: &[ArtifactMember],
    canonical_source: &Path,
) -> Result<()> {
    let source_names = top_level_regular_file_names(canonical_source)?;
    let expected_paths = source_names
        .iter()
        .map(|name| return format!("canonical/{name}"))
        .collect::<BTreeSet<_>>();
    let included_paths = members
        .iter()
        .filter_map(|member| {
            return canonical_member_name(&member.path).map(|_| return member.path.clone());
        })
        .collect::<BTreeSet<_>>();
    if included_paths != expected_paths {
        return Err(bundle_error(
            "included canonical payload does not match the expected source inventory",
        ));
    }
    for name in source_names {
        let source = canonical_source.join(&name);
        let included = directory.join("canonical").join(&name);
        let expected = hash_regular_file(&source)?;
        let actual = hash_regular_file(&included)?;
        if actual != expected {
            return Err(bundle_error(format!(
                "included canonical `{name}` does not match the expected source"
            )));
        }
    }
    return Ok(());
}

fn manifest_member<'manifest>(
    manifest: &'manifest BundleManifest,
    path: &str,
) -> Result<&'manifest ArtifactMember> {
    return manifest
        .members
        .iter()
        .find(|member| return member.path == path)
        .ok_or_else(|| return bundle_error(format!("bundle is missing required member `{path}`")));
}

fn read_bounded_regular_file(path: &Path, maximum_bytes: u64) -> Result<Vec<u8>> {
    let (mut file, size) = open_regular_file_nofollow(path)?;
    if size > maximum_bytes {
        return Err(bundle_error(format!(
            "bundle member `{}` exceeds {maximum_bytes} byte limit",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    io::Read::by_ref(&mut file)
        .take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum_bytes {
        return Err(bundle_error(format!(
            "bundle member `{}` exceeds {maximum_bytes} byte limit",
            path.display()
        )));
    }
    return Ok(bytes);
}

#[derive(Default)]
struct HashingWriter {
    hasher: blake3::Hasher,
    bytes: u64,
}

impl io::Write for HashingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.hasher.update(buf);
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(buf.len()).unwrap_or(u64::MAX));
        return Ok(buf.len());
    }

    fn flush(&mut self) -> io::Result<()> {
        return Ok(());
    }
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
    let maximum_archive_members = u64::try_from(MAX_BUNDLE_MEMBERS)
        .unwrap_or(u64::MAX)
        .saturating_add(1); // payload members plus manifest.json
    for entry_count in 0..=maximum_archive_members {
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
        if entry_count == maximum_archive_members {
            return Err(bundle_error(format!(
                "bundle archive exceeds {maximum_archive_members} members"
            )));
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
        if path == Path::new("manifest.json") && size > MAX_BUNDLE_MANIFEST_BYTES {
            return Err(bundle_error(format!(
                "bundle manifest exceeds {MAX_BUNDLE_MANIFEST_BYTES} byte limit"
            )));
        }
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

fn verify_bundle_members(
    directory: &Path,
    profile: BundleProfile,
    expected: &[ArtifactMember],
) -> Result<()> {
    if expected.len() > MAX_BUNDLE_MEMBERS {
        return Err(bundle_error(format!(
            "bundle manifest exceeds {MAX_BUNDLE_MEMBERS} members"
        )));
    }
    let expected_paths = expected
        .iter()
        .map(|member| return member.path.as_str())
        .collect::<BTreeSet<_>>();
    if expected_paths.len() != expected.len() {
        return Err(bundle_error(
            "bundle manifest contains duplicate member paths",
        ));
    }

    let mut actual = collect_artifact_members_bounded(directory, MAX_BUNDLE_TREE_ENTRIES)?;
    if actual.len() > MAX_BUNDLE_MEMBERS || actual.len() != expected.len() {
        return Err(bundle_error(
            "bundle payload members do not match their whole-file manifest",
        ));
    }
    for member in &mut actual {
        let semantics = bundle_member_semantics(profile, &member.path).ok_or_else(|| {
            return bundle_error(format!(
                "bundle member `{}` is outside the selected profile",
                member.path
            ));
        })?;
        member.sensitivity = semantics.sensitivity;
        member.relationship = semantics.relationship.to_owned();
        member.schema_type = semantics.schema_type.map(str::to_owned);
    }
    if actual != expected {
        return Err(bundle_error(
            "bundle payload members or semantic metadata do not match the selected profile",
        ));
    }
    for required in ["screen.txt", "observation.json"] {
        if !expected_paths.contains(required) {
            return Err(bundle_error(format!(
                "bundle is missing required member `{required}`"
            )));
        }
    }
    if matches!(profile, BundleProfile::Debug | BundleProfile::Forensic)
        && !expected_paths.contains("verification.json")
    {
        return Err(bundle_error(
            "bundle is missing required member `verification.json`",
        ));
    }
    if profile == BundleProfile::Debug && !expected_paths.contains("events-metadata.jsonl") {
        return Err(bundle_error(
            "bundle is missing required member `events-metadata.jsonl`",
        ));
    }
    if profile == BundleProfile::Forensic {
        for required in [
            "canonical/session.json",
            "canonical/events.jsonl",
            "canonical/completion.json",
        ] {
            if !expected_paths.contains(required) {
                return Err(bundle_error(format!(
                    "forensic bundle is missing required member `{required}`"
                )));
            }
        }
    }
    return Ok(());
}

fn copy_top_level_files(source: &Path, destination: &Path) -> Result<()> {
    for name in top_level_regular_file_names(source)? {
        let path = source.join(&name);
        let (mut reader, _size) = open_regular_file_nofollow(&path)?;
        let target = destination.join(name);
        let mut writer = secure_writer(&target)?;
        io::copy(&mut reader, &mut writer).map_err(|source_error| {
            return Error::Filesystem {
                path: path.clone(),
                source: source_error,
            };
        })?;
        finish_writer(writer, &target)?;
    }
    return Ok(());
}

fn top_level_regular_file_names(directory: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs_err::read_dir(directory).map_err(|source_error| {
        return Error::Filesystem {
            path: directory.to_path_buf(),
            source: source_error,
        };
    })? {
        let entry = entry.map_err(|source_error| {
            return Error::Filesystem {
                path: directory.to_path_buf(),
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
        let name = entry.file_name().into_string().map_err(|_name| {
            return bundle_error("canonical top-level file names must be valid UTF-8");
        })?;
        names.push(name);
        if names.len() > MAX_BUNDLE_MEMBERS {
            return Err(bundle_error(format!(
                "canonical source exceeds {MAX_BUNDLE_MEMBERS} top-level files"
            )));
        }
    }
    names.sort();
    return Ok(names);
}

fn planned(path: &str, sensitivity: ArtifactSensitivity, relationship: &str) -> BundlePlanMember {
    return BundlePlanMember {
        path: path.to_owned(),
        sensitivity,
        relationship: relationship.to_owned(),
    };
}

#[derive(Clone, Copy)]
struct BundleMemberSemantics {
    sensitivity: ArtifactSensitivity,
    relationship: &'static str,
    schema_type: Option<&'static str>,
}

fn bundle_member_semantics(profile: BundleProfile, path: &str) -> Option<BundleMemberSemantics> {
    let sensitive = ArtifactSensitivity::SensitiveTerminalData;
    let operational = ArtifactSensitivity::OperationalMetadata;
    return match path {
        "screen.txt" => Some(BundleMemberSemantics {
            sensitivity: sensitive,
            relationship: "selected_screen",
            schema_type: None,
        }),
        "observation.json" => Some(BundleMemberSemantics {
            sensitivity: sensitive,
            relationship: "selected_observation",
            schema_type: Some("observation.schema.json"),
        }),
        "verification.json"
            if matches!(profile, BundleProfile::Debug | BundleProfile::Forensic) =>
        {
            Some(BundleMemberSemantics {
                sensitivity: operational,
                relationship: "source_verification",
                schema_type: Some("verification-report.schema.json"),
            })
        }
        "events-metadata.jsonl" if profile == BundleProfile::Debug => Some(BundleMemberSemantics {
            sensitivity: operational,
            relationship: "metadata_only_event_projection",
            schema_type: Some("event-projection.schema.json"),
        }),
        value if profile == BundleProfile::Forensic && canonical_member_name(value).is_some() => {
            Some(BundleMemberSemantics {
                sensitivity: sensitive,
                relationship: "canonical_evidence",
                schema_type: canonical_schema_type(value),
            })
        }
        _ => None,
    };
}

fn canonical_member_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix("canonical/")?;
    return (!name.is_empty() && !name.contains('/')).then_some(name);
}

fn canonical_schema_type(path: &str) -> Option<&'static str> {
    return match canonical_member_name(path)? {
        "session.json" => Some("session.schema.json"),
        "completion.json" => Some("completion.schema.json"),
        "artifact-roots.jsonl" => Some("artifact-root-record.schema.json"),
        _ => None,
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

#[cfg(test)]
mod tests {
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
}
