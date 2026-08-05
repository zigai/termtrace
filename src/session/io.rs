use super::*;

/// Serialize a session manifest into its sole canonical on-disk form.
///
/// # Errors
///
/// Returns an error if the manifest cannot be serialized.
pub fn canonical_manifest_bytes(manifest: &SessionManifest) -> Result<Vec<u8>> {
    return serde_json::to_vec(manifest).map_err(|source| {
        return Error::InvalidJson {
            path: "session.json".into(),
            json_path: "$".to_owned(),
            source,
        };
    });
}

/// Return the BLAKE3 digest of exact session-manifest bytes.
pub fn manifest_hash(bytes: &[u8]) -> String {
    return blake3::hash(bytes).to_hex().to_string();
}

/// Read a session manifest from a session directory.
///
/// # Errors
///
/// Returns an error when the manifest cannot be read or decoded.
pub fn read_manifest(session_directory: &Path) -> Result<SessionManifest> {
    let path = session_directory.join("session.json");
    let bytes = fs_err::read(&path).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?;
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
    return serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let json_path = error.path().to_string();
        let source = error.into_inner();
        return Error::InvalidJson {
            path,
            json_path,
            source,
        };
    });
}

/// Read immutable completion facts from a completed session directory.
///
/// # Errors
///
/// Returns an error when the completion manifest cannot be read or decoded.
pub fn read_completion(session_directory: &Path) -> Result<SessionCompletion> {
    let path = session_directory.join("completion.json");
    let bytes = fs_err::read(&path).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?;
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);
    return serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        let json_path = error.path().to_string();
        let source = error.into_inner();
        return Error::InvalidJson {
            path,
            json_path,
            source,
        };
    });
}
