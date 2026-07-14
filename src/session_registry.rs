//! Owner-only named-session registry backed by canonical session identity.

use std::{
    env,
    ffi::OsString,
    fs::{File, OpenOptions, Permissions},
    io::Read as _,
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use nix::{libc::O_NOFOLLOW, unistd::getuid};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{sync_parent_directory, write_secure},
    control::CONTROL_PROTOCOL_VERSION,
    event::ByteString,
    session::{SESSION_MANIFEST_FORMAT, SESSION_MANIFEST_VERSION, read_manifest},
};

/// Registry-entry format identifier.
pub const REGISTRY_ENTRY_FORMAT: &str = "termtrace-session-registry-entry";
/// Registry-entry schema version.
pub const REGISTRY_ENTRY_VERSION: u32 = 1;
const REGISTRY_DIRECTORY: &str = "registry";

/// One friendly name bound to an immutable canonical session identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RegistryEntry {
    /// Stable entry format identifier.
    pub format: String,
    /// Entry schema version.
    pub version: u32,
    /// Validated friendly alias.
    pub alias: String,
    /// Exact canonical session-directory path bytes.
    pub canonical_session: ByteString,
    /// Immutable session ID read from `session.json` during registration.
    pub session_id: Uuid,
    /// Recorder PID when a supervisor supplied one.
    pub recorder_pid: Option<u32>,
    /// Linux process start-time ticks paired with `recorder_pid` against PID reuse.
    pub recorder_process_start_ticks: Option<u64>,
    /// Control protocol expected by the registering build.
    pub control_protocol_version: u32,
    /// Registry publication time from the local system clock.
    pub registered_unix_ns: u128,
}

impl RegistryEntry {
    /// Reconstruct the exact canonical session path.
    pub fn session_path(&self) -> PathBuf {
        return PathBuf::from(OsString::from_vec(
            self.canonical_session.as_bytes().to_vec(),
        ));
    }
}

/// Secure storage boundary for named session aliases.
#[derive(Clone, Debug)]
pub struct SessionRegistry {
    directory: PathBuf,
}

impl SessionRegistry {
    /// Open or securely initialize the default per-user registry.
    ///
    /// `TERMTRACE_STATE_DIR` overrides the base directory. Otherwise the base
    /// is `$XDG_STATE_HOME/termtrace`, falling back to `$HOME/.local/state/termtrace`.
    ///
    /// # Errors
    ///
    /// Returns an error when no state root can be selected or an existing root
    /// is not an owner-only real directory.
    pub fn open_default() -> Result<Self> {
        let root = default_state_root()?;
        return Self::open(root);
    }

    /// Open or securely initialize a registry beneath an explicit state root.
    ///
    /// # Errors
    ///
    /// Returns an error when directories cannot be created or do not have
    /// owner-only ownership and permissions.
    pub fn open(state_root: impl AsRef<Path>) -> Result<Self> {
        let state_root = state_root.as_ref();
        ensure_owner_directory(state_root)?;
        let directory = state_root.join(REGISTRY_DIRECTORY);
        ensure_owner_directory(&directory)?;
        return Ok(Self { directory });
    }

    /// Register a new alias without replacing an existing binding.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid alias, insecure or unreadable session,
    /// schema/identity mismatch, or existing alias.
    pub fn register(&self, alias: &str, session: &Path) -> Result<RegistryEntry> {
        validate_alias(alias)?;
        validate_owner_directory(session)?;
        let canonical = session.canonicalize().map_err(|source| {
            return Error::Filesystem {
                path: session.to_path_buf(),
                source,
            };
        })?;
        validate_owner_directory(&canonical)?;
        let manifest = read_manifest(&canonical)?;
        if manifest.format != SESSION_MANIFEST_FORMAT
            || manifest.version != SESSION_MANIFEST_VERSION
        {
            return Err(registry_error(
                "register session alias",
                "session manifest format is unsupported by this registry build",
            ));
        }
        let registered_unix_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| return Error::ClockBeforeEpoch)?
            .as_nanos();
        let entry = RegistryEntry {
            format: REGISTRY_ENTRY_FORMAT.to_owned(),
            version: REGISTRY_ENTRY_VERSION,
            alias: alias.to_owned(),
            canonical_session: ByteString::from(canonical.as_os_str().as_bytes()),
            session_id: manifest.session_id,
            recorder_pid: None,
            recorder_process_start_ticks: None,
            control_protocol_version: CONTROL_PROTOCOL_VERSION,
            registered_unix_ns,
        };
        let encoded = serde_json::to_vec_pretty(&entry).map_err(|source| {
            return Error::InvalidJson {
                path: self.entry_path(alias),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        write_secure(&self.entry_path(alias), &encoded)?;
        return Ok(entry);
    }

    /// Resolve and validate an alias against its canonical session manifest.
    ///
    /// # Errors
    ///
    /// Returns an error when the entry is missing, insecure, malformed,
    /// incompatible, or no longer names the registered immutable session ID.
    pub fn resolve(&self, alias: &str) -> Result<RegistryEntry> {
        validate_alias(alias)?;
        let path = self.entry_path(alias);
        let entry = read_entry_nofollow(&path)?;
        if entry.format != REGISTRY_ENTRY_FORMAT
            || entry.version != REGISTRY_ENTRY_VERSION
            || entry.alias != alias
            || entry.control_protocol_version != CONTROL_PROTOCOL_VERSION
            || entry.recorder_pid.is_some() != entry.recorder_process_start_ticks.is_some()
        {
            return Err(registry_error(
                "resolve registry entry",
                "entry format, alias, protocol, or process identity is incompatible",
            ));
        }
        let session = entry.session_path();
        validate_owner_directory(&session)?;
        let canonical = session.canonicalize().map_err(|source| {
            return Error::Filesystem {
                path: session.clone(),
                source,
            };
        })?;
        if canonical.as_os_str().as_bytes() != entry.canonical_session.as_bytes() {
            return Err(registry_error(
                "resolve registry entry",
                "canonical session path changed after registration",
            ));
        }
        let manifest = read_manifest(&canonical)?;
        if manifest.session_id != entry.session_id {
            return Err(registry_error(
                "resolve registry entry",
                "session manifest identity no longer matches the registry binding",
            ));
        }
        return Ok(entry);
    }

    fn entry_path(&self, alias: &str) -> PathBuf {
        return self.directory.join(format!("{alias}.json"));
    }
}

fn default_state_root() -> Result<PathBuf> {
    if let Some(path) = env::var_os("TERMTRACE_STATE_DIR") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(path).join("termtrace"));
    }
    let home = env::var_os("HOME").ok_or_else(|| {
        return registry_error(
            "select registry state root",
            "TERMTRACE_STATE_DIR, XDG_STATE_HOME, and HOME are unset",
        );
    })?;
    return Ok(PathBuf::from(home).join(".local/state/termtrace"));
}

fn validate_alias(alias: &str) -> Result<()> {
    if alias.is_empty()
        || alias.len() > 64
        || !alias.bytes().all(|byte| {
            return byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_';
        })
        || !alias
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
    {
        return Err(registry_error(
            "validate session alias",
            "alias must be 1-64 ASCII letters, digits, '-' or '_', beginning with a letter or digit",
        ));
    }
    return Ok(());
}

fn ensure_owner_directory(path: &Path) -> Result<()> {
    if path.exists() {
        return validate_owner_directory(path);
    }
    fs_err::create_dir_all(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    fs_err::set_permissions(path, Permissions::from_mode(0o700)).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    sync_parent_directory(path)?;
    return validate_owner_directory(path);
}

fn validate_owner_directory(path: &Path) -> Result<()> {
    let metadata = fs_err::symlink_metadata(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    let mode = metadata.mode() & 0o777;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_dir()
        || metadata.uid() != getuid().as_raw()
        || mode != 0o700
    {
        return Err(Error::InsecureArtifactDirectory {
            path: path.to_path_buf(),
            owner_uid: metadata.uid(),
            mode,
        });
    }
    return Ok(());
}

#[allow(
    clippy::verbose_file_reads,
    reason = "the already-opened O_NOFOLLOW descriptor must be read without reopening its path"
)]
fn read_entry_nofollow(path: &Path) -> Result<RegistryEntry> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(O_NOFOLLOW)
        .open(path)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    validate_entry_file(path, &file)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    return serde_json::from_slice(&bytes).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    });
}

fn validate_entry_file(path: &Path, file: &File) -> Result<()> {
    let metadata = file.metadata().map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    if metadata.mode() & 0o170_000 != 0o100_000
        || metadata.uid() != getuid().as_raw()
        || metadata.mode() & 0o777 != 0o600
    {
        return Err(registry_error(
            "validate registry entry",
            "entry must be an owner-owned regular file with mode 0600",
        ));
    }
    return Ok(());
}

fn registry_error(operation: &'static str, reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation,
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    #[test]
    fn alias_validation_rejects_paths_and_ambiguous_prefixes() {
        for alias in ["", "-bad", "../escape", "has space", "é"] {
            assert!(validate_alias(alias).is_err(), "accepted alias {alias:?}");
        }
        assert!(validate_alias("build-01_main").is_ok());
    }

    #[test]
    fn registry_rejects_symlink_entries_without_following_them() {
        let temporary = TempDir::new().expect("temporary directory");
        let registry = SessionRegistry::open(temporary.path().join("state")).expect("registry");
        let target = temporary.path().join("target");
        fs_err::write(&target, b"{}").expect("target");
        symlink(&target, registry.entry_path("demo")).expect("symlink");
        assert!(registry.resolve("demo").is_err());
    }
}
