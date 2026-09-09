use crate::{
    auxiliary::bundle::{
        collect_regular_files, hash_regular_file, open_regular_nofollow, validate_relative_path,
        BundleIdentity, BUNDLE_FORMAT, BUNDLE_MANIFEST_NAME, BUNDLE_VERSION,
        MAX_BUNDLE_MANIFEST_BYTES, MAX_BUNDLE_MEMBERS,
    },
    storage::{is_lowercase_blake3, Error, Result},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read as _,
    path::Path,
};
use termtrace_model::{artifact::bundle::BundleManifest, artifact::metadata::ArtifactMember};

/// Verify a bundle directory and return its validated manifest.
///
/// # Errors
///
/// Returns an error when the root, manifest, expected identity, or member inventory is invalid.
pub fn verify(directory: &Path, expected: Option<&BundleIdentity>) -> Result<BundleManifest> {
    let root_metadata = fs_err::symlink_metadata(directory)
        .map_err(|source| return Error::filesystem(directory, source))?;

    if !root_metadata.file_type().is_dir() {
        return Err(Error::invalid("bundle root is not a real directory"));
    }

    let manifest_path = directory.join(BUNDLE_MANIFEST_NAME);
    let manifest_bytes = read_bounded_regular_file(&manifest_path, MAX_BUNDLE_MANIFEST_BYTES)?;
    let manifest: BundleManifest = serde_json::from_slice(&manifest_bytes).map_err(|source| {
        return Error::InvalidJson {
            path: manifest_path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    if manifest.format != BUNDLE_FORMAT || manifest.version != BUNDLE_VERSION {
        return Err(Error::invalid("unsupported bundle manifest"));
    }

    if manifest.members.len() > MAX_BUNDLE_MEMBERS {
        return Err(Error::invalid("bundle manifest has too many members"));
    }

    if let Some(reason) = expected.and_then(|identity| return identity.mismatch_reason(&manifest)) {
        return Err(Error::invalid(reason));
    }

    verify_members(directory, &manifest.members)?;
    return Ok(manifest);
}

fn verify_members(directory: &Path, expected: &[ArtifactMember]) -> Result<()> {
    let mut by_path: BTreeMap<&str, &ArtifactMember> = BTreeMap::new();

    for member in expected {
        validate_relative_path(Path::new(&member.path))?;

        if member.media_type.is_empty()
            || member.relationship.is_empty()
            || !is_lowercase_blake3(&member.blake3)
        {
            return Err(Error::invalid(format!(
                "bundle manifest member `{}` has invalid metadata",
                member.path
            )));
        }

        if by_path.insert(&member.path, member).is_some() {
            return Err(Error::invalid(
                "bundle manifest contains duplicate member paths",
            ));
        }
    }

    let mut actual_paths = Vec::new();
    collect_regular_files(directory, directory, &mut actual_paths)?;
    let actual: BTreeSet<String> = actual_paths
        .into_iter()
        .map(|path| {
            return path
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| return Error::invalid("bundle paths must be valid UTF-8"));
        })
        .collect::<Result<_>>()?;

    let mut expected_paths: BTreeSet<String> = by_path
        .keys()
        .map(|path| return (*path).to_owned())
        .collect();
    expected_paths.insert(BUNDLE_MANIFEST_NAME.to_owned());

    if actual != expected_paths {
        return Err(Error::invalid(
            "bundle payload members do not match their whole-file manifest",
        ));
    }

    for (path, member) in by_path {
        let (byte_length, hash) = hash_regular_file(&directory.join(path))?;

        if byte_length != member.byte_length || hash != member.blake3 {
            return Err(Error::invalid(format!(
                "bundle member `{path}` does not match its whole-file manifest"
            )));
        }
    }

    return Ok(());
}

pub(crate) fn read_bounded_regular_file(path: &Path, maximum_bytes: u64) -> Result<Vec<u8>> {
    let (mut file, size) = open_regular_nofollow(path)?;

    if size > maximum_bytes {
        return Err(Error::invalid(format!(
            "bundle member `{}` exceeds the {maximum_bytes} byte limit",
            path.display()
        )));
    }

    let mut bytes: Vec<u8> = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
    file.by_ref()
        .take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| return Error::filesystem(path, source))?;

    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum_bytes {
        return Err(Error::invalid(format!(
            "bundle member `{}` exceeds the {maximum_bytes} byte limit",
            path.display()
        )));
    }
    return Ok(bytes);
}
