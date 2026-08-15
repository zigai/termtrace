//! Deterministic child-environment resolution for the recording process.

use std::{
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
};

use termtrace_model::execution::environment::{
    ChildEnvironmentPolicy, ColorPolicy, EnvironmentBase, EnvironmentSpec,
};

use crate::{Error, Result};

/// Exact child environment and its persisted derivation policy.
#[derive(Clone, Debug)]
pub struct EffectiveEnvironment {
    variables: BTreeMap<OsString, OsString>,
    policy: ChildEnvironmentPolicy,
}

impl EffectiveEnvironment {
    /// Resolve one child environment without launching a process.
    ///
    /// # Errors
    ///
    /// Returns an error when the specification is invalid or requests a missing
    /// ambient variable.
    pub fn resolve(spec: &EnvironmentSpec, term: &str) -> Result<Self> {
        spec.validate()
            .map_err(|error| return environment_error(error.to_string()))?;
        let ambient: BTreeMap<OsString, OsString> = env::vars_os().collect();
        return Self::resolve_from_snapshot(spec, term, &ambient);
    }

    fn resolve_from_snapshot(
        spec: &EnvironmentSpec,
        term: &str,
        ambient: &BTreeMap<OsString, OsString>,
    ) -> Result<Self> {
        let mut variables = match spec.base {
            EnvironmentBase::Inherit => ambient.clone(),
            EnvironmentBase::Clear => BTreeMap::new(),
        };
        for name in &spec.unset {
            variables.remove(OsStr::new(name));
        }
        for name in &spec.copy {
            let value = ambient.get(OsStr::new(name)).ok_or_else(|| {
                return Error::TerminalOperation {
                    operation: "copy environment",
                    reason: format!("requested variable `{name}` is not set"),
                };
            })?;
            variables.insert(OsString::from(name), value.clone());
        }
        for (name, value) in &spec.set {
            variables.insert(
                OsString::from(name),
                OsString::from_vec(value.as_bytes().to_vec()),
            );
        }
        apply_color_policy(&mut variables, spec.color);
        variables.insert(OsString::from("TERM"), OsString::from(term));
        variables.insert(OsString::from("COLORTERM"), OsString::from("truecolor"));
        let effective_digest = digest_variables(&variables);
        return Ok(Self {
            variables,
            policy: ChildEnvironmentPolicy {
                base: spec.base,
                explicit_names: spec.set.keys().cloned().collect(),
                copied_names: spec.copy.clone(),
                unset_names: spec.unset.clone(),
                color: spec.color,
                effective_digest,
            },
        });
    }

    /// Exact variables supplied to the child process.
    #[must_use]
    pub fn variables(&self) -> &BTreeMap<OsString, OsString> {
        return &self.variables;
    }

    /// Persisted policy and digest for this exact environment.
    #[must_use]
    pub fn policy(&self) -> &ChildEnvironmentPolicy {
        return &self.policy;
    }
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
    force: &str,
    clicolor: &str,
    clicolor_force: &str,
) {
    variables.insert(OsString::from("FORCE_COLOR"), OsString::from(force));
    variables.insert(OsString::from("CLICOLOR"), OsString::from(clicolor));
    variables.insert(
        OsString::from("CLICOLOR_FORCE"),
        OsString::from(clicolor_force),
    );
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

fn environment_error(reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation: "validate environment",
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use std::{
        array,
        collections::{BTreeMap, BTreeSet},
        ffi::{OsStr, OsString},
        sync::Arc,
        thread,
    };

    use super::*;
    use termtrace_model::{execution::environment::EnvironmentBase, value::ByteString};

    #[test]
    fn clear_environment_applies_explicit_and_host_values() {
        let spec = EnvironmentSpec {
            base: EnvironmentBase::Clear,
            set: BTreeMap::from([("HOME".to_owned(), ByteString::from(b"/tmp/home".as_slice()))]),
            color: ColorPolicy::Never,
            ..EnvironmentSpec::default()
        };
        let effective = EffectiveEnvironment::resolve(&spec, "xterm-test").expect("resolve");
        assert_eq!(
            effective.variables().get(OsStr::new("HOME")),
            Some(&OsString::from("/tmp/home"))
        );
        assert_eq!(
            effective.variables().get(OsStr::new("TERM")),
            Some(&OsString::from("xterm-test"))
        );
        assert_eq!(
            effective.variables().get(OsStr::new("NO_COLOR")),
            Some(&OsString::from("1"))
        );
        assert!(effective.variables().get(OsStr::new("PATH")).is_none());
    }

    #[test]
    fn precedence_covers_inherit_overrides_removals_color_path_home_and_locale() {
        let ambient = BTreeMap::from([
            (OsString::from("PATH"), OsString::from("/host/bin")),
            (OsString::from("HOME"), OsString::from("/host/home")),
            (OsString::from("LC_ALL"), OsString::from("en_US.UTF-8")),
            (OsString::from("NO_COLOR"), OsString::from("host-value")),
        ]);
        let inherited_spec = EnvironmentSpec {
            set: BTreeMap::from([(
                "HOME".to_owned(),
                ByteString::from(b"/isolated/home".as_slice()),
            )]),
            unset: BTreeSet::from(["LC_ALL".to_owned()]),
            color: ColorPolicy::Always,
            ..EnvironmentSpec::default()
        };
        let inherited =
            EffectiveEnvironment::resolve_from_snapshot(&inherited_spec, "xterm-test", &ambient)
                .expect("inherit resolution");
        assert_eq!(
            inherited.variables().get(OsStr::new("PATH")),
            Some(&OsString::from("/host/bin"))
        );
        assert_eq!(
            inherited.variables().get(OsStr::new("HOME")),
            Some(&OsString::from("/isolated/home"))
        );
        assert!(inherited.variables().get(OsStr::new("LC_ALL")).is_none());
        assert!(inherited.variables().get(OsStr::new("NO_COLOR")).is_none());
        assert_eq!(
            inherited.variables().get(OsStr::new("FORCE_COLOR")),
            Some(&OsString::from("1"))
        );

        let cleared_spec = EnvironmentSpec {
            base: EnvironmentBase::Clear,
            copy: BTreeSet::from(["PATH".to_owned()]),
            color: ColorPolicy::Auto,
            ..EnvironmentSpec::default()
        };
        let cleared =
            EffectiveEnvironment::resolve_from_snapshot(&cleared_spec, "xterm-test", &ambient)
                .expect("clear resolution");
        assert_eq!(
            cleared.variables().get(OsStr::new("PATH")),
            Some(&OsString::from("/host/bin"))
        );
        assert!(cleared.variables().get(OsStr::new("HOME")).is_none());
        assert!(cleared.variables().get(OsStr::new("LC_ALL")).is_none());
        assert!(cleared.variables().get(OsStr::new("NO_COLOR")).is_none());
    }

    #[test]
    fn parallel_resolution_is_isolated_and_deterministic() {
        let spec = Arc::new(EnvironmentSpec {
            base: EnvironmentBase::Clear,
            set: BTreeMap::from([("FIXED".to_owned(), ByteString::from(b"value".as_slice()))]),
            ..EnvironmentSpec::default()
        });
        let workers: [_; 32] = array::from_fn(|_| {
            let spec = Arc::clone(&spec);
            return thread::spawn(move || {
                return EffectiveEnvironment::resolve(&spec, "xterm-test")
                    .expect("parallel resolution")
                    .policy()
                    .effective_digest
                    .clone();
            });
        });
        let digests = workers
            .into_iter()
            .map(|worker| return worker.join().expect("resolution worker"))
            .collect::<BTreeSet<_>>();
        assert_eq!(digests.len(), 1);
    }
}
