use crate::storage::{Error, Result};
use nix::libc::{O_CLOEXEC, O_NOFOLLOW};
use std::{
    fs::{File, OpenOptions, Permissions},
    io,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Component, Path, PathBuf},
};
use termtrace_model::{
    artifact::{
        bundle::{
            BundleManifest, BundleProfile, BUNDLE_FORMAT, BUNDLE_MANIFEST_NAME, BUNDLE_VERSION,
        },
        metadata::{ArtifactMember, ArtifactSensitivity},
    },
    value::ByteString,
};
use uuid::Uuid;

mod create;
mod tar;
mod verify;

pub use create::{BundleDraft, BundleMemberWriter};
pub use tar::{archive, verify_archive};
pub use verify::verify;

const MAX_BUNDLE_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_BUNDLE_MEMBERS: usize = 10_000;
const MAX_BUNDLE_TREE_ENTRIES: usize = MAX_BUNDLE_MEMBERS + 2;
const MAX_ARCHIVE_EXPANDED_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Caller-owned canonical identity expected in a bundle manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleIdentity {
    /// Selected evidence-disclosure profile.
    pub profile: BundleProfile,
    /// Immutable source session identifier.
    pub session_id: Uuid,
    /// Canonical source session path bytes.
    pub canonical_session: ByteString,
    /// Completion-bound final event hash.
    pub source_final_event_hash: String,
    /// Whether canonical raw evidence is included.
    pub includes_canonical_evidence: bool,
}

impl BundleIdentity {
    /// Build a manifest for a completed member inventory.
    #[must_use]
    pub fn manifest(&self, members: Vec<ArtifactMember>) -> BundleManifest {
        return BundleManifest {
            format: BUNDLE_FORMAT.to_owned(),
            version: BUNDLE_VERSION,
            profile: self.profile,
            session_id: self.session_id,
            canonical_session: self.canonical_session.clone(),
            source_final_event_hash: self.source_final_event_hash.clone(),
            includes_canonical_evidence: self.includes_canonical_evidence,
            members,
        };
    }

    pub(crate) fn mismatch_reason(&self, manifest: &BundleManifest) -> Option<&'static str> {
        if manifest.profile != self.profile
            || manifest.includes_canonical_evidence != self.includes_canonical_evidence
        {
            return Some("bundle manifest does not match the caller-owned source identity");
        }

        if manifest.session_id != self.session_id
            || manifest.canonical_session != self.canonical_session
            || manifest.source_final_event_hash != self.source_final_event_hash
        {
            return Some("bundle manifest does not match the canonical source identity");
        }
        return None;
    }
}

/// Metadata assigned to a new bundle payload member.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleMemberMetadata {
    /// Stable media type.
    pub media_type: String,
    /// Public schema identifier for structured content.
    pub schema_type: Option<String>,
    /// Content sensitivity classification.
    pub sensitivity: ArtifactSensitivity,
    /// Relationship of the member to its source evidence.
    pub relationship: String,
}

pub(crate) fn validate_relative_path(path: &Path) -> Result<String> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| return !matches!(component, Component::Normal(_)))
    {
        return Err(Error::invalid("bundle member path is unsafe"));
    }

    let path = path
        .to_str()
        .ok_or_else(|| return Error::invalid("bundle member paths must be valid UTF-8"))?;

    if path == BUNDLE_MANIFEST_NAME {
        return Err(Error::invalid(
            "manifest.json is reserved for the bundle manifest",
        ));
    }
    return Ok(path.to_owned());
}

pub(crate) fn create_private_directory(path: &Path) -> Result<()> {
    fs_err::create_dir(path).map_err(|source| return Error::filesystem(path, source))?;
    fs_err::set_permissions(path, Permissions::from_mode(0o700))
        .map_err(|source| return Error::filesystem(path, source))?;

    sync_parent(path)?;
    return Ok(());
}

pub(crate) fn create_private_parents(root: &Path, relative: &Path) -> Result<()> {
    let Some(parent) = relative.parent() else {
        return Ok(());
    };

    let mut current = root.to_path_buf();

    for component in parent.components() {
        let Component::Normal(name) = component else {
            return Err(Error::invalid("bundle member path is unsafe"));
        };
        current.push(name);

        match fs_err::create_dir(&current) {
            Ok(()) => {
                fs_err::set_permissions(&current, Permissions::from_mode(0o700))
                    .map_err(|source| return Error::filesystem(&current, source))?;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let metadata = fs_err::symlink_metadata(&current)
                    .map_err(|source| return Error::filesystem(&current, source))?;

                if !metadata.file_type().is_dir() {
                    return Err(Error::invalid("bundle member parent is not a directory"));
                }
            }
            Err(source) => return Err(Error::filesystem(&current, source)),
        }
    }

    return Ok(());
}

pub(crate) fn secure_file(path: &Path) -> Result<File> {
    return OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(O_CLOEXEC | O_NOFOLLOW)
        .open(path)
        .map_err(|source| {
            if source.kind() == io::ErrorKind::AlreadyExists {
                return Error::DestinationExists {
                    path: path.to_path_buf(),
                };
            }
            return Error::filesystem(path, source);
        });
}

pub(crate) fn open_regular_nofollow(path: &Path) -> Result<(File, u64)> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(O_CLOEXEC | O_NOFOLLOW)
        .open(path)
        .map_err(|source| return Error::filesystem(path, source))?;
    let metadata = file
        .metadata()
        .map_err(|source| return Error::filesystem(path, source))?;

    if !metadata.is_file() {
        return Err(Error::invalid(format!(
            "bundle member `{}` is not a regular file",
            path.display()
        )));
    }
    return Ok((file, metadata.len()));
}

/// Hash a bundle member without following symbolic links.
///
/// Returns its exact byte length and BLAKE3 digest.
///
/// # Errors
///
/// Returns an error when the path is not a readable regular file.
pub fn hash_regular_file(path: &Path) -> Result<(u64, String)> {
    let (mut file, size) = open_regular_nofollow(path)?;
    let mut hasher = blake3::Hasher::new();
    hasher
        .update_reader(&mut file)
        .map_err(|source| return Error::filesystem(path, source))?;

    return Ok((size, hasher.finalize().to_hex().to_string()));
}

pub(crate) fn sync_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| return Error::invalid("path has no parent"))?;

    File::open(parent)
        .and_then(|directory| return directory.sync_all())
        .map_err(|source| return Error::filesystem(parent, source))?;
    return Ok(());
}

pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| return directory.sync_all())
        .map_err(|source| return Error::filesystem(path, source))?;
    return Ok(());
}

pub(crate) fn collect_regular_files(
    root: &Path,
    directory: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in
        fs_err::read_dir(directory).map_err(|source| return Error::filesystem(directory, source))?
    {
        let entry = entry.map_err(|source| return Error::filesystem(directory, source))?;
        let file_type = entry
            .file_type()
            .map_err(|source| return Error::filesystem(entry.path(), source))?;

        if file_type.is_dir() {
            collect_regular_files(root, &entry.path(), files)?;
            continue;
        }

        let entry_metadata = entry
            .metadata()
            .map_err(|source| return Error::filesystem(entry.path(), source))?;

        if !entry_metadata.is_file() {
            return Err(Error::invalid("bundle contains a symlink or special file"));
        }

        files.push(
            entry
                .path()
                .strip_prefix(root)
                .map_err(|_| return Error::invalid("bundle path escaped its root"))?
                .to_path_buf(),
        );

        if files.len() > MAX_BUNDLE_TREE_ENTRIES {
            return Err(Error::invalid("bundle contains too many members"));
        }
    }

    return Ok(());
}

#[cfg(test)]
mod tests;
