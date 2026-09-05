use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    fs::File,
    os::unix::{ffi::OsStrExt as _, fs::PermissionsExt as _},
    path::{Path, PathBuf},
};

use nix::unistd::{getegid, geteuid, getgid, getuid};
use rustix::system::uname;
use termtrace_model::{
    execution::provenance::{
        ContentIdentityStatus, EnvironmentProvenance, ExecutionProvenance, FileProvenance,
        KernelProvenance, ProvenanceCapture, ProvenancePolicy, ProvenancePolicyKind,
        ProvenanceSensitivity, RecorderProvenance, RuntimeProvenance,
    },
    value::ByteString,
};

/// Exact environment passed to the recorded child process.
pub type EffectiveEnvironment = BTreeMap<OsString, OsString>;

/// Recorder build identifiers bound into execution provenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecorderBuildIdentity<'identity> {
    /// Recorder package version.
    pub version: &'identity str,
    /// Source revision of the recorder build.
    pub source_revision: &'identity str,
    /// Pinned semantic-engine revision.
    pub semantic_engine_revision: &'identity str,
}

/// Collect execution provenance under the selected disclosure policy.
pub fn collect_execution_provenance(
    policy: &ProvenancePolicy,
    child_environment: &EffectiveEnvironment,
    command: &[OsString],
    working_directory: &Path,
    recorder: RecorderBuildIdentity<'_>,
) -> ExecutionProvenance {
    let rules = effective_rules(policy, child_environment);
    let environment = rules
        .into_iter()
        .map(|(name, capture)| {
            let value = child_environment
                .get(OsStr::new(&name))
                .map(|value| return value.as_bytes());
            let sensitivity = if is_public_configuration(&name) {
                ProvenanceSensitivity::PublicConfiguration
            } else {
                ProvenanceSensitivity::PotentiallySensitive
            };
            return (
                name,
                represent_environment_value(capture, value, sensitivity),
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
        runtime: collect_runtime_provenance(should_hash_files(policy.kind), recorder),
    };
}

fn collect_runtime_provenance(
    hash_files: bool,
    recorder: RecorderBuildIdentity<'_>,
) -> RuntimeProvenance {
    let kernel = uname();
    let current_executable = env::current_exe().ok();
    let binary = collect_file_provenance(
        current_executable
            .as_deref()
            .map_or_else(|| return OsStr::new(""), Path::as_os_str),
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
            system: kernel.sysname().to_string_lossy().into_owned(),
            release: kernel.release().to_string_lossy().into_owned(),
            version: kernel.version().to_string_lossy().into_owned(),
            machine: kernel.machine().to_string_lossy().into_owned(),
        },
        recorder: RecorderProvenance {
            version: recorder.version.to_owned(),
            source_revision: recorder.source_revision.to_owned(),
            semantic_engine_revision: recorder.semantic_engine_revision.to_owned(),
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
    let search = child_environment.get(OsStr::new("PATH"))?;
    for directory in env::split_paths(search) {
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
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
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
    let resolved_path = Path::new(OsStr::from_bytes(resolved.as_bytes()));
    let name = resolved_path.file_name()?.to_string_lossy();
    let interpreter = matches!(
        name.as_ref(),
        "sh" | "bash" | "dash" | "zsh" | "node" | "ruby" | "perl"
    ) || name.starts_with("python");
    if !interpreter {
        return None;
    }
    let argument = command.get(1)?;
    if argument.as_bytes().starts_with(b"-") {
        return None;
    }
    let path = Path::new(argument);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        working_directory.join(path)
    };
    if !candidate.metadata().ok()?.is_file() {
        return None;
    }
    return Some((argument.clone(), candidate.canonicalize().ok()?));
}

fn hash_file(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(&mut file).ok()?;
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
            for name in child_environment
                .keys()
                .filter_map(|name| return name.to_str())
            {
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
    let value_hash = match capture {
        ProvenanceCapture::Value | ProvenanceCapture::Hash => {
            value.map(|bytes| return blake3::hash(bytes).to_hex().to_string())
        }
        ProvenanceCapture::Name | ProvenanceCapture::NotCaptured => None,
    };
    let captured_value = if capture == ProvenanceCapture::Value {
        value.map(ByteString::from)
    } else {
        None
    };
    return EnvironmentProvenance {
        capture,
        present: value.is_some(),
        value: captured_value,
        value_hash,
        sensitivity,
    };
}

fn is_public_configuration(name: &str) -> bool {
    return matches!(name, "TERM" | "COLORTERM" | "LANG" | "TZ" | "LC_ALL")
        || name.starts_with("LC_");
}

#[cfg(test)]
mod tests {
    use std::{fs::Permissions, os::unix::fs::PermissionsExt as _};

    use super::*;

    fn recorder() -> RecorderBuildIdentity<'static> {
        return RecorderBuildIdentity {
            version: "test",
            source_revision: "test",
            semantic_engine_revision: "test",
        };
    }

    #[test]
    fn minimal_policy_records_terminal_values_without_locale_values() {
        let policy = ProvenancePolicy::default();
        let environment = BTreeMap::from([(OsString::from("TERM"), OsString::from("xterm-test"))]);
        let collected = collect_execution_provenance(
            &policy,
            &environment,
            &[OsString::from("/bin/true")],
            Path::new("/"),
            recorder(),
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
        let environment =
            BTreeMap::from([(OsString::from("PRIVATE_TOKEN"), OsString::from("secret"))]);
        let collected = collect_execution_provenance(
            &policy,
            &environment,
            &[OsString::from("/bin/true")],
            Path::new("/"),
            recorder(),
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
        let environment =
            BTreeMap::from([(OsString::from("PRIVATE_TOKEN"), OsString::from("secret"))]);
        let collected = collect_execution_provenance(
            &policy,
            &environment,
            &[OsString::from("/bin/true")],
            Path::new("/"),
            recorder(),
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
        let environment = BTreeMap::new();
        let collected = collect_execution_provenance(
            &policy,
            &environment,
            &[OsString::from("/bin/sh"), OsString::from("fixture.sh")],
            temporary.path(),
            recorder(),
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
        let environment = BTreeMap::from([(
            OsString::from("PATH"),
            temporary.path().as_os_str().to_os_string(),
        )]);
        let collected = collect_execution_provenance(
            &ProvenancePolicy::default(),
            &environment,
            &[OsString::from("child-only-command")],
            Path::new("/"),
            recorder(),
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
