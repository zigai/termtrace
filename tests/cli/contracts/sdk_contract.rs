//! Independent contract checks; the cross-language runner owns adapter execution.
use std::{path::Path, process::Command};
use termtrace_sdk::contracts::{decode_media_artifact, encode_media_artifact};

#[test]
fn sdk_contract_inventory_is_complete_and_consistent() {
    let result = Command::new("python3")
        .args(["scripts/sdk-conformance.py", "--profile", "inventory"])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn sdk_contract_portable_vector_preserves_exact_values() {
    let wire = include_str!("../../../fixtures/conformance/sdk/cases/portable.json");
    let value = decode_media_artifact(wire).unwrap();
    assert_eq!(value.members[0].byte_length.0, 9_007_199_254_740_993);
    assert_eq!(value.source.boundary.next_event_seq.0, u64::MAX);
    assert_eq!(
        decode_media_artifact(&encode_media_artifact(&value).unwrap()).unwrap(),
        value
    );
}
