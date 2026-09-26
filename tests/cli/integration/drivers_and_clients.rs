//! drivers and clients CLI integration tests.

use crate::support::*;
use termtrace_model::protocol::driver::DRIVER_PROTOCOL_VERSION;

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
        "version": DRIVER_PROTOCOL_VERSION,
        "request_id": hello_id,
        "method": { "method": "hello" }
    });

    let attach = serde_json::json!({
        "version": DRIVER_PROTOCOL_VERSION,
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
    assert_eq!(
        first["result"]["data"]["driver_protocol_version"],
        DRIVER_PROTOCOL_VERSION
    );
    assert_eq!(third["result"]["data"]["kind"], "attached");
    assert_eq!(third["result"]["data"]["completed"], true);
    assert!(third["result"]["data"]["handle"].is_string());
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
