use super::support::*;

fn termtrace() -> Command {
    return Command::new(assert_cmd::cargo::cargo_bin!("termtrace"));
}

#[test]
fn agent_success_is_one_compact_versioned_object() {
    let output = termtrace()
        .args(["--response", "agent", "tooling", "version"])
        .output()
        .expect("run version command");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response = compact_json_line(&output.stdout);
    validate_json_value("cli-response.schema.json", &response);
    assert_eq!(response["v"], 1);
    assert_eq!(response["ok"], true);
    assert_eq!(response["type"], "version");
    assert!(
        output.stdout.len() <= 64 * 1024,
        "agent response was {} bytes",
        output.stdout.len()
    );
}

#[test]
fn explicit_agent_help_is_versioned_but_default_help_stays_readable() {
    let agent = termtrace()
        .args(["--response", "agent", "--help"])
        .output()
        .expect("run agent help");
    assert!(agent.status.success());
    let response = compact_json_line(&agent.stdout);
    validate_json_value("cli-response.schema.json", &response);
    assert_eq!(response["v"], 1);
    assert_eq!(response["ok"], true);
    assert_eq!(response["type"], "help");
    assert!(
        response["output"]
            .as_str()
            .is_some_and(|help| return help.contains("canonical output"))
    );

    let default = termtrace()
        .arg("--help")
        .output()
        .expect("run default help");
    assert!(default.status.success());
    assert!(String::from_utf8_lossy(&default.stdout).contains("Agent workflow:"));
}

#[test]
fn agent_argument_failure_keeps_stdout_strict() {
    let output = termtrace()
        .args(["--response", "agent", "not-a-command"])
        .output()
        .expect("run invalid command");

    assert!(!output.status.success());
    let response = compact_json_line(&output.stdout);
    validate_json_value("cli-response.schema.json", &response);
    assert_eq!(response["v"], 1);
    assert_eq!(response["ok"], false);
    assert_eq!(response["type"], "arguments");
    assert_eq!(response["code"], "invalid_arguments");
    assert_eq!(response["applied"], "none");
    assert_eq!(response["retry"], "safe_new_request");
    assert_eq!(response["session_state"], "unknown");
    assert!(!output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("{\"v\""));
}

#[test]
fn environment_selects_agent_and_flag_takes_precedence() {
    let agent = termtrace()
        .env("TERMTRACE_RESPONSE", "agent")
        .args(["tooling", "version"])
        .output()
        .expect("run agent version");
    assert!(agent.status.success());
    let response = compact_json_line(&agent.stdout);
    assert_eq!(response["v"], 1);
    assert_eq!(response["ok"], true);
    assert_eq!(response["type"], "version");

    let quiet = termtrace()
        .env("TERMTRACE_RESPONSE", "agent")
        .args(["--response", "quiet", "tooling", "version"])
        .output()
        .expect("run quiet version");
    assert!(quiet.status.success());
    assert!(quiet.stdout.is_empty());
    assert!(quiet.stderr.is_empty());
}

#[test]
fn mutation_failure_has_retry_state_without_second_stdout_error() {
    let output = termtrace()
        .args([
            "--response",
            "agent",
            "send",
            "name:response-contract-session-that-does-not-exist",
            "text:x",
        ])
        .output()
        .expect("run missing-session send");

    assert!(!output.status.success());
    let response = compact_json_line(&output.stdout);
    assert_eq!(response["ok"], false);
    assert!(response["applied"].is_string());
    assert!(response["retry"].is_string());
    assert!(response["session_state"].is_string());
    assert_eq!(
        output
            .stdout
            .iter()
            .filter(|byte| return **byte == b'\n')
            .count(),
        1
    );
}

#[test]
fn full_preserves_legacy_forensic_payload_and_quiet_is_exit_only() {
    let full = termtrace()
        .args(["--response", "full", "tooling", "version", "--json"])
        .output()
        .expect("run full version");
    assert!(full.status.success());
    let value = compact_json_line(&full.stdout);
    assert!(value.get("termtrace_version").is_some());
    assert!(value.get("ok").is_none());

    let quiet = termtrace()
        .args(["--response", "quiet", "tooling", "version", "--json"])
        .output()
        .expect("run quiet version");
    assert!(quiet.status.success());
    assert!(quiet.stdout.is_empty());

    let quiet_failure = termtrace()
        .args(["--response", "quiet", "not-a-command"])
        .output()
        .expect("run quiet failure");
    assert!(!quiet_failure.status.success());
    assert!(quiet_failure.stdout.is_empty());
    assert!(quiet_failure.stderr.is_empty());
}
