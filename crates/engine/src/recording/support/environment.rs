use std::{
    fs::OpenOptions,
    io::Read as _,
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
};

use nix::{libc, unistd::geteuid};
use termtrace_model::execution::environment::EnvironmentSpec;

use super::terminal_error;
use crate::{Error, Result};

const MAX_SPEC_BYTES: u64 = 4 * 1024 * 1024;

/// Read and validate an owner-only, non-symlink environment specification.
///
/// The parser never includes file contents or environment values in diagnostics.
///
/// # Errors
///
/// Returns an error for an insecure file, invalid JSON, or invalid environment policy.
pub fn read_environment_spec(path: &Path) -> Result<EnvironmentSpec> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    let metadata = file.metadata().map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    if !metadata.is_file() {
        return Err(environment_error(
            "specification must be a regular non-symlink file",
        ));
    }
    if metadata.uid() != geteuid().as_raw() {
        return Err(environment_error(
            "specification must be owned by the current user",
        ));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(environment_error(
            "specification must be owner-only (mode 0600 or stricter)",
        ));
    }
    if metadata.len() > MAX_SPEC_BYTES {
        return Err(environment_error(
            "specification exceeds the 4 MiB safety limit",
        ));
    }
    let mut bytes: Vec<u8> = Vec::new();
    file.take(MAX_SPEC_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    return parse_environment_spec(&bytes, path);
}

/// Parse and validate bounded inline environment-spec bytes.
///
/// # Errors
///
/// Returns an error for oversized input, invalid JSON, or invalid environment policy.
pub fn parse_environment_spec(bytes: &[u8], source_path: &Path) -> Result<EnvironmentSpec> {
    if bytes.len() > MAX_SPEC_BYTES as usize {
        return Err(environment_error(
            "specification exceeds the 4 MiB safety limit",
        ));
    }
    let spec: EnvironmentSpec = serde_json::from_slice(bytes).map_err(|source| {
        return Error::InvalidJson {
            path: source_path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    spec.validate()
        .map_err(|error| return terminal_error("validate environment", error.to_string()))?;
    return Ok(spec);
}

fn environment_error(reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation: "read environment specification",
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use std::{
        fs::Permissions,
        os::unix::fs::{PermissionsExt as _, symlink},
    };

    use super::*;

    #[test]
    fn owner_only_spec_parser_rejects_permissions_and_symlinks() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let path = temporary.path().join("environment.json");
        fs_err::write(
            &path,
            serde_json::to_vec(&EnvironmentSpec::default()).expect("spec JSON"),
        )
        .expect("write spec");
        fs_err::set_permissions(&path, Permissions::from_mode(0o644)).expect("public mode");
        assert!(read_environment_spec(&path).is_err());
        fs_err::set_permissions(&path, Permissions::from_mode(0o600)).expect("private mode");
        assert_eq!(
            read_environment_spec(&path).expect("private spec"),
            EnvironmentSpec::default()
        );
        let link = temporary.path().join("environment-link.json");
        symlink(&path, &link).expect("symlink");
        assert!(read_environment_spec(&link).is_err());
    }
}
