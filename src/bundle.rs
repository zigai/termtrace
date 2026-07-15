//! Deterministic, sensitivity-labeled evidence handoff directories.

use std::{
    fs::FileType,
    os::unix::fs::FileTypeExt as _,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{
        ArtifactMember, ArtifactSensitivity, collect_artifact_members, commit_directory,
        create_private_directory, sync_directory, write_secure,
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
