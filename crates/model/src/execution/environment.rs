//! Deterministic child-environment policy value contracts.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{error::ModelValidationError, value::ByteString};

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

impl EnvironmentSpec {
    /// Validate names, conflicts, values, and reserved host controls.
    ///
    /// # Errors
    ///
    /// Returns a value-free diagnostic when the specification is invalid.
    pub fn validate(&self) -> Result<(), ModelValidationError> {
        const MAX_VARIABLES: usize = 4096;
        const MAX_VALUE_BYTES: usize = 1024 * 1024;

        let count = self
            .set
            .len()
            .saturating_add(self.copy.len())
            .saturating_add(self.unset.len());
        if count > MAX_VARIABLES {
            return Err(ModelValidationError::TooManyEnvironmentEntries);
        }
        for name in self.set.keys().chain(&self.copy).chain(&self.unset) {
            validate_name(name)?;
            if reserved_name(name) {
                return Err(ModelValidationError::ControlledEnvironmentVariable {
                    name: name.clone(),
                });
            }
        }
        for (name, value) in &self.set {
            if value.as_bytes().contains(&0) {
                return Err(ModelValidationError::EnvironmentValueContainsNul {
                    name: name.clone(),
                });
            }
            if value.as_bytes().len() > MAX_VALUE_BYTES {
                return Err(ModelValidationError::EnvironmentValueTooLarge { name: name.clone() });
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
    ///
    /// # Errors
    ///
    /// Returns a value-free diagnostic when the policy is invalid.
    pub fn validate(&self) -> Result<(), ModelValidationError> {
        const MAX_VARIABLES: usize = 4096;

        let count = self
            .explicit_names
            .len()
            .saturating_add(self.copied_names.len())
            .saturating_add(self.unset_names.len());
        if count > MAX_VARIABLES {
            return Err(ModelValidationError::TooManyEnvironmentEntries);
        }
        for name in self
            .explicit_names
            .iter()
            .chain(&self.copied_names)
            .chain(&self.unset_names)
        {
            validate_name(name)?;
            if reserved_name(name) {
                return Err(ModelValidationError::ReservedEnvironmentVariable {
                    name: name.clone(),
                });
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
            return Err(ModelValidationError::InvalidEnvironmentDigest);
        }
        return Ok(());
    }
}

fn validate_name(name: &str) -> Result<(), ModelValidationError> {
    if name.is_empty()
        || name
            .as_bytes()
            .first()
            .is_none_or(|byte| return !byte.is_ascii_alphabetic() && *byte != b'_')
        || !name
            .as_bytes()
            .iter()
            .all(|byte| return byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        return Err(ModelValidationError::InvalidEnvironmentVariableName {
            name: name.to_owned(),
        });
    }
    return Ok(());
}

fn reserved_name(name: &str) -> bool {
    return matches!(
        name,
        "TERM" | "COLORTERM" | "NO_COLOR" | "FORCE_COLOR" | "CLICOLOR" | "CLICOLOR_FORCE"
    );
}

fn conflict(name: &str) -> ModelValidationError {
    return ModelValidationError::ConflictingEnvironmentOperations {
        name: name.to_owned(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let mut policy = ChildEnvironmentPolicy {
            effective_digest: "0".repeat(64),
            ..ChildEnvironmentPolicy::default()
        };
        policy.explicit_names.insert("TERM".to_owned());
        assert!(policy.validate().is_err());
    }
}
