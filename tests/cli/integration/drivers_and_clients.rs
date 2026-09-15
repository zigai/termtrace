//! drivers and clients CLI integration tests.

use crate::support::*;

#[test]
fn driver_handshake_attaches_sessions_and_caches_correlated_retries() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("driver-session");
    let mut record = Command::cargo_bin("termtrace").expect("binary should build");
    record
        .args(["record", "--no-input", "--out"])
        .arg(&session)
        .args(["/bin/sh", "-c", "printf DRIVER"])
        .assert()
        .success();

    let hello_id = "019f0000-0000-7000-8000-000000000001";
    let attach_id = "019f0000-0000-7000-8000-000000000002";

    let hello = serde_json::json!({
        "version": 5,
        "request_id": hello_id,
        "method": { "method": "hello" }
    });

    let attach = serde_json::json!({
        "version": 5,
        "request_id": attach_id,
        "method": { "method": "attach", "session": session }
    });

    let input = format!("{hello}\n{hello}\n{attach}\n");
    let mut driver = Command::cargo_bin("termtrace").expect("binary should build");
    let output = driver
        .arg("agent")
        .write_stdin(input)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let responses = output
        .split(|byte| return *byte == b'\n')
        .filter(|line| return !line.is_empty())
        .map(|line| return serde_json::from_slice::<serde_json::Value>(line).expect("driver JSON"))
        .collect::<Vec<_>>();

    let [first, second, third] = responses.as_slice() else {
        panic!("expected three driver responses");
    };

    assert_eq!(first, second);
    assert_eq!(first["result"]["data"]["kind"], "hello");
    assert_eq!(first["result"]["data"]["driver_protocol_version"], 5);
    assert_eq!(third["result"]["data"]["kind"], "attached");
    assert_eq!(third["result"]["data"]["completed"], true);
    assert!(third["result"]["data"]["handle"].is_string());
}

#[test]
fn removed_language_sdks_and_tooling_are_not_present() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let deleted_paths = [
        "packages/python",
        "packages/typescript",
        "tests/consumers/python",
        "tests/consumers/typescript",
        "scripts/build-python-wheel.sh",
        "scripts/build-npm-platform-package.sh",
        "scripts/generate-client-protocol.sh",
        "scripts/ci-clients.sh",
    ];

    for path in deleted_paths {
        assert!(
            !root.join(path).exists(),
            "deleted path {path} must not exist in repository"
        );
    }

    // CLI client subcommand must be rejected
    let mut command = Command::cargo_bin("termtrace").expect("binary should build");
    command.arg("client").assert().failure();

    // Verify configuration files have no residue of deleted packages
    let justfile = fs_err::read_to_string(root.join("Justfile")).expect("Justfile");
    assert!(!justfile.contains("packages/python"));
    assert!(!justfile.contains("packages/typescript"));

    let pre_commit = fs_err::read_to_string(root.join(".pre-commit-config.yaml"))
        .expect(".pre-commit-config.yaml");
    assert!(!pre_commit.contains("packages/python"));
    assert!(!pre_commit.contains("packages/typescript"));

    let ci_yml = fs_err::read_to_string(root.join(".github/workflows/ci.yml")).expect("ci.yml");
    assert!(!ci_yml.contains("clients:"));
    assert!(!ci_yml.contains("packages/python"));
    assert!(!ci_yml.contains("packages/typescript"));

    let release_yml =
        fs_err::read_to_string(root.join(".github/workflows/release.yml")).expect("release.yml");
    assert!(!release_yml.contains("packages/python"));
    assert!(!release_yml.contains("packages/typescript"));
    assert!(!release_yml.contains("publish-python"));
    assert!(!release_yml.contains("publish-npm"));

    let installer = fs_err::read_to_string(root.join("install.sh")).expect("install.sh");
    assert!(!installer.contains("TERMTRACE_CLIENT_DIR"));
    assert!(!installer.contains("clients/python"));
    assert!(!installer.contains("clients/typescript"));
}

#[test]
fn installer_script_validates_binary_only_release() {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/install.sh");
    let status = StdCommand::new("bash")
        .arg(&script)
        .status()
        .expect("run tests/install.sh");
    assert!(status.success(), "tests/install.sh should pass");
}
