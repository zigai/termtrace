//! Task08-only schemas; no generated binding declarations.

use jsonschema::{meta::is_valid, Validator};
use termtrace_schema::catalog;

#[expect(
    clippy::expect_used,
    reason = "test schema fixture construction must fail loudly instead of skipping"
)]
fn validator(filename: &str) -> Validator {
    let entry = catalog()
        .expect("valid derivative schema fixture")
        .iter()
        .find(|entry| return entry.filename == filename)
        .expect("valid derivative schema fixture");

    let schema: serde_json::Value = serde_json::from_slice(
        &entry
            .generate()
            .expect("valid derivative schema fixture")
            .json,
    )
    .expect("valid derivative schema fixture");
    assert!(is_valid(&schema));

    return jsonschema::validator_for(&schema).expect("valid derivative schema fixture");
}

#[test]
fn derivative_options_schema_preserves_explicit_source_and_fixed_formats() {
    let v = validator("derivative-export-options.schema.json");
    for input in [
        r#"{"formats":[{"format":"gif"},{"format":"webm"}],"source_kind":"semantic_rendering"}"#,
        r#"{"formats":[{"format":"gif","loop":"once"}],"source_kind":"native_replay"}"#,
    ] {
        assert!(v.is_valid(&serde_json::from_str(input).expect("valid derivative schema fixture")));
    }

    for input in [
        r#"{"formats":[],"source_kind":"native_replay"}"#,
        r#"{"formats":[{"format":"webm","loop":"infinite"}],"source_kind":"original_execution"}"#,
        r#"{"formats":[{"format":"gif","transparent":true}],"source_kind":"native_replay"}"#,
        r#"{"formats":[{"format":"webm"}]}"#,
    ] {
        assert!(
            !v.is_valid(&serde_json::from_str(input).expect("valid derivative schema fixture")),
            "{input}"
        );
    }
}

#[test]
fn gif_timing_schema_uses_exact_decimal_frame_ids_and_explicit_units() {
    let v = validator("gif-frame-timing.schema.json");
    assert!(v.is_valid(&serde_json::json!({"frame":"9007199254740993","start_centiseconds":"123","delay_centiseconds":2})));
    assert!(!v.is_valid(&serde_json::json!({"frame":9_007_199_254_740_993_u64,"start_centiseconds":"123","delay_centiseconds":2})));
    assert!(!v.is_valid(&serde_json::json!({"frame":"18446744073709551616","start_centiseconds":"123","delay_centiseconds":2})));
}
