//! Sensitivity-aware execution provenance collection policies.

use std::{collections::BTreeMap, env, os::unix::ffi::OsStrExt as _};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::event::ByteString;

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
}

/// Collect environment provenance after applying recorder-owned child overrides.
pub fn collect_execution_provenance(
    policy: &ProvenancePolicy,
    child_overrides: &BTreeMap<String, String>,
) -> ExecutionProvenance {
    let rules = effective_rules(policy);
    let environment = rules
        .into_iter()
        .map(|(name, capture)| {
            let value = child_overrides
                .get(&name)
                .map(|value| return value.as_bytes().to_vec())
                .or_else(|| {
                    return env::var_os(&name)
                        .map(|value| return value.as_os_str().as_bytes().to_vec());
                });
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
    return ExecutionProvenance {
        policy: policy.clone(),
        environment,
    };
}

fn effective_rules(policy: &ProvenancePolicy) -> BTreeMap<String, ProvenanceCapture> {
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
            for (name, _) in env::vars_os() {
                if let Some(name) = name.to_str()
                    && name.starts_with("LC_")
                {
                    rules.insert(name.to_owned(), ProvenanceCapture::Value);
                }
            }
            return rules;
        }
        ProvenancePolicyKind::Custom => return policy.environment.clone(),
    }
}

fn represent_environment_value(
    capture: ProvenanceCapture,
    value: Option<&[u8]>,
    sensitivity: ProvenanceSensitivity,
) -> EnvironmentProvenance {
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
    use super::*;

    #[test]
    fn minimal_policy_records_terminal_values_without_locale_values() {
        let policy = ProvenancePolicy::default();
        let overrides = BTreeMap::from([
            ("COLORTERM".to_owned(), "truecolor".to_owned()),
            ("TERM".to_owned(), "xterm-test".to_owned()),
        ]);
        let collected = collect_execution_provenance(&policy, &overrides);
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
        let overrides = BTreeMap::from([("PRIVATE_TOKEN".to_owned(), "secret".to_owned())]);
        let collected = collect_execution_provenance(&policy, &overrides);
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
}
