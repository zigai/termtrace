//! Deterministic profile planning, publication, and source-bound verification.

use crate::{Error, Result};
use serde::{de::DeserializeOwned, Serialize};
use std::{
    collections::BTreeSet,
    fs::File,
    io::{self, Read, Write},
    os::unix::fs::FileTypeExt as _,
    path::{Path, PathBuf},
};
use termtrace_engine::{observation::observe_session, replay::ValidatedSession};
use termtrace_evidence::{
    artifact::{hash_regular_file, member_media_type, member_schema_type},
    bundle::{verify as verify_bundle, BundleDraft, BundleIdentity, BundleMemberMetadata},
    event_log::{CanonicalLogFile, EventLogCursor},
    session::verify_session,
};
use termtrace_model::{
    artifact::bundle::{
        BundleManifest, BundlePlan, BundlePlanMember, BundleProfile, BundleVerificationPolicy,
        BUNDLE_FORMAT, BUNDLE_VERSION,
    },
    artifact::metadata::{ArtifactMember, ArtifactSensitivity},
    execution::event::EventProjection,
    execution::layout::{self, file_role, FileRole, SessionLayout},
    execution::session::{VerificationLevel, VerificationReport},
    terminal::observation::{Observation, ObservationResolution, StatePoint},
    value::ByteString,
};

const MAX_STRUCTURED_MEMBER_BYTES: u64 = 64 * 1024 * 1024;

/// Return deterministic content and sensitivity policy without writing.
#[must_use]
pub fn plan(profile: BundleProfile) -> BundlePlan {
    let mut members: Vec<BundlePlanMember> = vec![
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
            "canonical_session_files",
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

/// Publish one manifested evidence export without replacing an existing destination.
///
/// # Errors
///
/// Returns an error when the source session cannot be trusted, `destination` is inside it or
/// already exists, the export would exceed `max_bytes`, or evidence cannot be observed, written, or
/// published.
pub fn create(
    session: &Path,
    destination: &Path,
    profile: BundleProfile,
    max_bytes: Option<u64>,
) -> Result<BundleManifest> {
    let trusted = TrustedSource::resolve(session)?;

    if destination.starts_with(trusted.canonical()) {
        return Err(Error::invalid(
            "evidence export destination must be outside the canonical session",
        ));
    }

    let identity = trusted.identity(profile);
    let mut draft = BundleDraft::create(destination, identity, max_bytes)?;
    let observation = observe_session(trusted.canonical(), StatePoint::LatestPresented)?;
    draft.add_member_bytes(
        "screen.txt",
        member_metadata(profile, "screen.txt")?,
        observation.frame.viewport_text.as_bytes(),
    )?;
    add_json_member(
        &mut draft,
        "observation.json",
        member_metadata(profile, "observation.json")?,
        &observation,
    )?;

    if matches!(profile, BundleProfile::Debug | BundleProfile::Forensic) {
        let verification = verify_session(trusted.canonical(), VerificationLevel::Content);
        if !verification.valid {
            return Err(Error::invalid(
                "canonical source failed content verification",
            ));
        }

        add_json_member(
            &mut draft,
            "verification.json",
            member_metadata(profile, "verification.json")?,
            &verification,
        )?;
    }

    if profile == BundleProfile::Debug {
        let metadata = member_metadata(profile, "events-metadata.jsonl")?;
        let mut writer = draft.start_member("events-metadata.jsonl", metadata)?;
        trusted.write_event_projection(&mut writer)?;
        writer.finish()?;
    }

    if profile == BundleProfile::Forensic {
        for name in canonical_file_names(trusted.canonical())? {
            let source = trusted.canonical().join(&name);
            let mut reader: Box<dyn Read> = if name == layout::EVENT_LOG {
                Box::new(
                    CanonicalLogFile::open(&source)
                        .map_err(|error| return Error::filesystem(&source, error))?,
                )
            } else {
                Box::new(
                    File::open(&source)
                        .map_err(|error| return Error::filesystem(&source, error))?,
                )
            };

            let relative = format!("canonical/{name}");
            draft.add_member_from_reader(
                &relative,
                member_metadata(profile, &relative)?,
                &mut reader,
            )?;
        }
    }

    trusted.session.check()?;

    return Ok(draft.commit()?);
}

/// Verify an evidence export against a caller-owned source and exact profile policy.
///
/// # Errors
///
/// Returns an error when the source session cannot be trusted, the bundle does not match the source
/// identity, its members or payloads fail validation or violate the profile policy, or forensic
/// canonical evidence fails content verification.
pub fn verify_profile(
    directory: &Path,
    policy: &BundleVerificationPolicy,
) -> Result<BundleManifest> {
    let trusted = TrustedSource::resolve(&policy.source_session)?;
    let identity = trusted.identity(policy.profile);
    let manifest = verify_bundle(directory, Some(&identity)).map_err(|error| {
        if let termtrace_evidence::Error::InvalidEvidence { reason, .. } = &error
            && (reason == "bundle manifest does not match the caller-owned source identity"
                || reason == "bundle manifest does not match the canonical source identity")
        {
            return Error::invalid(reason.clone());
        }
        return Error::invalid(format!(
            "bundle payload members or semantic metadata failed validation: {error}"
        ));
    })?;

    verify_member_policy(policy.profile, &manifest.members)?;
    verify_bound_payloads(directory, &manifest, &trusted)?;

    if policy.profile == BundleProfile::Forensic {
        let report = verify_session(directory.join("canonical"), VerificationLevel::Content);

        if !report.valid
            || report.session_id != Some(manifest.session_id)
            || report.final_event_hash.as_deref() != Some(manifest.source_final_event_hash.as_str())
        {
            return Err(Error::invalid(
                "included canonical evidence failed validation",
            ));
        }

        verify_canonical_payload(directory, &manifest.members, trusted.canonical())?;
    }

    return Ok(manifest);
}

pub(super) fn identity_for_policy(policy: &BundleVerificationPolicy) -> Result<BundleIdentity> {
    return Ok(TrustedSource::resolve(&policy.source_session)?.identity(policy.profile));
}

struct TrustedSource {
    session: ValidatedSession,
}

impl TrustedSource {
    fn resolve(session: &Path) -> Result<Self> {
        let canonical: PathBuf = session
            .canonicalize()
            .map_err(|source| return Error::filesystem(session, source))?;

        return Ok(Self {
            session: ValidatedSession::open(&canonical)?,
        });
    }

    fn canonical(&self) -> &Path {
        return self.session.root();
    }

    fn write_event_projection(&self, writer: &mut impl Write) -> Result<()> {
        let completion = self.session.completion();
        let mut cursor = EventLogCursor::open(SessionLayout::new(self.canonical()).event_log())?;

        while let Some(record) = cursor.next_record()? {
            serde_json::to_writer(&mut *writer, &EventProjection::from_record(&record, true))
                .map_err(|source| return Error::json("events-metadata.jsonl", source))?;

            writer
                .write_all(b"\n")
                .map_err(|source| return Error::filesystem("events-metadata.jsonl", source))?;
        }

        cursor.validate_boundary(completion.event_count, &completion.final_event_hash)?;
        let _pending = cursor.finish()?;

        return Ok(());
    }

    fn identity(&self, profile: BundleProfile) -> BundleIdentity {
        let completion = self.session.completion();

        return BundleIdentity {
            profile,
            session_id: completion.session_id,
            canonical_session: ByteString::from(self.canonical().as_os_str().as_encoded_bytes()),
            source_final_event_hash: completion.final_event_hash.clone(),
            includes_canonical_evidence: profile == BundleProfile::Forensic,
        };
    }
}

fn verify_bound_payloads(
    directory: &Path,
    manifest: &BundleManifest,
    trusted: &TrustedSource,
) -> Result<()> {
    let observation: Observation = read_json_bounded(
        &directory.join("observation.json"),
        MAX_STRUCTURED_MEMBER_BYTES,
    )?;

    if !matches!(
        observation.resolution,
        ObservationResolution::CanonicalReplay | ObservationResolution::ValidatedCheckpoint
    ) {
        return Err(Error::invalid(
            "bundle observation has an invalid source resolution",
        ));
    }

    let mut expected = observe_session(trusted.canonical(), StatePoint::LatestPresented)?;
    expected.resolution = observation.resolution;

    if observation != expected {
        return Err(Error::invalid(
            "bundle observation does not derive from the expected canonical source",
        ));
    }

    let screen = read_bounded(&directory.join("screen.txt"), MAX_STRUCTURED_MEMBER_BYTES)?;

    if screen != observation.frame.viewport_text.as_bytes() {
        return Err(Error::invalid(
            "bundle screen text does not match its source-bound observation",
        ));
    }

    if matches!(
        manifest.profile,
        BundleProfile::Debug | BundleProfile::Forensic
    ) {
        let verification: VerificationReport = read_json_bounded(
            &directory.join("verification.json"),
            MAX_STRUCTURED_MEMBER_BYTES,
        )?;

        let expected_verification = verify_session(trusted.canonical(), VerificationLevel::Content);

        if !expected_verification.valid || verification != expected_verification {
            return Err(Error::invalid(
                "bundle verification report does not bind the expected canonical source",
            ));
        }
    }

    if manifest.profile == BundleProfile::Debug {
        let member = manifest_member(manifest, "events-metadata.jsonl")?;
        let mut expected = HashingWriter::default();
        trusted.write_event_projection(&mut expected)?;
        if member.byte_length != expected.bytes
            || member.blake3 != expected.hasher.finalize().to_hex().as_str()
        {
            return Err(Error::invalid(
                "metadata-only event projection does not match the expected canonical source",
            ));
        }
    }

    return Ok(());
}

fn verify_member_policy(profile: BundleProfile, members: &[ArtifactMember]) -> Result<()> {
    let paths: BTreeSet<&str> = members
        .iter()
        .map(|member| return member.path.as_str())
        .collect();

    if paths.len() != members.len() {
        return Err(Error::invalid(
            "bundle manifest contains duplicate member paths",
        ));
    }

    for member in members {
        let expected = member_metadata(profile, &member.path)?;
        if member.media_type != expected.media_type
            || member.schema_type != expected.schema_type
            || member.sensitivity != expected.sensitivity
            || member.relationship != expected.relationship
        {
            return Err(Error::invalid(format!(
                "bundle member `{}` has sensitivity or semantic metadata outside the selected profile",
                member.path
            )));
        }
    }

    for required in ["screen.txt", "observation.json"] {
        require_member(&paths, required)?;
    }

    if matches!(profile, BundleProfile::Debug | BundleProfile::Forensic) {
        require_member(&paths, "verification.json")?;
    }

    if profile == BundleProfile::Debug {
        require_member(&paths, "events-metadata.jsonl")?;
    }

    if profile == BundleProfile::Forensic {
        for required in [layout::MANIFEST, layout::EVENT_LOG, layout::COMPLETION] {
            require_member(&paths, &format!("canonical/{required}"))?;
        }
    }

    return Ok(());
}

pub(super) fn verify_archived_manifest(
    manifest: &BundleManifest,
    policy: &BundleVerificationPolicy,
) -> Result<()> {
    let trusted = TrustedSource::resolve(&policy.source_session)?;
    let identity = trusted.identity(policy.profile);

    if manifest.format != BUNDLE_FORMAT
        || manifest.version != BUNDLE_VERSION
        || manifest.profile != identity.profile
        || manifest.session_id != identity.session_id
        || manifest.canonical_session != identity.canonical_session
        || manifest.source_final_event_hash != identity.source_final_event_hash
        || manifest.includes_canonical_evidence != identity.includes_canonical_evidence
    {
        return Err(Error::invalid(
            "archive manifest does not match the requested source and profile",
        ));
    }

    verify_member_policy(policy.profile, &manifest.members)?;

    let mut observation = observe_session(trusted.canonical(), StatePoint::LatestPresented)?;
    let screen = observation.frame.viewport_text.as_bytes();
    verify_member_bytes(manifest, "screen.txt", screen)?;

    let mut observation_matches = false;

    for resolution in [
        ObservationResolution::CanonicalReplay,
        ObservationResolution::ValidatedCheckpoint,
    ] {
        observation.resolution = resolution;
        let bytes = encoded_json_member(&observation, "observation.json")?;

        if member_matches_bytes(manifest_member(manifest, "observation.json")?, &bytes) {
            observation_matches = true;
            break;
        }
    }

    if !observation_matches {
        return Err(Error::invalid(
            "archived observation does not derive from the expected canonical source",
        ));
    }

    if matches!(
        policy.profile,
        BundleProfile::Debug | BundleProfile::Forensic
    ) {
        let verification = verify_session(trusted.canonical(), VerificationLevel::Content);
        if !verification.valid {
            return Err(Error::invalid(
                "canonical source failed content verification",
            ));
        }

        let bytes = encoded_json_member(&verification, "verification.json")?;
        verify_member_bytes(manifest, "verification.json", &bytes)?;
    }

    if policy.profile == BundleProfile::Debug {
        let mut expected = HashingWriter::default();
        trusted.write_event_projection(&mut expected)?;

        let member = manifest_member(manifest, "events-metadata.jsonl")?;

        if member.byte_length != expected.bytes
            || member.blake3 != expected.hasher.finalize().to_hex().as_str()
        {
            return Err(Error::invalid(
                "archived event projection does not match the expected canonical source",
            ));
        }
    }

    if policy.profile == BundleProfile::Forensic {
        let names = canonical_file_names(trusted.canonical())?;
        let expected: BTreeSet<String> = names
            .iter()
            .map(|name| format!("canonical/{name}"))
            .collect();

        let included: BTreeSet<String> = manifest
            .members
            .iter()
            .filter(|member| return canonical_member_name(&member.path).is_some())
            .map(|member| return member.path.clone())
            .collect();

        if included != expected {
            return Err(Error::invalid(
                "archived canonical payload does not match the expected source inventory",
            ));
        }

        for name in names {
            let source = trusted.canonical().join(&name);
            let member = manifest_member(manifest, &format!("canonical/{name}"))?;
            let (byte_length, blake3) = hash_regular_file(&source)?;
            if member.byte_length != byte_length || member.blake3 != blake3 {
                return Err(Error::invalid(format!(
                    "archived canonical member `{name}` does not match the trusted source"
                )));
            }
        }
    }

    return Ok(());
}

fn encoded_json_member(value: &impl Serialize, path: &str) -> Result<Vec<u8>> {
    let mut bytes =
        serde_json::to_vec_pretty(value).map_err(|source| return Error::json(path, source))?;
    bytes.push(b'\n');
    return Ok(bytes);
}

fn member_matches_bytes(member: &ArtifactMember, bytes: &[u8]) -> bool {
    return member.byte_length == u64::try_from(bytes.len()).unwrap_or(u64::MAX)
        && member.blake3 == blake3::hash(bytes).to_hex().as_str();
}

fn verify_member_bytes(manifest: &BundleManifest, path: &str, bytes: &[u8]) -> Result<()> {
    if !member_matches_bytes(manifest_member(manifest, path)?, bytes) {
        return Err(Error::invalid(format!(
            "archived member `{path}` does not match the expected canonical source"
        )));
    }
    return Ok(());
}

fn require_member(paths: &BTreeSet<&str>, path: &str) -> Result<()> {
    if !paths.contains(path) {
        return Err(Error::invalid(format!(
            "bundle is missing required member `{path}`"
        )));
    }
    return Ok(());
}

fn verify_canonical_payload(
    directory: &Path,
    members: &[ArtifactMember],
    source: &Path,
) -> Result<()> {
    let source_names = canonical_file_names(source)?;
    let expected: BTreeSet<String> = source_names
        .iter()
        .map(|name| format!("canonical/{name}"))
        .collect();

    let included: BTreeSet<String> = members
        .iter()
        .filter(|member| return canonical_member_name(&member.path).is_some())
        .map(|member| return member.path.clone())
        .collect();

    if included != expected {
        return Err(Error::invalid(
            "included canonical payload does not match the expected source inventory",
        ));
    }

    for name in source_names {
        let expected_hash = if name == layout::EVENT_LOG {
            let path = source.join(&name);
            let mut file = CanonicalLogFile::open(&path)
                .map_err(|error| return Error::filesystem(&path, error))?;

            let mut hash = blake3::Hasher::new();
            let bytes = io::copy(&mut file, &mut hash)
                .map_err(|error| return Error::filesystem(&path, error))?;

            (bytes, hash.finalize().to_hex().to_string())
        } else {
            hash_regular_file(&source.join(&name))?
        };

        let actual_hash = hash_regular_file(&directory.join("canonical").join(&name))?;
        if actual_hash != expected_hash {
            return Err(Error::invalid(format!(
                "included canonical `{name}` does not match the expected source"
            )));
        }
    }

    return Ok(());
}

/// Names of the top-level entries the session layout classifies as canonical, sorted.
fn canonical_file_names(directory: &Path) -> Result<Vec<String>> {
    let mut names: Vec<String> = Vec::new();
    for entry in
        fs_err::read_dir(directory).map_err(|source| return Error::filesystem(directory, source))?
    {
        let entry = entry.map_err(|source| return Error::filesystem(directory, source))?;
        let Some(name) = entry
            .file_name()
            .to_str()
            .filter(|name| return file_role(name) == Some(FileRole::Canonical))
            .map(str::to_owned)
        else {
            continue;
        };

        let file_type = entry
            .file_type()
            .map_err(|source| return Error::filesystem(entry.path(), source))?;

        if file_type.is_symlink()
            || file_type.is_dir()
            || file_type.is_block_device()
            || file_type.is_char_device()
            || file_type.is_fifo()
            || file_type.is_socket()
        {
            return Err(Error::invalid(format!(
                "canonical session file `{name}` is not a regular file"
            )));
        }

        names.push(name);
    }

    names.sort();
    return Ok(names);
}

fn member_metadata(profile: BundleProfile, path: &str) -> Result<BundleMemberMetadata> {
    let sensitive = ArtifactSensitivity::SensitiveTerminalData;
    let operational = ArtifactSensitivity::OperationalMetadata;
    let (sensitivity, relationship, schema_type) = match path {
        "screen.txt" => (sensitive, "selected_screen", None),
        "observation.json" => (
            sensitive,
            "selected_observation",
            Some("observation.schema.json"),
        ),
        "verification.json"
            if matches!(profile, BundleProfile::Debug | BundleProfile::Forensic) =>
        {
            (
                operational,
                "source_verification",
                Some("verification-report.schema.json"),
            )
        }
        "events-metadata.jsonl" if profile == BundleProfile::Debug => (
            operational,
            "metadata_only_event_projection",
            Some("event-projection.schema.json"),
        ),
        value if profile == BundleProfile::Forensic && canonical_member_name(value).is_some() => {
            (sensitive, "canonical_evidence", member_schema_type(value))
        }
        _ => {
            return Err(Error::invalid(format!(
                "bundle member `{path}` is outside the selected profile"
            )));
        }
    };

    return Ok(BundleMemberMetadata {
        media_type: member_media_type(path).to_owned(),
        schema_type: schema_type.map(str::to_owned),
        sensitivity,
        relationship: relationship.to_owned(),
    });
}

fn planned(path: &str, sensitivity: ArtifactSensitivity, relationship: &str) -> BundlePlanMember {
    return BundlePlanMember {
        path: path.to_owned(),
        sensitivity,
        relationship: relationship.to_owned(),
    };
}

fn canonical_member_name(path: &str) -> Option<&str> {
    let name = path.strip_prefix("canonical/")?;
    return (!name.is_empty() && !name.contains('/')).then_some(name);
}

fn add_json_member(
    draft: &mut BundleDraft,
    path: &str,
    metadata: BundleMemberMetadata,
    value: &impl Serialize,
) -> Result<()> {
    let mut writer = draft.start_member(path, metadata)?;
    serde_json::to_writer_pretty(&mut writer, value)
        .map_err(|source| return Error::json(path, source))?;
    writer
        .write_all(b"\n")
        .map_err(|source| return Error::filesystem(path, source))?;
    writer.finish()?;

    return Ok(());
}

fn read_json_bounded<T: DeserializeOwned>(path: &Path, maximum: u64) -> Result<T> {
    let bytes = read_bounded(path, maximum)?;
    return serde_json::from_slice(&bytes).map_err(|source| return Error::json(path, source));
}

fn read_bounded(path: &Path, maximum: u64) -> Result<Vec<u8>> {
    let mut file = File::open(path).map_err(|source| return Error::filesystem(path, source))?;
    let size = file
        .metadata()
        .map_err(|source| return Error::filesystem(path, source))?
        .len();

    if size > maximum {
        return Err(Error::invalid(format!(
            "bundle member `{}` exceeds {maximum} bytes",
            path.display()
        )));
    }

    let mut bytes: Vec<u8> = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    Read::by_ref(&mut file)
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| return Error::filesystem(path, source))?;

    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum {
        return Err(Error::invalid(format!(
            "bundle member `{}` exceeds {maximum} bytes",
            path.display()
        )));
    }
    return Ok(bytes);
}

fn manifest_member<'manifest>(
    manifest: &'manifest BundleManifest,
    path: &str,
) -> Result<&'manifest ArtifactMember> {
    return manifest
        .members
        .iter()
        .find(|member| return member.path == path)
        .ok_or_else(|| {
            return Error::invalid(format!("bundle is missing required member `{path}`"));
        });
}

#[derive(Default)]
struct HashingWriter {
    hasher: blake3::Hasher,
    bytes: u64,
}

impl Write for HashingWriter {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use termtrace_model::value::path_bytes::PathValue;

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
            (layout::MANIFEST, b"session".as_slice()),
            (layout::EVENT_LOG, b"events".as_slice()),
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

        verify_canonical_payload(&bundle, &members, &source).expect("exact canonical inventory");

        fs_err::write(SessionLayout::new(&canonical).event_log(), b"changed")
            .expect("tamper member");

        let content_error = verify_canonical_payload(&bundle, &members, &source)
            .expect_err("canonical content must match");
        assert!(content_error.to_string().contains("does not match"));

        fs_err::write(SessionLayout::new(&canonical).event_log(), b"events")
            .expect("restore member");

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
        let inventory_error = verify_canonical_payload(&bundle, &injected_members, &source)
            .expect_err("attacker-added canonical member must be rejected");
        assert!(inventory_error.to_string().contains("source inventory"));
    }

    fn recorded_source(path: &Path) {
        use std::{
            env,
            process::{Command, Stdio},
        };
        let binary = env::var_os("TERMTRACE_BIN").map_or_else(
            || {
                return PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../target/debug/termtrace");
            },
            PathBuf::from,
        );

        let status = Command::new(binary)
            .args(["record", "--no-input", "--out"])
            .arg(path)
            .args(["--", "/bin/sh", "-c", "printf VERIFIED"])
            .stdout(Stdio::null())
            .status()
            .expect("build the real termtrace CLI before owning export tests");
        assert!(status.success());
    }

    fn source_hashes(path: &Path) -> Vec<(String, (u64, String))> {
        return canonical_file_names(path)
            .expect("canonical inventory")
            .into_iter()
            .map(|name| {
                let hash = hash_regular_file(&path.join(&name)).expect("canonical hash");

                return (name, hash);
            })
            .collect();
    }

    #[test]
    fn verification_member_nonutf8_roundtrip_strong_source_binding_and_utf8_compatibility() {
        use std::{
            ffi::OsString,
            os::unix::ffi::{OsStrExt as _, OsStringExt as _},
        };
        let temp = TempDir::new().expect("private root");
        let wrong = temp.path().join("other");
        recorded_source(&wrong);

        for (label, suffix) in [
            ("utf8", "é-λ-\"-\\".as_bytes()),
            ("bytes", b"source-\xff".as_slice()),
        ] {
            let source = temp.path().join(OsString::from_vec(suffix.to_vec()));
            recorded_source(&source);
            let source = source.canonicalize().expect("source");
            let before = source_hashes(&source);

            for profile in [BundleProfile::Debug, BundleProfile::Forensic] {
                let directory = temp.path().join(format!("{label}-{profile:?}"));
                let mut manifest = create(&source, &directory, profile, None).expect("real bundle");
                verify_profile(&directory, &BundleVerificationPolicy::new(&source, profile))
                    .expect("strong directory verification");

                verify_archived_manifest(
                    &manifest,
                    &BundleVerificationPolicy::new(&source, profile),
                )
                .expect("exact archived-member verification");
                assert!(verify_profile(
                    &directory,
                    &BundleVerificationPolicy::new(&wrong, profile)
                )
                .is_err());
                let path = directory.join("verification.json");
                let encoded = fs_err::read(&path).expect("verification bytes");
                let report: VerificationReport =
                    serde_json::from_slice(&encoded).expect("roundtrip");
                assert_eq!(
                    report.session.as_os_str().as_bytes(),
                    source.as_os_str().as_bytes()
                );
                assert_eq!(report, verify_session(&source, VerificationLevel::Content));
                if label == "utf8" {
                    let golden: VerificationReport = serde_json::from_slice(include_bytes!(
                        "../../../fixtures/conformance/sdk/verification-report-utf8.json"
                    ))
                    .expect("historical report");

                    let mut historical = golden;
                    historical.session = source.clone();
                    let old_path =
                        serde_json::to_string(&source).expect("historical PathBuf string");
                    let current =
                        encoded_json_member(&historical, "verification.json").expect("UTF8 report");
                    assert!(String::from_utf8(current)
                        .expect("UTF8")
                        .contains(&old_path));
                } else {
                    let mut changed: serde_json::Value =
                        serde_json::from_slice(&encoded).expect("metadata");

                    let decoded: PathValue =
                        serde_json::from_value(changed["session"].clone()).expect("exact path");
                    assert_eq!(
                        decoded.into_bytes().as_bytes(),
                        source.as_os_str().as_bytes()
                    );
                    let mut wrong_path = source.as_os_str().as_bytes().to_vec();
                    let last = wrong_path
                        .iter_mut()
                        .find(|b| return **b == 255)
                        .expect("nonUTF8 byte");
                    *last = 254; // Identical lossy display, different exact path.
                    changed["session"] =
                        serde_json::to_value(PathValue::from_bytes(&ByteString::new(wrong_path)))
                            .expect("path bytes");
                    let mut tampered = serde_json::to_vec_pretty(&changed).expect("changed member");
                    tampered.push(b'\n');
                    fs_err::write(&path, &tampered).expect("tamper");

                    let member = manifest
                        .members
                        .iter_mut()
                        .find(|m| return m.path == "verification.json")
                        .expect("member");
                    member.byte_length = u64::try_from(tampered.len()).expect("length");
                    member.blake3 = blake3::hash(&tampered).to_hex().to_string();

                    fs_err::write(
                        directory.join("manifest.json"),
                        serde_json::to_vec_pretty(&manifest).expect("rehashed manifest"),
                    )
                    .expect("manifest");
                    assert!(verify_profile(
                        &directory,
                        &BundleVerificationPolicy::new(&source, profile)
                    )
                    .is_err());
                    assert!(verify_archived_manifest(
                        &manifest,
                        &BundleVerificationPolicy::new(&source, profile)
                    )
                    .is_err());
                }

                assert_eq!(source_hashes(&source), before);
            }
        }
    }
}
