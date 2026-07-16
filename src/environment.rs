//! Deterministic child-environment policy and resolution.

use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    ffi::{OsStr, OsString},
    fs::OpenOptions,
    io::Read as _,
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _},
    path::Path,
};

use nix::{libc, unistd::geteuid};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{Error, Result, event::ByteString};

/// Source environment used before explicit child overrides are applied.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentBase {
    /// Snapshot and inherit the recorder process environment.
    #[default]
    Inherit,
    /// Begin with an empty environment.
    Clear,
}

/// Deterministic common color-variable policy applied after caller overrides.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ColorPolicy {
    /// Preserve inherited color variables and add none to a clear environment.
    #[default]
    Auto,
    /// Request colored output through common de-facto variables.
    Always,
    /// Request plain output through common de-facto variables.
    Never,
}

/// Caller-owned child-environment configuration.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentSpec {
    /// Whether resolution begins with the recorder's ambient environment.
    #[serde(default)]
    pub base: EnvironmentBase,
    /// Exact UTF-8-named values supplied by the caller.
    #[serde(default)]
    pub set: BTreeMap<String, ByteString>,
    /// Names copied from the recorder environment without placing values in arguments.
    #[serde(default)]
    pub copy: BTreeSet<String>,
    /// Names removed from an inherited environment.
    #[serde(default)]
    pub unset: BTreeSet<String>,
    /// Deterministic color-variable behavior.
    #[serde(default)]
    pub color: ColorPolicy,
}

/// Value-free environment declaration retained in canonical evidence.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChildEnvironmentPolicy {
    /// Base environment policy.
    pub base: EnvironmentBase,
    /// Names whose values were supplied explicitly.
    pub explicit_names: BTreeSet<String>,
    /// Names copied from the recorder environment.
    pub copied_names: BTreeSet<String>,
    /// Names explicitly removed before host policy was applied.
    pub unset_names: BTreeSet<String>,
    /// Deterministic color-variable behavior.
    pub color: ColorPolicy,
    /// BLAKE3 commitment to the exact ordered name/value byte pairs passed to the child.
    pub effective_digest: String,
}

/// One immutable effective child environment used by spawn and provenance.
#[derive(Clone, Debug)]
pub struct EffectiveEnvironment {
    variables: BTreeMap<OsString, OsString>,
    policy: ChildEnvironmentPolicy,
}

impl EnvironmentSpec {
    /// Resolve and validate this policy from one ambient snapshot.
    pub fn resolve(&self, term: &str) -> Result<EffectiveEnvironment> {
        self.validate()?;
        let ambient = env::vars_os().collect::<BTreeMap<_, _>>();
        return self.resolve_from_snapshot(term, &ambient);
    }

    fn resolve_from_snapshot(
        &self,
        term: &str,
        ambient: &BTreeMap<OsString, OsString>,
    ) -> Result<EffectiveEnvironment> {
        let mut variables = match self.base {
            EnvironmentBase::Inherit => ambient.clone(),
            EnvironmentBase::Clear => BTreeMap::new(),
        };
        for name in &self.unset {
            variables.remove(OsStr::new(name));
        }
        for name in &self.copy {
            let value = ambient.get(OsStr::new(name)).ok_or_else(|| {
                return environment_error(
                    "copy environment",
                    format!("requested variable `{name}` is not set"),
                );
            })?;
            variables.insert(OsString::from(name), value.clone());
        }
        for (name, value) in &self.set {
            variables.insert(
                OsString::from(name),
                OsString::from_vec(value.as_bytes().to_vec()),
            );
        }
        apply_color_policy(&mut variables, self.color);
        variables.insert(OsString::from("TERM"), OsString::from(term));
        variables.insert(OsString::from("COLORTERM"), OsString::from("truecolor"));
        let effective_digest = digest_variables(&variables);
        return Ok(EffectiveEnvironment {
            variables,
            policy: ChildEnvironmentPolicy {
                base: self.base,
                explicit_names: self.set.keys().cloned().collect(),
                copied_names: self.copy.clone(),
                unset_names: self.unset.clone(),
                color: self.color,
                effective_digest,
            },
        });
    }

    /// Validate names, conflicts, values, and reserved host controls.
    pub fn validate(&self) -> Result<()> {
        const MAX_VARIABLES: usize = 4096;
        const MAX_VALUE_BYTES: usize = 1024 * 1024;
        let count = self
            .set
            .len()
            .saturating_add(self.copy.len())
            .saturating_add(self.unset.len());
        if count > MAX_VARIABLES {
            return Err(environment_error(
                "validate environment",
                "environment policy exceeds 4096 entries",
            ));
        }
        for name in self.set.keys().chain(&self.copy).chain(&self.unset) {
            validate_name(name)?;
            if reserved_name(name) {
                return Err(environment_error(
                    "validate environment",
                    format!("`{name}` is controlled by --term or --color"),
                ));
            }
        }
        for (name, value) in &self.set {
            if value.as_bytes().contains(&0) {
                return Err(environment_error(
                    "validate environment",
                    format!("value for `{name}` contains NUL"),
                ));
            }
            if value.as_bytes().len() > MAX_VALUE_BYTES {
                return Err(environment_error(
                    "validate environment",
                    format!("value for `{name}` exceeds 1 MiB"),
                ));
            }
        }
        for name in &self.copy {
            if self.set.contains_key(name) || self.unset.contains(name) {
                return Err(conflict(name));
            }
        }
        for name in &self.unset {
            if self.set.contains_key(name) {
                return Err(conflict(name));
            }
        }
        return Ok(());
    }
}

impl ChildEnvironmentPolicy {
    /// Validate the value-free policy retained in canonical evidence.
    pub fn validate(&self) -> Result<()> {
        const MAX_VARIABLES: usize = 4096;
        let count = self
            .explicit_names
            .len()
            .saturating_add(self.copied_names.len())
            .saturating_add(self.unset_names.len());
        if count > MAX_VARIABLES {
            return Err(environment_error(
                "validate child environment policy",
                "environment policy exceeds 4096 entries",
            ));
        }
        for name in self
            .explicit_names
            .iter()
            .chain(&self.copied_names)
            .chain(&self.unset_names)
        {
            validate_name(name)?;
            if reserved_name(name) {
                return Err(environment_error(
                    "validate child environment policy",
                    format!("`{name}` is a reserved terminal or color variable"),
                ));
            }
        }
        if let Some(name) = self
            .explicit_names
            .intersection(&self.copied_names)
            .chain(self.explicit_names.intersection(&self.unset_names))
            .chain(self.copied_names.intersection(&self.unset_names))
            .next()
        {
            return Err(conflict(name));
        }
        if self.effective_digest.len() != 64
            || !self
                .effective_digest
                .bytes()
                .all(|byte| return byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(environment_error(
                "validate child environment policy",
                "effective environment digest must be lowercase BLAKE3 hex",
            ));
        }
        return Ok(());
    }
}

fn digest_variables(variables: &BTreeMap<OsString, OsString>) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"termtrace-child-environment-v1\0");
    for (name, value) in variables {
        let name = name.as_os_str().as_bytes();
        let value = value.as_os_str().as_bytes();
        hasher.update(&u64::try_from(name.len()).unwrap_or(u64::MAX).to_le_bytes());
        hasher.update(name);
        hasher.update(&u64::try_from(value.len()).unwrap_or(u64::MAX).to_le_bytes());
        hasher.update(value);
    }
    return hasher.finalize().to_hex().to_string();
}

impl EffectiveEnvironment {
    /// Exact ordered variables passed to the child.
    pub fn variables(&self) -> &BTreeMap<OsString, OsString> {
        return &self.variables;
    }

    /// Value-free policy retained in the session manifest.
    pub fn policy(&self) -> &ChildEnvironmentPolicy {
        return &self.policy;
    }

    /// Return one exact value by UTF-8 variable name.
    pub fn get(&self, name: &str) -> Option<&OsStr> {
        return self
            .variables
            .get(OsStr::new(name))
            .map(OsString::as_os_str);
    }

    /// Iterate UTF-8 variable names for provenance rule discovery.
    pub fn utf8_names(&self) -> impl Iterator<Item = &str> {
        return self
            .variables
            .keys()
            .filter_map(|name| return name.to_str());
    }
}

/// Read and validate an owner-only, non-symlink environment specification.
///
/// The parser never includes file contents or environment values in diagnostics.
pub fn read_environment_spec(path: &Path) -> Result<EnvironmentSpec> {
    const MAX_SPEC_BYTES: u64 = 4 * 1024 * 1024;

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
            "read environment specification",
            "specification must be a regular non-symlink file",
        ));
    }
    if metadata.uid() != geteuid().as_raw() {
        return Err(environment_error(
            "read environment specification",
            "specification must be owned by the current user",
        ));
    }
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(environment_error(
            "read environment specification",
            "specification must be owner-only (mode 0600 or stricter)",
        ));
    }
    if metadata.len() > MAX_SPEC_BYTES {
        return Err(environment_error(
            "read environment specification",
            "specification exceeds the 4 MiB safety limit",
        ));
    }
    let mut bytes = Vec::new();
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

/// Parse and validate bounded environment-spec bytes without echoing their contents.
pub fn parse_environment_spec(bytes: &[u8], source_path: &Path) -> Result<EnvironmentSpec> {
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(environment_error(
            "read environment specification",
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
    spec.validate()?;
    return Ok(spec);
}

fn apply_color_policy(variables: &mut BTreeMap<OsString, OsString>, policy: ColorPolicy) {
    match policy {
        ColorPolicy::Auto => {}
        ColorPolicy::Always => {
            variables.remove(OsStr::new("NO_COLOR"));
            set_color_variables(variables, "1", "1", "1");
        }
        ColorPolicy::Never => {
            variables.insert(OsString::from("NO_COLOR"), OsString::from("1"));
            set_color_variables(variables, "0", "0", "0");
        }
    }
}

fn set_color_variables(
    variables: &mut BTreeMap<OsString, OsString>,
    force_color: &str,
    clicolor: &str,
    clicolor_force: &str,
) {
    variables.insert(OsString::from("FORCE_COLOR"), OsString::from(force_color));
    variables.insert(OsString::from("CLICOLOR"), OsString::from(clicolor));
    variables.insert(
        OsString::from("CLICOLOR_FORCE"),
        OsString::from(clicolor_force),
    );
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.as_bytes().first().is_none_or(|byte| {
            return !byte.is_ascii_alphabetic() && *byte != b'_';
        })
        || !name
            .as_bytes()
            .iter()
            .all(|byte| return byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return Err(environment_error(
            "validate environment",
            format!("invalid environment variable name `{name}`"),
        ));
    }
    return Ok(());
}

fn reserved_name(name: &str) -> bool {
    return matches!(
        name,
        "TERM" | "COLORTERM" | "NO_COLOR" | "FORCE_COLOR" | "CLICOLOR" | "CLICOLOR_FORCE"
    );
}

fn conflict(name: &str) -> Error {
    return environment_error(
        "validate environment",
        format!("environment variable `{name}` has conflicting operations"),
    );
}

fn environment_error(operation: &'static str, reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation,
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use std::{
        fs::Permissions,
        os::unix::fs::{PermissionsExt as _, symlink},
        sync::Arc,
        thread,
    };

    use super::*;

    #[test]
    fn clear_environment_applies_explicit_and_host_values() {
        let spec = EnvironmentSpec {
            base: EnvironmentBase::Clear,
            set: BTreeMap::from([("HOME".to_owned(), ByteString::from(b"/tmp/home".as_slice()))]),
            color: ColorPolicy::Never,
            ..EnvironmentSpec::default()
        };
        let effective = spec.resolve("xterm-test").expect("resolve");
        assert_eq!(effective.get("HOME"), Some(OsStr::new("/tmp/home")));
        assert_eq!(effective.get("TERM"), Some(OsStr::new("xterm-test")));
        assert_eq!(effective.get("NO_COLOR"), Some(OsStr::new("1")));
        assert!(effective.get("PATH").is_none());
    }

    #[test]
    fn precedence_covers_inherit_overrides_removals_color_path_home_and_locale() {
        let ambient = BTreeMap::from([
            (OsString::from("PATH"), OsString::from("/host/bin")),
            (OsString::from("HOME"), OsString::from("/host/home")),
            (OsString::from("LC_ALL"), OsString::from("en_US.UTF-8")),
            (OsString::from("NO_COLOR"), OsString::from("host-value")),
        ]);
        let inherited = EnvironmentSpec {
            set: BTreeMap::from([(
                "HOME".to_owned(),
                ByteString::from(b"/isolated/home".as_slice()),
            )]),
            unset: BTreeSet::from(["LC_ALL".to_owned()]),
            color: ColorPolicy::Always,
            ..EnvironmentSpec::default()
        }
        .resolve_from_snapshot("xterm-test", &ambient)
        .expect("inherit resolution");
        assert_eq!(inherited.get("PATH"), Some(OsStr::new("/host/bin")));
        assert_eq!(inherited.get("HOME"), Some(OsStr::new("/isolated/home")));
        assert!(inherited.get("LC_ALL").is_none());
        assert!(inherited.get("NO_COLOR").is_none());
        assert_eq!(inherited.get("FORCE_COLOR"), Some(OsStr::new("1")));

        let cleared = EnvironmentSpec {
            base: EnvironmentBase::Clear,
            copy: BTreeSet::from(["PATH".to_owned()]),
            color: ColorPolicy::Auto,
            ..EnvironmentSpec::default()
        }
        .resolve_from_snapshot("xterm-test", &ambient)
        .expect("clear resolution");
        assert_eq!(cleared.get("PATH"), Some(OsStr::new("/host/bin")));
        assert!(cleared.get("HOME").is_none());
        assert!(cleared.get("LC_ALL").is_none());
        assert!(cleared.get("NO_COLOR").is_none());
    }

    #[test]
    fn rejects_reserved_and_conflicting_names() {
        let reserved = EnvironmentSpec {
            set: BTreeMap::from([("TERM".to_owned(), ByteString::from(b"bad".as_slice()))]),
            ..EnvironmentSpec::default()
        };
        assert!(reserved.validate().is_err());
        let conflict = EnvironmentSpec {
            set: BTreeMap::from([("HOME".to_owned(), ByteString::from(b"x".as_slice()))]),
            unset: BTreeSet::from(["HOME".to_owned()]),
            ..EnvironmentSpec::default()
        };
        assert!(conflict.validate().is_err());

        let mut policy = EnvironmentSpec::default()
            .resolve("xterm-test")
            .expect("policy source")
            .policy()
            .clone();
        policy.explicit_names.insert("TERM".to_owned());
        assert!(policy.validate().is_err());
    }

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

    #[test]
    #[allow(
        clippy::needless_collect,
        reason = "all workers must be spawned before any join to exercise concurrency"
    )]
    fn parallel_resolution_is_isolated_and_deterministic() {
        let spec = Arc::new(EnvironmentSpec {
            base: EnvironmentBase::Clear,
            set: BTreeMap::from([("FIXED".to_owned(), ByteString::from(b"value".as_slice()))]),
            ..EnvironmentSpec::default()
        });
        let workers = (0..32)
            .map(|_| {
                let spec = Arc::clone(&spec);
                return thread::spawn(move || {
                    return spec
                        .resolve("xterm-test")
                        .expect("parallel resolution")
                        .policy()
                        .effective_digest
                        .clone();
                });
            })
            .collect::<Vec<_>>();
        let digests = workers
            .into_iter()
            .map(|worker| return worker.join().expect("resolution worker"))
            .collect::<BTreeSet<_>>();
        assert_eq!(digests.len(), 1);
    }
}
