use crate::{error::invalid, Error, Result};
use serde::de::Error as _;
use std::{collections::BTreeMap, path::Path};
use termtrace_model::{
    execution::environment::EnvironmentSpec,
    scenario::{Scenario, ScenarioEnvironment, ScenarioStep, SCENARIO_VERSION},
    value::ByteString,
};

/// Parse and validate one strict scenario JSON document.
pub(super) fn read(path: impl AsRef<Path>) -> Result<Scenario> {
    let path = path.as_ref();
    let encoded = fs_err::read(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

    let mut deserializer = serde_json::Deserializer::from_slice(&encoded);
    let scenario: Scenario =
        serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
            let json_path = error.path().to_string();
            let source = error.into_inner();
            let source = if json_path.starts_with("launch.environment") {
                serde_json::Error::custom(format!(
                    "invalid environment value at line {} column {}",
                    source.line(),
                    source.column(),
                ))
            } else {
                source
            };

            return Error::InvalidJson {
                path: path.to_path_buf(),
                json_path,
                source,
            };
        })?;

    validate(&scenario)?;

    return Ok(scenario);
}

/// Validate all bounded scenario document invariants.
pub(super) fn validate(scenario: &Scenario) -> Result<()> {
    if scenario.version != SCENARIO_VERSION {
        return Err(invalid(format!(
            "scenario version {} is unsupported; expected {SCENARIO_VERSION}",
            scenario.version
        )));
    }

    if scenario.name.trim().is_empty()
        || scenario.launch.command.is_empty()
        || scenario.steps.is_empty()
    {
        return Err(invalid(
            "scenario name, launch command, and steps must not be empty",
        ));
    }

    if scenario.launch.geometry.cols == 0 || scenario.launch.geometry.rows == 0 {
        return Err(invalid("scenario geometry must be greater than zero"));
    }

    if scenario.launch.timeout_ms == 0 || scenario.launch.output_byte_budget == Some(0) {
        return Err(invalid(
            "scenario timeout and configured resource budgets must be greater than zero",
        ));
    }

    to_environment_spec(&scenario.launch.environment)?
        .validate()
        .map_err(|error| return invalid(error.to_string()))?;

    for step in &scenario.steps {
        match step {
            ScenarioStep::WaitScreen(wait) => {
                if wait.contains.as_deref().is_none_or(str::is_empty)
                    && wait.absent.as_deref().is_none_or(str::is_empty)
                {
                    return Err(invalid("screen wait requires contains or absent text"));
                }

                if wait.contains.is_some() == wait.absent.is_some() {
                    return Err(invalid(
                        "screen wait requires exactly one of contains or absent",
                    ));
                }
            }

            ScenarioStep::Send(inputs) if inputs.is_empty() => {
                return Err(invalid("send step must contain at least one input"));
            }

            ScenarioStep::MatchScreenSnapshot(snapshot) => {
                validate_snapshot_name(&snapshot.name)?;
            }

            ScenarioStep::AssertTimeline(assertion) if assertion.between_markers.len() != 2 => {
                return Err(invalid(
                    "timeline assertion requires exactly two marker labels",
                ));
            }

            ScenarioStep::Send(_)
            | ScenarioStep::AssertTimeline(_)
            | ScenarioStep::Mark(_)
            | ScenarioStep::Stop {} => {}
        }
    }

    let mut combinations = 1_usize;
    for (key, values) in &scenario.matrix {
        if key.is_empty()
            || !key
                .bytes()
                .all(|byte| return byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || values.is_empty()
            || values.len() > 64
        {
            return Err(invalid(
                "matrix keys must be safe identifiers with 1 through 64 values",
            ));
        }

        combinations = combinations.saturating_mul(values.len());

        if combinations > 256 {
            return Err(invalid("scenario matrix exceeds 256 variants"));
        }
    }

    return Ok(());
}

/// Expand a bounded Cartesian matrix into independently named scenarios.
pub(super) fn expand_matrix(document: Scenario) -> Result<Vec<(String, Scenario)>> {
    if document.matrix.is_empty() {
        return Ok(vec![(String::new(), document)]);
    }

    let mut variants: Vec<BTreeMap<String, String>> = vec![BTreeMap::new()];

    for (key, values) in &document.matrix {
        let mut expanded: Vec<BTreeMap<String, String>> = Vec::new();
        for variant in &variants {
            for value in values {
                let mut next = variant.clone();
                next.insert(key.clone(), value.clone());
                expanded.push(next);
            }
        }

        variants = expanded;
    }

    return variants
        .into_iter()
        .map(|values| {
            let label = values
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<String>>()
                .join(",");

            let mut scenario = document.clone();
            scenario.matrix.clear();
            scenario.name = format!("{} [{label}]", scenario.name);

            for argument in &mut scenario.launch.command {
                *argument = substitute_matrix(argument, &values)?;
            }

            for step in &mut scenario.steps {
                if let ScenarioStep::MatchScreenSnapshot(snapshot) = step {
                    let suffix = blake3::hash(label.as_bytes())
                        .to_hex()
                        .chars()
                        .take(12)
                        .collect::<String>();
                    snapshot.name = format!("{}-{suffix}", snapshot.name);
                }
            }

            return Ok((label, scenario));
        })
        .collect();
}

fn substitute_matrix(value: &str, matrix: &BTreeMap<String, String>) -> Result<String> {
    let mut substituted = value.to_owned();
    for (key, replacement) in matrix {
        substituted = substituted.replace(&format!("{{{{{key}}}}}"), replacement);
    }

    if substituted.contains("{{") || substituted.contains("}}") {
        return Err(invalid(format!(
            "unresolved matrix token in command argument `{value}`"
        )));
    }
    return Ok(substituted);
}

pub(crate) fn to_environment_spec(environment: &ScenarioEnvironment) -> Result<EnvironmentSpec> {
    let mut set: BTreeMap<String, ByteString> = environment
        .set
        .iter()
        .map(|(name, value)| return (name.clone(), ByteString::new(value.as_bytes().to_vec())))
        .collect();

    for (name, value) in &environment.set_bytes {
        if set.insert(name.clone(), value.base64.clone()).is_some() {
            return Err(invalid(format!(
                "launch.environment.setBytes.{name} duplicates launch.environment.set.{name}"
            )));
        }
    }

    return Ok(EnvironmentSpec {
        base: environment.base,
        set,
        copy: environment.copy.clone(),
        unset: environment.unset.clone(),
        color: environment.color,
    });
}

pub(crate) fn validate_snapshot_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|byte| return byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(invalid(
            "snapshot names accept 1 through 128 ASCII letters, digits, hyphens, or underscores",
        ));
    }
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn strict_scenario_parses_typed_steps() {
        let scenario: Scenario = serde_json::from_value(serde_json::json!({
            "version": 2,
            "name": "ready flow",
            "launch": { "command": ["/bin/sh"], "geometry": { "cols": 90, "rows": 25 } },
            "steps": [
                { "waitScreen": { "contains": "Ready", "stableForMs": 20 } },
                { "send": [{ "text": "help" }, { "key": "enter" }] },
                { "mark": "help-visible" },
                { "stop": {} }
            ]
        }))
        .expect("scenario JSON");

        validate(&scenario).expect("valid scenario");
        assert_eq!(scenario.steps[1].kind(), "send");
    }

    #[test]
    fn scenario_version_one_is_rejected() {
        let scenario: Scenario = serde_json::from_value(serde_json::json!({
            "version": 1,
            "name": "obsolete",
            "launch": { "command": ["/bin/true"] },
            "steps": [{ "stop": {} }]
        }))
        .expect("version one scenario JSON");
        assert!(validate(&scenario).is_err());
    }

    #[test]
    fn scenario_rejects_ambiguous_screen_predicate() {
        let scenario: Scenario = serde_json::from_value(serde_json::json!({
            "version": 2,
            "name": "ambiguous",
            "launch": { "command": ["/bin/true"] },
            "steps": [{ "waitScreen": { "contains": "yes", "absent": "no" } }]
        }))
        .expect("scenario JSON");
        assert!(validate(&scenario).is_err());
    }

    #[test]
    fn scenario_matrix_expands_commands_and_isolates_snapshot_names() {
        let scenario: Scenario = serde_json::from_value(serde_json::json!({
            "version": 2,
            "name": "matrix",
            "matrix": { "theme": ["dark", "light"], "width": ["80", "120"] },
            "launch": { "command": ["/bin/echo", "{{theme}}", "{{width}}"] },
            "steps": [{ "matchScreenSnapshot": { "name": "matrix-final" } }]
        }))
        .expect("scenario JSON");

        let expanded = expand_matrix(scenario).expect("matrix expansion");
        assert_eq!(expanded.len(), 4);

        let first = expanded.first().expect("first matrix variant");
        assert_eq!(first.1.launch.command.get(1).expect("theme"), "dark");
        assert_eq!(first.1.launch.command.get(2).expect("width"), "80");

        let names = expanded
            .iter()
            .filter_map(|(_label, scenario)| {
                return match scenario.steps.first() {
                    Some(ScenarioStep::MatchScreenSnapshot(snapshot)) => {
                        Some(snapshot.name.clone())
                    }
                    _ => None,
                };
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(names.len(), 4);
    }

    #[test]
    fn scenario_environment_distinguishes_text_and_explicit_bytes() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let path = temporary.path().join("scenario.json");
        let json = serde_json::json!({
            "version": 2,
            "name": "environment",
            "launch": {
                "command": ["/bin/true"],
                "environment": {
                    "set": { "PI_SKIP_VERSION_CHECK": "1", "UNICODE": "λ" },
                    "setBytes": { "BINARY": { "base64": "AQI=" } }
                }
            },
            "steps": [{ "stop": {} }]
        });

        fs_err::write(&path, serde_json::to_vec(&json).expect("scenario JSON"))
            .expect("write scenario");

        let scenario = read(&path).expect("read scenario document");
        let spec =
            to_environment_spec(&scenario.launch.environment).expect("convert to environment spec");
        assert_eq!(
            spec.set
                .get("PI_SKIP_VERSION_CHECK")
                .map(|b| return b.as_bytes()),
            Some(b"1".as_slice())
        );
        assert_eq!(
            spec.set.get("UNICODE").map(|b| return b.as_bytes()),
            Some("λ".as_bytes())
        );
        assert_eq!(
            spec.set.get("BINARY").map(|b| return b.as_bytes()),
            Some(&[1, 2][..])
        );

        let duplicate_json = serde_json::json!({
            "version": 2,
            "name": "dup",
            "launch": {
                "command": ["/bin/true"],
                "environment": {
                    "set": { "COLLISION": "text" },
                    "setBytes": { "COLLISION": { "base64": "AQI=" } }
                }
            },
            "steps": [{ "stop": {} }]
        });

        let dup_path = temporary.path().join("dup.json");
        fs_err::write(
            &dup_path,
            serde_json::to_vec(&duplicate_json).expect("dup JSON"),
        )
        .expect("write dup");

        let err = read(&dup_path).expect_err("duplicate key must fail");
        assert!(err
            .to_string()
            .contains("duplicates launch.environment.set.COLLISION"));
    }

    #[test]
    fn scenario_environment_type_errors_do_not_reveal_values() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let path = temporary.path().join("scenario.json");
        fs_err::write(
            &path,
            br#"{"version":2,"name":"invalid environment","launch":{"command":["/bin/true"],"environment":{"set":{"TOKEN":100000000}}},"steps":[{"stop":{}}]}"#,
        )
        .expect("write scenario");

        let error = read(&path).expect_err("invalid environment value");
        assert!(!error.to_string().contains("100000000"));
        let Error::InvalidJson {
            json_path, source, ..
        } = error
        else {
            panic!("expected JSON diagnostic");
        };
        assert_eq!(json_path, "launch.environment.set.TOKEN");
        assert!(!source.to_string().contains("100000000"));
    }

    #[test]
    fn scenario_read_reports_environment_value_path() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let path = temporary.path().join("invalid-scenario.json");
        fs_err::write(
            &path,
            br#"{
                "version": 2,
                "name": "invalid environment",
                "launch": {
                    "command": ["/bin/true"],
                    "environment": { "set": { "PI_SKIP_VERSION_CHECK": 1 } }
                },
                "steps": [{ "stop": {} }]
            }"#,
        )
        .expect("write scenario");

        let error = read(&path).expect_err("invalid environment value");
        let diagnostic = error.to_string();

        assert!(matches!(
            error,
            Error::InvalidJson { json_path, .. }
                if json_path == "launch.environment.set.PI_SKIP_VERSION_CHECK"
        ));
        assert!(diagnostic.contains("launch.environment.set.PI_SKIP_VERSION_CHECK"));
    }
}
