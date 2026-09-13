use crate::support::*;

fn termtrace() -> Command {
    return Command::new(assert_cmd::cargo::cargo_bin!("termtrace"));
}

#[test]
fn version_supports_json_and_plain_text() {
    let output = termtrace()
        .args(["version", "--json"])
        .output()
        .expect("run version command");
    assert!(output.status.success());

    let response: serde_json::Value = serde_json::from_slice(&output.stdout).expect("valid JSON");
    assert!(response["termtrace_version"].is_string());

    let plain = termtrace()
        .arg("version")
        .output()
        .expect("run plain version");
    assert!(plain.status.success());
    assert!(String::from_utf8_lossy(&plain.stdout).starts_with("termtrace"));
}

#[test]
fn quiet_mode_suppresses_stdout() {
    let output = termtrace()
        .args(["-q", "version"])
        .output()
        .expect("run quiet version");
    assert!(output.status.success());
    assert!(output.stdout.is_empty());

    let failed = termtrace()
        .args(["-q", "not-a-command"])
        .output()
        .expect("run quiet failure");
    assert_eq!(failed.status.code(), Some(2));
    assert!(failed.stdout.is_empty());
    assert!(failed.stderr.is_empty());
}
