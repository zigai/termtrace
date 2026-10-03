#![expect(missing_docs, reason = "integration test target has no public API")]

use cargo_metadata::{Metadata, MetadataCommand};
use std::{collections::BTreeMap, error::Error, path::Path};
use tempfile::TempDir;

fn allowed_dependencies(package: &str) -> Option<&'static [&'static str]> {
    return match package {
        "termtrace" => Some(&[
            "termtrace-control",
            "termtrace-engine",
            "termtrace-evidence",
            "termtrace-export",
            "termtrace-media",
            "termtrace-model",
            "termtrace-process",
            "termtrace-render",
            "termtrace-scenario",
            "termtrace-schema",
            "termtrace-sdk",
            "termtrace-service",
            "termtrace-terminal",
            "termtrace-tmux",
        ]),
        "termtrace-control" => Some(&["termtrace-model", "termtrace-process"]),
        "termtrace-engine" => Some(&[
            "termtrace-control",
            "termtrace-evidence",
            "termtrace-model",
            "termtrace-process",
            "termtrace-terminal",
        ]),
        "termtrace-evidence" => Some(&["termtrace-model"]),
        "termtrace-export" => Some(&[
            "termtrace-engine",
            "termtrace-evidence",
            "termtrace-model",
            "termtrace-render",
            "termtrace-tmux",
        ]),
        "termtrace-media" => Some(&[
            "termtrace-control",
            "termtrace-engine",
            "termtrace-evidence",
            "termtrace-model",
            "termtrace-process",
            "termtrace-render",
            "termtrace-terminal",
        ]),
        "termtrace-model" => Some(&[]),
        "termtrace-process" => Some(&[]),
        "termtrace-render" => Some(&["termtrace-model", "termtrace-terminal"]),
        "termtrace-node" => Some(&["termtrace-model", "termtrace-sdk"]),
        "termtrace-scenario" => Some(&[
            "termtrace-engine",
            "termtrace-evidence",
            "termtrace-model",
            "termtrace-render",
            "termtrace-service",
        ]),
        "termtrace-python" => Some(&["termtrace-model", "termtrace-sdk"]),
        "termtrace-schema" => Some(&["termtrace-model", "termtrace-tmux"]),
        "termtrace-sdk" => Some(&[
            "termtrace-control",
            "termtrace-media",
            "termtrace-model",
            "termtrace-process",
            "termtrace-scenario",
            "termtrace-service",
        ]),
        "termtrace-service" => Some(&[
            "termtrace-control",
            "termtrace-engine",
            "termtrace-evidence",
            "termtrace-export",
            "termtrace-media",
            "termtrace-model",
            "termtrace-process",
            "termtrace-render",
            "termtrace-terminal",
            "termtrace-tmux",
        ]),
        "termtrace-terminal" => Some(&["termtrace-model"]),
        "termtrace-tmux" => Some(&[
            "termtrace-control",
            "termtrace-evidence",
            "termtrace-model",
            "termtrace-process",
        ]),
        "termtrace-test-fixture" => {
            Some(&["termtrace-engine", "termtrace-evidence", "termtrace-model"])
        }
        _ => None,
    };
}

fn violations(metadata: &Metadata) -> Result<Vec<String>, Box<dyn Error>> {
    let packages = metadata.workspace_packages();
    let mut packages_by_path = BTreeMap::new();

    for package in &packages {
        let directory = package
            .manifest_path
            .as_std_path()
            .parent()
            .ok_or("manifest has no parent")?;
        packages_by_path.insert(fs_err::canonicalize(directory)?, package.name.as_str());
    }

    let mut violations = Vec::new();
    for package in packages {
        let Some(allowed) = allowed_dependencies(package.name.as_str()) else {
            violations.push(format!(
                "workspace package without dependency policy: {}",
                package.name
            ));

            continue;
        };

        for dependency in &package.dependencies {
            let Some(path) = &dependency.path else {
                continue;
            };
            let path = fs_err::canonicalize(path)?;
            let Some(target) = packages_by_path.get(&path) else {
                continue;
            };

            if !allowed.contains(target) {
                violations.push(format!(
                    "forbidden internal dependency: {} -> {target}",
                    package.name
                ));
            }
        }
    }

    violations.sort();
    violations.dedup();

    return Ok(violations);
}

fn metadata(manifest: &Path) -> Result<Metadata, cargo_metadata::Error> {
    return MetadataCommand::new()
        .manifest_path(manifest)
        .no_deps()
        .other_options(vec!["--locked".to_owned(), "--offline".to_owned()])
        .exec();
}

fn fixture(packages: &[(&str, &str)]) -> Result<TempDir, Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let members = packages
        .iter()
        .map(|(name, _)| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(", ");

    fs_err::write(
        directory.path().join("Cargo.toml"),
        format!("[workspace]\nresolver = \"3\"\nmembers = [{members}]\n"),
    )?;

    for (name, dependencies) in packages {
        let package = directory.path().join(name);
        fs_err::create_dir_all(package.join("src"))?;
        fs_err::write(package.join("src/lib.rs"), "")?;
        fs_err::write(package.join("Cargo.toml"), format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[dependencies]\n{dependencies}\n"))?;
    }

    return Ok(directory);
}

fn fixture_violations(packages: &[(&str, &str)]) -> Result<Vec<String>, Box<dyn Error>> {
    let directory = fixture(packages)?;
    let manifest = directory.path().join("Cargo.toml");

    return violations(&metadata(&manifest)?);
}

#[test]
fn workspace_dependencies_follow_the_architecture() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let workspace = metadata(&manifest).expect("workspace metadata");
    let violations = violations(&workspace).expect("workspace package paths");
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

#[test]
fn allows_removing_an_unused_dependency() {
    let actual = fixture_violations(&[("termtrace-control", ""), ("termtrace-model", "")])
        .expect("fixture metadata");
    assert_eq!(actual, Vec::<String>::new());
}

#[test]
fn allows_a_subset_of_permitted_dependencies() {
    let actual = fixture_violations(&[
        (
            "termtrace-control",
            "termtrace-model = { path = \"../termtrace-model\" }",
        ),
        ("termtrace-model", ""),
    ])
    .expect("fixture metadata");
    assert_eq!(actual, Vec::<String>::new());
}

#[test]
fn rejects_a_dependency_in_the_forbidden_direction() {
    let actual = fixture_violations(&[
        (
            "termtrace-model",
            "termtrace-engine = { path = \"../termtrace-engine\" }",
        ),
        ("termtrace-engine", ""),
    ])
    .expect("fixture metadata");
    assert_eq!(
        actual,
        ["forbidden internal dependency: termtrace-model -> termtrace-engine"]
    );
}

#[test]
fn rejects_an_unclassified_workspace_package() {
    let actual = fixture_violations(&[("termtrace-unclassified", "")]).expect("fixture metadata");
    assert_eq!(
        actual,
        ["workspace package without dependency policy: termtrace-unclassified"]
    );
}

#[test]
fn resolves_a_renamed_dependency_by_its_package_path() {
    let actual = fixture_violations(&[
        (
            "termtrace-model",
            "engine_alias = { package = \"termtrace-engine\", path = \"../termtrace-engine\" }",
        ),
        ("termtrace-engine", ""),
    ])
    .expect("fixture metadata");
    assert_eq!(
        actual,
        ["forbidden internal dependency: termtrace-model -> termtrace-engine"]
    );
}

#[test]
fn ignores_external_packages_with_workspace_names() {
    let actual = fixture_violations(&[
        ("termtrace-model", "termtrace-engine = \"99.0.0\""),
        ("termtrace-engine", ""),
    ])
    .expect("fixture metadata");
    assert_eq!(actual, Vec::<String>::new());
}
