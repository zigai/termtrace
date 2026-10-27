//! Scenario suite validation and launch-error propagation through the public API.

use std::path::PathBuf;
use termtrace_model::scenario::ScenarioStatus;
use termtrace_scenario::{run_suite, Error, ScenarioSuiteOptions};

#[test]
fn suite_options_error_handling_and_validation() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let evidence_root = temporary.path().join("evidence");
    let failure_root = temporary.path().join("failures");
    let binary = PathBuf::from("/bin/termtrace");

    let base_options = ScenarioSuiteOptions {
        binary,
        evidence_root,
        failure_root,
        jobs: 1,
        shard_index: None,
        shard_count: None,
        update_snapshots: false,
        reruns: 0,
        fail_on_flaky: false,
    };

    // 1. Zero jobs error
    let mut zero_jobs = base_options.clone();
    zero_jobs.jobs = 0;
    let result = run_suite(Vec::new(), &zero_jobs);
    assert!(matches!(result, Err(Error::InvalidScenario { .. })));
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("scenario jobs must be greater than zero"));

    // 2. Shard index >= count error
    let mut invalid_shard = base_options.clone();
    invalid_shard.shard_index = Some(2);
    invalid_shard.shard_count = Some(2);
    let result = run_suite(Vec::new(), &invalid_shard);
    assert!(matches!(result, Err(Error::InvalidScenario { .. })));
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("must be less than non-zero shard count"));

    // 3. Shard count 0 error
    let mut zero_count = base_options.clone();
    zero_count.shard_index = Some(0);
    zero_count.shard_count = Some(0);
    let result = run_suite(Vec::new(), &zero_count);
    assert!(matches!(result, Err(Error::InvalidScenario { .. })));

    // 4. Missing shard count
    let mut missing_count = base_options.clone();
    missing_count.shard_index = Some(0);
    missing_count.shard_count = None;
    let result = run_suite(Vec::new(), &missing_count);
    assert!(matches!(result, Err(Error::InvalidScenario { .. })));
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("scenario sharding requires index and count"));

    // 5. Missing shard index
    let mut missing_index = base_options.clone();
    missing_index.shard_index = None;
    missing_index.shard_count = Some(2);
    let result = run_suite(Vec::new(), &missing_index);
    assert!(matches!(result, Err(Error::InvalidScenario { .. })));

    // 6. Empty scenario selection
    let result = run_suite(Vec::new(), &base_options);
    assert!(matches!(result, Err(Error::InvalidScenario { .. })));
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("scenario selection produced an empty shard"));

    // 7. Non-existent scenario path (filesystem error propagation)
    let nonexistent = temporary.path().join("missing.termtrace.json");
    let result = run_suite(vec![nonexistent], &base_options);
    assert!(matches!(result, Err(Error::Filesystem { .. })));

    // 8. Malformed JSON scenario file (invalid JSON error propagation)
    let malformed_file = temporary.path().join("malformed.termtrace.json");
    fs_err::write(&malformed_file, b"{ not valid json").expect("write malformed");
    let result = run_suite(vec![malformed_file], &base_options);
    assert!(matches!(result, Err(Error::InvalidJson { .. })));

    // 9. Unsupported version scenario file
    let old_version_file = temporary.path().join("old_version.termtrace.json");
    fs_err::write(
        &old_version_file,
        serde_json::to_vec(&serde_json::json!({
            "version": 1,
            "name": "v1 scenario",
            "launch": { "command": ["/bin/echo"] },
            "steps": []
        }))
        .expect("serialize v1"),
    )
    .expect("write v1");

    let result = run_suite(vec![old_version_file], &base_options);
    assert!(matches!(result, Err(Error::InvalidScenario { .. })));
    assert!(result.unwrap_err().to_string().contains("unsupported"));
}

#[test]
fn suite_sharding_and_matrix_expansion_lifecycle() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let evidence_root = temporary.path().join("evidence");
    let failure_root = temporary.path().join("failures");

    // Create scenario 1 with a 2-way matrix
    let matrix_scenario = temporary.path().join("matrix.termtrace.json");
    fs_err::write(
        &matrix_scenario,
        serde_json::to_vec(&serde_json::json!({
            "version": 2,
            "name": "matrix test",
            "launch": {
                "command": ["/bin/echo", "hello"],
                "geometry": { "cols": 80, "rows": 24 },
                "timeoutMs": 1000
            },
            "matrix": {
                "theme": ["dark", "light"]
            },
            "steps": [{ "stop": {} }]
        }))
        .expect("serialize matrix"),
    )
    .expect("write matrix scenario");

    // Create scenario 2 without matrix
    let simple_scenario = temporary.path().join("simple.termtrace.json");
    fs_err::write(
        &simple_scenario,
        serde_json::to_vec(&serde_json::json!({
            "version": 2,
            "name": "simple test",
            "launch": {
                "command": ["/bin/echo", "simple"],
                "geometry": { "cols": 80, "rows": 24 },
                "timeoutMs": 1000
            },
            "steps": [{ "stop": {} }]
        }))
        .expect("serialize simple"),
    )
    .expect("write simple scenario");

    // Total tasks = 2 (matrix) + 1 (simple) = 3 tasks.
    // If we request shard 3 of 4: tasks 0, 1, 2 modulo 4 never equal 3,
    // so the shard is empty, which must produce an error.
    let empty_shard_options = ScenarioSuiteOptions {
        binary: PathBuf::from("/bin/termtrace"),
        evidence_root,
        failure_root,
        jobs: 1,
        shard_index: Some(3),
        shard_count: Some(4),
        update_snapshots: false,
        reruns: 0,
        fail_on_flaky: false,
    };
    let result = run_suite(vec![matrix_scenario, simple_scenario], &empty_shard_options);
    let err = result.unwrap_err();
    assert!(err
        .to_string()
        .contains("scenario selection produced an empty shard"));
}

#[test]
fn suite_execution_error_propagation_when_launcher_binary_missing() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let evidence_root = temporary.path().join("evidence");
    let failure_root = temporary.path().join("failures");
    let missing_binary = temporary.path().join("nonexistent_binary");
    let valid_scenario = temporary.path().join("scenario.termtrace.json");
    fs_err::write(
        &valid_scenario,
        serde_json::to_vec(&serde_json::json!({
            "version": 2,
            "name": "valid scenario",
            "launch": {
                "command": ["/bin/echo"],
                "geometry": { "cols": 80, "rows": 24 },
                "timeoutMs": 1000
            },
            "steps": [{ "stop": {} }]
        }))
        .expect("serialize scenario"),
    )
    .expect("write valid scenario");

    let options = ScenarioSuiteOptions {
        binary: missing_binary,
        evidence_root,
        failure_root,
        jobs: 1,
        shard_index: None,
        shard_count: None,
        update_snapshots: false,
        reruns: 0,
        fail_on_flaky: false,
    };
    let result = run_suite(vec![valid_scenario], &options);
    let report = result.expect("structured launch report");

    assert!(report.ci_failed());

    let attempt = &report.cases[0].attempts[0].report;

    assert_eq!(attempt.status, ScenarioStatus::Error);
    assert_eq!(attempt.session_id, None);
    assert_eq!(attempt.canonical_session, None);
    assert_eq!(attempt.infrastructure_errors[0].phase, "launch");
}
