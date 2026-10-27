use super::*;
use jsonschema::meta::is_valid;
use std::{collections::BTreeSet, path::Path};

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
            "regenerate schemas with `termtrace schema --out schemas`"
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
fn sdk_artifact_contract_vector_matches_schema() {
    let entry = catalog()
        .unwrap()
        .iter()
        .find(|e| return e.filename == "sdk-media-artifact-result.schema.json")
        .unwrap();

    let schema: serde_json::Value =
        serde_json::from_slice(&entry.generate().unwrap().json).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/conformance/sdk/cases/portable.json"
    ))
    .unwrap();
    assert!(validator.is_valid(&value));

    value["members"][0]["byte_length"] = serde_json::json!(9_007_199_254_740_993_u64);

    assert!(!validator.is_valid(&value));
}

#[test]
fn sdk_schema_rejects_overflow_and_requires_nullable_member_key() {
    let entry = catalog()
        .unwrap()
        .iter()
        .find(|e| return e.filename == "sdk-media-artifact-result.schema.json")
        .unwrap();

    let schema: serde_json::Value =
        serde_json::from_slice(&entry.generate().unwrap().json).unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let original: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/conformance/sdk/cases/portable.json"
    ))
    .unwrap();

    for bad in ["18446744073709551616", "01", "-1", "1.5"] {
        let mut value = original.clone();
        value["members"][0]["byte_length"] = bad.into();

        assert!(!validator.is_valid(&value), "schema accepted {bad}");
    }

    let mut value = original;
    value["members"][0]
        .as_object_mut()
        .unwrap()
        .remove("schema_type");

    assert!(!validator.is_valid(&value));
}
