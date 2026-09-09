use super::*;
use jsonschema::meta::is_valid;
use schemars::schema_for;
use std::{collections::BTreeSet, path::Path};
use termtrace_model::protocol::driver::DriverResponse;

#[test]
fn checked_in_schemas_match_catalog() {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas");
    let generated = generate().expect("generate schemas");
    let registered: BTreeSet<String> = generated
        .iter()
        .map(|schema| return schema.filename.to_owned())
        .collect();

    let mut checked_in: BTreeSet<String> = BTreeSet::new();

    for entry in fs_err::read_dir(&directory).expect("read schema directory") {
        let entry = entry.expect("read schema directory entry");
        if entry.file_type().expect("read schema file type").is_dir() {
            continue;
        }

        let filename = entry
            .file_name()
            .into_string()
            .expect("schema filename must be UTF-8");

        if filename.ends_with(".schema.json") {
            checked_in.insert(filename);
        }
    }

    assert_eq!(
        checked_in, registered,
        "every checked-in schema must have exactly one catalog entry"
    );

    for generated in generated {
        let path = directory.join(generated.filename);
        let schema: serde_json::Value =
            serde_json::from_slice(&generated.json).expect("schema JSON");

        assert!(
            is_valid(&schema),
            "invalid JSON Schema {}",
            generated.filename
        );
        assert_eq!(
            fs_err::read(&path).unwrap_or_else(|error| {
                panic!("read checked-in schema {}: {error}", path.display())
            }),
            generated.json,
            "regenerate schemas with `termtrace tooling schemas --out schemas`"
        );
    }
}

#[test]
fn acceptance_benchmark_instance_matches_schema() {
    let schema_bytes = include_bytes!("../../../schemas/acceptance-benchmark.schema.json");
    let schema: serde_json::Value = serde_json::from_slice(schema_bytes).expect("schema json");
    let validator = jsonschema::validator_for(&schema).expect("schema validator");
    let instance: serde_json::Value =
        serde_json::from_slice(include_bytes!("../../../benchmarks/acceptance-v1.json"))
            .expect("acceptance benchmark json");

    assert!(validator.is_valid(&instance));
}

#[test]
fn shared_driver_response_fixtures_match_the_rust_contract() {
    let schema = serde_json::to_value(schema_for!(DriverResponse)).expect("driver schema");
    let validator = jsonschema::validator_for(&schema).expect("driver validator");

    let fixtures: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../fixtures/conformance/protocol/driver-responses.json"
    ))
    .expect("shared protocol fixtures");
    let valid = fixtures
        .get("valid")
        .and_then(serde_json::Value::as_array)
        .expect("valid fixture array");
    let invalid = fixtures
        .get("invalid")
        .and_then(serde_json::Value::as_array)
        .expect("invalid fixture array");

    for case in valid {
        let response = case.get("response").expect("valid fixture response");
        assert!(
            validator.is_valid(response),
            "shared valid response fixture failed: {}",
            case.get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unnamed")
        );
    }

    for case in invalid {
        let response = case.get("response").expect("invalid fixture response");
        assert!(
            !validator.is_valid(response),
            "shared invalid response fixture passed: {}",
            case.get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unnamed")
        );
    }
}
