use crate::storage::{Error, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};
use termtrace_model::execution::session::{SessionCompletion, SessionManifest};

/// Serialize a session manifest using its canonical compact JSON representation.
///
/// # Errors
///
/// Returns an error when the manifest cannot be serialized.
pub fn canonical_manifest_bytes(manifest: &SessionManifest) -> Result<Vec<u8>> {
    return serde_json::to_vec(manifest).map_err(|source| {
        return Error::InvalidJson {
            path: "session.json".into(),
            json_path: "$".to_owned(),
            source,
        };
    });
}

/// Compute the BLAKE3 identity of exact canonical manifest bytes.
pub fn manifest_hash(bytes: &[u8]) -> String {
    return blake3::hash(bytes).to_hex().to_string();
}

/// Read a session manifest with path-aware JSON diagnostics.
///
/// # Errors
///
/// Returns an error when `session.json` cannot be read or decoded.
pub fn read_manifest(session_directory: &Path) -> Result<SessionManifest> {
    return read_json(session_directory.join("session.json"));
}

/// Read a session completion record with path-aware JSON diagnostics.
///
/// # Errors
///
/// Returns an error when `completion.json` cannot be read or decoded.
pub fn read_completion(session_directory: &Path) -> Result<SessionCompletion> {
    return read_json(session_directory.join("completion.json"));
}

fn read_json<T>(path: PathBuf) -> Result<T>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let bytes = fs::read(&path).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?;

    let mut deserializer = serde_json::Deserializer::from_slice(&bytes);

    return serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
        return Error::InvalidJson {
            path,
            json_path: error.path().to_string(),
            source: error.into_inner(),
        };
    });
}
