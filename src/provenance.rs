//! Sensitivity-aware execution provenance collection policies.

use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    fs::File,
    io::{BufReader, Read as _},
    os::unix::ffi::OsStrExt as _,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use nix::unistd::{getegid, geteuid, getgid, getuid};
use rustix::system::uname;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{environment::EffectiveEnvironment, event::ByteString, replay::GHOSTTY_REVISION};

/// Built-in execution-provenance collection policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProvenancePolicyKind {
    /// Retain only essential terminal values and inventory common locale context.
    #[default]
    Minimal,
    /// Retain values needed to recreate common locale and timezone behavior.
    Reproducible,
    /// Apply only caller-supplied environment capture rules.
    Custom,
}

/// How one selected provenance value is represented.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceCapture {
    /// Retain the exact value and its hash.
    Value,
    /// Retain only a value hash for later equality checks.
    Hash,
    /// Retain only the variable name and whether it was set.
    Name,
    /// Explicitly document that the selected value was not captured.
    NotCaptured,
}

/// Sensitivity classification for one provenance item.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceSensitivity {
    /// Conventional terminal, locale, or timezone configuration.
    PublicConfiguration,
    /// Caller-selected data that may contain private or proprietary content.
    PotentiallySensitive,
}

/// Explicit immutable provenance policy.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProvenancePolicy {
    /// Built-in policy behavior.
    pub kind: ProvenancePolicyKind,
    /// Custom environment rules; required to be empty for built-in policies.
    pub environment: BTreeMap<String, ProvenanceCapture>,
}

/// Captured or inventoried environment provenance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentProvenance {
    /// Representation selected by the policy.
    pub capture: ProvenanceCapture,
    /// Whether the variable existed in the effective child environment.
    pub present: bool,
    /// Exact bytes when capture is `value`.
    pub value: Option<ByteString>,
    /// BLAKE3 of the exact value bytes for `value` or `hash` capture.
    pub value_hash: Option<String>,
    /// Sensitivity classification used by exporters and reviewers.
    pub sensitivity: ProvenanceSensitivity,
}

/// Policy plus its immutable environment inventory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecutionProvenance {
    /// Policy that selected the inventory.
    pub policy: ProvenancePolicy,
    /// Selected effective child-environment facts, ordered by name.
    pub environment: BTreeMap<String, EnvironmentProvenance>,
    /// Resolved executable identity.
    pub executable: FileProvenance,
    /// Interpreter-invoked script when it can be identified without guessing.
    pub invoked_file: Option<FileProvenance>,
    /// Recorder, kernel, and local identity facts.
    pub runtime: RuntimeProvenance,
}

/// Whether content identity was retained for a resolved file.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContentIdentityStatus {
    /// The policy did not request file content hashing.
    NotRequested,
    /// BLAKE3 was computed from the complete file.
    Captured,
    /// The path could not be resolved to an executable or regular file.
    Unresolved,
    /// A resolved file could not be read completely.
    Unavailable,
}

/// Exact requested path plus resolved content identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileProvenance {
    /// Bytes supplied by the caller or discovered as an interpreter argument.
    pub requested: ByteString,
    /// Canonical resolved path bytes when resolution succeeded.
    pub resolved: Option<ByteString>,
    /// Complete-file BLAKE3 under a policy that requests content identity.
    pub content_hash: Option<String>,
    /// Explicit content identity outcome.
    pub content_identity: ContentIdentityStatus,
}

/// Runtime kernel identity returned by the host.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KernelProvenance {
    /// Operating-system name.
    pub system: String,
    /// Kernel release.
    pub release: String,
    /// Kernel build/version string.
    pub version: String,
    /// Runtime machine architecture.
    pub machine: String,
}

/// Recorder build and binary identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecorderProvenance {
    /// Package version.
    pub version: String,
    /// Source revision embedded at build time.
    pub source_revision: String,
    /// Pinned semantic-engine revision.
    pub semantic_engine_revision: String,
    /// Running recorder binary identity.
    pub binary: FileProvenance,
}

/// Recorder process identity and host context.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProvenance {
    /// Real user ID.
    pub uid: u32,
    /// Effective user ID.
    pub effective_uid: u32,
    /// Real group ID.
    pub gid: u32,
    /// Effective group ID.
    pub effective_gid: u32,
    /// Whether the effective identity is privileged root.
    pub privileged: bool,
    /// Process umask read without modifying it, when the host exposes it.
    pub umask: Option<u32>,
    /// Runtime kernel identity.
    pub kernel: KernelProvenance,
    /// Recorder build/binary identity.
    pub recorder: RecorderProvenance,
}

/// Collect provenance from the exact immutable environment passed to the child.
pub fn collect_execution_provenance(
    policy: &ProvenancePolicy,
    child_environment: &EffectiveEnvironment,
    command: &[OsString],
    working_directory: &Path,
) -> ExecutionProvenance {
    let rules = effective_rules(policy, child_environment);
    let environment = rules
        .into_iter()
        .map(|(name, capture)| {
            let value = child_environment
                .get(&name)
                .map(|value| return value.as_bytes().to_vec());
            let sensitivity = if is_public_configuration(&name) {
                ProvenanceSensitivity::PublicConfiguration
            } else {
                ProvenanceSensitivity::PotentiallySensitive
            };
            return (
                name,
                represent_environment_value(capture, value.as_deref(), sensitivity),
            );
        })
        .collect();
    let executable = command.first().map_or_else(
        || return unresolved_file(OsStr::new("")),
        |requested| {
            return collect_file_provenance(
                requested.as_os_str(),
                resolve_executable(requested, working_directory, child_environment),
                should_hash_files(policy.kind),
            );
        },
    );
    let invoked_file = discover_invoked_file(command, &executable, working_directory).map(
        |(requested, resolved)| {
            return collect_file_provenance(
                requested.as_os_str(),
                Some(resolved),
                should_hash_files(policy.kind),
            );
        },
    );
    return ExecutionProvenance {
        policy: policy.clone(),
        environment,
        executable,
        invoked_file,
        runtime: collect_runtime_provenance(should_hash_files(policy.kind)),
    };
}

fn collect_runtime_provenance(hash_files: bool) -> RuntimeProvenance {
    let uname = uname();
    let current_executable = env::current_exe().ok();
    let binary = collect_file_provenance(
        current_executable
            .as_deref()
            .map_or_else(|| return OsStr::new(""), |path| return path.as_os_str()),
        current_executable.clone(),
        hash_files,
    );
    let uid = getuid().as_raw();
    let effective_uid = geteuid().as_raw();
    return RuntimeProvenance {
        uid,
        effective_uid,
        gid: getgid().as_raw(),
        effective_gid: getegid().as_raw(),
        privileged: effective_uid == 0,
        umask: read_process_umask(),
        kernel: KernelProvenance {
            system: uname.sysname().to_string_lossy().into_owned(),
            release: uname.release().to_string_lossy().into_owned(),
            version: uname.version().to_string_lossy().into_owned(),
            machine: uname.machine().to_string_lossy().into_owned(),
        },
        recorder: RecorderProvenance {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            source_revision: env!("TERMTRACE_BUILD_REVISION").to_owned(),
            semantic_engine_revision: GHOSTTY_REVISION.to_owned(),
            binary,
        },
    };
}

fn collect_file_provenance(
    requested: &OsStr,
    resolved: Option<PathBuf>,
    hash_requested: bool,
) -> FileProvenance {
    let Some(resolved) = resolved else {
        return unresolved_file(requested);
    };
    let (content_hash, content_identity) = if hash_requested {
        hash_file(&resolved).map_or((None, ContentIdentityStatus::Unavailable), |hash| {
            return (Some(hash), ContentIdentityStatus::Captured);
        })
    } else {
        (None, ContentIdentityStatus::NotRequested)
    };
    return FileProvenance {
        requested: ByteString::from(requested.as_bytes()),
        resolved: Some(ByteString::from(resolved.as_os_str().as_bytes())),
        content_hash,
        content_identity,
    };
}

fn unresolved_file(requested: &OsStr) -> FileProvenance {
    return FileProvenance {
        requested: ByteString::from(requested.as_bytes()),
        resolved: None,
        content_hash: None,
        content_identity: ContentIdentityStatus::Unresolved,
    };
}

fn should_hash_files(policy: ProvenancePolicyKind) -> bool {
    return matches!(policy, ProvenancePolicyKind::Reproducible);
}

fn resolve_executable(
    requested: &OsStr,
    working_directory: &Path,
    child_environment: &EffectiveEnvironment,
) -> Option<PathBuf> {
    let requested_path = Path::new(requested);
    if requested.as_bytes().contains(&b'/') {
        let candidate = if requested_path.is_absolute() {
            requested_path.to_path_buf()
        } else {
            working_directory.join(requested_path)
        };
        return canonical_executable(candidate);
    }
    let search = child_environment.get("PATH")?;
    for directory in env::split_paths(&search) {
        let directory = if directory.as_os_str().is_empty() {
            working_directory.to_path_buf()
        } else if directory.is_absolute() {
            directory
        } else {
            working_directory.join(directory)
        };
        if let Some(path) = canonical_executable(directory.join(requested_path)) {
            return Some(path);
        }
    }
    return None;
}

fn canonical_executable(path: PathBuf) -> Option<PathBuf> {
    let metadata = path.metadata().ok()?;
    if metadata.mode() & 0o170_000 != 0o100_000 || metadata.permissions().mode() & 0o111 == 0 {
        return None;
    }
    return path.canonicalize().ok();
}

fn discover_invoked_file(
    command: &[OsString],
    executable: &FileProvenance,
    working_directory: &Path,
) -> Option<(OsString, PathBuf)> {
    let resolved = executable.resolved.as_ref()?;
    let name = Path::new(OsStr::from_bytes(resolved.as_bytes()))
        .file_name()?
        .to_string_lossy();
    let interpreter = name == "sh"
        || name == "bash"
        || name == "dash"
        || name == "zsh"
        || name == "node"
        || name == "ruby"
        || name == "perl"
        || name.starts_with("python");
    if !interpreter {
        return None;
    }
    let argument = command.get(1)?;
    if argument.as_os_str().as_bytes().starts_with(b"-") {
        return None;
    }
    let path = Path::new(argument);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_directory.join(path)
    };
    let metadata = candidate.metadata().ok()?;
    if metadata.mode() & 0o170_000 != 0o100_000 {
        return None;
    }
    return Some((argument.clone(), candidate.canonicalize().ok()?));
}

fn hash_file(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; 64 * 1024].into_boxed_slice();
    loop {
        let count = reader.read(&mut buffer).ok()?;
        if count == 0 {
            break;
        }
        hasher.update(buffer.get(..count)?);
    }
    return Some(hasher.finalize().to_hex().to_string());
}

fn read_process_umask() -> Option<u32> {
    let status = fs_err::read_to_string("/proc/self/status").ok()?;
    let value = status
        .lines()
        .find_map(|line| return line.strip_prefix("Umask:").map(str::trim))?;
    return u32::from_str_radix(value, 8).ok();
}

fn effective_rules(
    policy: &ProvenancePolicy,
    child_environment: &EffectiveEnvironment,
) -> BTreeMap<String, ProvenanceCapture> {
    match policy.kind {
        ProvenancePolicyKind::Minimal => {
            return BTreeMap::from([
                ("COLORTERM".to_owned(), ProvenanceCapture::Value),
                ("LANG".to_owned(), ProvenanceCapture::Name),
                ("LC_ALL".to_owned(), ProvenanceCapture::Name),
                ("TERM".to_owned(), ProvenanceCapture::Value),
                ("TZ".to_owned(), ProvenanceCapture::Name),
            ]);
        }
        ProvenancePolicyKind::Reproducible => {
            let mut rules = BTreeMap::from([
                ("COLORTERM".to_owned(), ProvenanceCapture::Value),
                ("LANG".to_owned(), ProvenanceCapture::Value),
                ("LC_ALL".to_owned(), ProvenanceCapture::Value),
                ("TERM".to_owned(), ProvenanceCapture::Value),
                ("TZ".to_owned(), ProvenanceCapture::Value),
            ]);
            for name in child_environment.utf8_names() {
                if name.starts_with("LC_") {
                    rules.insert(name.to_owned(), ProvenanceCapture::Value);
                }
            }
            return rules;
        }
        ProvenancePolicyKind::Custom => return policy.environment.clone(),
    }
}

fn represent_environment_value(
    requested_capture: ProvenanceCapture,
    value: Option<&[u8]>,
    sensitivity: ProvenanceSensitivity,
) -> EnvironmentProvenance {
    let capture = if sensitivity == ProvenanceSensitivity::PotentiallySensitive
        && requested_capture == ProvenanceCapture::Value
    {
        ProvenanceCapture::Hash
    } else {
        requested_capture
    };
    let present = value.is_some();
    let value_hash = match capture {
        ProvenanceCapture::Value | ProvenanceCapture::Hash => {
            value.map(|bytes| return blake3::hash(bytes).to_hex().to_string())
        }
        ProvenanceCapture::Name | ProvenanceCapture::NotCaptured => None,
    };
    let value = if matches!(capture, ProvenanceCapture::Value) {
        value.map(ByteString::from)
    } else {
        None
    };
    return EnvironmentProvenance {
        capture,
        present,
        value,
        value_hash,
        sensitivity,
    };
}

fn is_public_configuration(name: &str) -> bool {
    return name == "TERM"
        || name == "COLORTERM"
        || name == "LANG"
        || name == "TZ"
        || name == "LC_ALL"
        || name.starts_with("LC_");
}

#[cfg(test)]
mod tests {
    use std::{fs::Permissions, os::unix::fs::PermissionsExt as _};

    use super::*;
    use crate::environment::{EnvironmentBase, EnvironmentSpec};

    #[test]
    fn minimal_policy_records_terminal_values_without_locale_values() {
        let policy = ProvenancePolicy::default();
        let environment = EnvironmentSpec {
            base: EnvironmentBase::Clear,
            ..EnvironmentSpec::default()
        }
        .resolve("xterm-test")
        .expect("environment");
        let collected = collect_execution_provenance(
            &policy,
            &environment,
            &[OsString::from("/bin/true")],
            Path::new("/"),
        );
        assert_eq!(
            collected.environment.get("TERM").and_then(|item| {
                return item.value.as_ref().map(|value| return value.as_bytes());
            }),
            Some(b"xterm-test".as_slice())
        );
        assert_eq!(
            collected
                .environment
                .get("LANG")
                .map(|item| return item.capture),
            Some(ProvenanceCapture::Name)
        );
        assert!(
            collected
                .environment
                .get("LANG")
                .is_none_or(|item| return item.value.is_none() && item.value_hash.is_none())
        );
    }

    #[test]
    fn custom_policy_hashes_sensitive_values_without_retaining_them() {
        let policy = ProvenancePolicy {
            kind: ProvenancePolicyKind::Custom,
            environment: BTreeMap::from([("PRIVATE_TOKEN".to_owned(), ProvenanceCapture::Hash)]),
        };
        let environment = EnvironmentSpec {
            base: EnvironmentBase::Clear,
            set: BTreeMap::from([(
                "PRIVATE_TOKEN".to_owned(),
                ByteString::from(b"secret".as_slice()),
            )]),
            ..EnvironmentSpec::default()
        }
        .resolve("xterm-test")
        .expect("environment");
        let collected = collect_execution_provenance(
            &policy,
            &environment,
            &[OsString::from("/bin/true")],
            Path::new("/"),
        );
        let item = collected.environment.get("PRIVATE_TOKEN").expect("item");
        assert_eq!(item.capture, ProvenanceCapture::Hash);
        assert_eq!(
            item.sensitivity,
            ProvenanceSensitivity::PotentiallySensitive
        );
        assert!(item.value.is_none());
        assert_eq!(
            item.value_hash.as_deref(),
            Some(blake3::hash(b"secret").to_hex().as_str())
        );
    }

    #[test]
    fn sensitive_value_capture_is_forced_to_a_hash() {
        let policy = ProvenancePolicy {
            kind: ProvenancePolicyKind::Custom,
            environment: BTreeMap::from([("PRIVATE_TOKEN".to_owned(), ProvenanceCapture::Value)]),
        };
        let environment = EnvironmentSpec {
            base: EnvironmentBase::Clear,
            set: BTreeMap::from([(
                "PRIVATE_TOKEN".to_owned(),
                ByteString::from(b"secret".as_slice()),
            )]),
            ..EnvironmentSpec::default()
        }
        .resolve("xterm-test")
        .expect("environment");
        let collected = collect_execution_provenance(
            &policy,
            &environment,
            &[OsString::from("/bin/true")],
            Path::new("/"),
        );
        let item = collected.environment.get("PRIVATE_TOKEN").expect("item");
        assert_eq!(item.capture, ProvenanceCapture::Hash);
        assert!(item.value.is_none());
        assert!(item.value_hash.is_some());
    }

    #[test]
    fn reproducible_policy_hashes_resolved_executable_and_script() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let script = temporary.path().join("fixture.sh");
        fs_err::write(&script, b"exit 0\n").expect("script");
        let policy = ProvenancePolicy {
            kind: ProvenancePolicyKind::Reproducible,
            environment: BTreeMap::new(),
        };
        let environment = EnvironmentSpec::default()
            .resolve("xterm-test")
            .expect("environment");
        let collected = collect_execution_provenance(
            &policy,
            &environment,
            &[OsString::from("/bin/sh"), OsString::from("fixture.sh")],
            temporary.path(),
        );
        assert_eq!(
            collected.executable.content_identity,
            ContentIdentityStatus::Captured
        );
        let invoked = collected.invoked_file.expect("invoked script");
        assert_eq!(invoked.content_identity, ContentIdentityStatus::Captured);
        assert_eq!(
            invoked.content_hash.as_deref(),
            Some(blake3::hash(b"exit 0\n").to_hex().as_str())
        );
    }

    #[test]
    fn executable_resolution_uses_only_the_effective_child_path() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let executable = temporary.path().join("child-only-command");
        fs_err::write(&executable, b"#!/bin/sh\nexit 0\n").expect("fixture executable");
        fs_err::set_permissions(&executable, Permissions::from_mode(0o700)).expect("fixture mode");
        let environment = EnvironmentSpec {
            base: EnvironmentBase::Clear,
            set: BTreeMap::from([(
                "PATH".to_owned(),
                ByteString::from(temporary.path().as_os_str().as_bytes()),
            )]),
            ..EnvironmentSpec::default()
        }
        .resolve("xterm-test")
        .expect("environment");
        let collected = collect_execution_provenance(
            &ProvenancePolicy::default(),
            &environment,
            &[OsString::from("child-only-command")],
            Path::new("/"),
        );
        assert_eq!(
            collected
                .executable
                .resolved
                .as_ref()
                .map(ByteString::as_bytes),
            Some(executable.as_os_str().as_bytes())
        );
    }
}
