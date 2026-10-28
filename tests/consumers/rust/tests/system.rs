mod support;

use serde_json::Value;
use std::{collections::BTreeSet, fs, process::Command, sync::atomic::AtomicBool};
use support::{binary, shell, stop, terminal, wait_text, Launch};
use termtrace_sdk::{
    session::LaunchSpec,
    system::{CapabilityStatus, SystemService},
    ControlCommand, ControlRequest,
};

fn cli_version() -> String {
    let output = Command::new(binary())
        .arg("--version")
        .output()
        .expect("termtrace --version");
    assert!(output.status.success());

    return String::from_utf8(output.stdout).expect("UTF-8 version");
}

#[test]
fn versions_match_the_cli_and_the_wire_protocol() {
    let versions = SystemService::versions();

    assert!(
        cli_version().starts_with(&format!("termtrace {} ", versions.termtrace)),
        "{versions:?}"
    );
    assert_eq!(
        versions.control_protocol,
        ControlRequest::new(ControlCommand::Ping)
            .version
            .to_string()
    );
    assert_eq!(versions.target_os, std::env::consts::OS);
    assert_eq!(versions.target_arch, std::env::consts::ARCH);
    assert!(
        versions.ghostty_revision.len() >= 7
            && versions
                .ghostty_revision
                .chars()
                .all(|c| return c.is_ascii_hexdigit()),
        "{versions:?}"
    );
}

#[test]
fn probes_report_every_capability_and_honor_cancellation() {
    let report = SystemService::probe_capabilities(&AtomicBool::new(false));

    assert!(
        cli_version().contains(&format!("(build {})", report.build_revision)),
        "{}",
        report.build_revision
    );
    assert_eq!(
        report.termtrace_version,
        SystemService::versions().termtrace
    );
    let ids: BTreeSet<&str> = report
        .capabilities
        .iter()
        .map(|capability| return capability.id.as_str())
        .collect();
    assert_eq!(ids.len(), report.capabilities.len(), "{ids:?}");
    for id in [
        "semantic",
        "native-screenshot",
        "native-video",
        "presentation",
    ] {
        assert!(ids.contains(id), "{id} missing from {ids:?}");
    }
    let semantic = report.capability("semantic").expect("semantic");
    assert_eq!(semantic.status, CapabilityStatus::Ready);

    let cancelled = SystemService::probe_capabilities(&AtomicBool::new(true));
    let native = cancelled
        .capability("native-screenshot")
        .expect("native screenshot");
    assert_eq!(native.status, CapabilityStatus::Blocked, "{native:?}");
    assert!(native.remediation.is_some() || !native.detail.is_empty());
}

#[test]
fn schemas_list_the_catalog_and_describe_real_evidence() {
    let names = SystemService::schemas().expect("schemas");
    assert!(
        names.contains(&"completion.schema.json".to_owned()),
        "{names:?}"
    );
    assert!(names.iter().all(|name| return name.ends_with(".json")));

    let schema = SystemService::schema("completion.schema.json").expect("completion schema");
    let committed: Value = serde_json::from_slice(
        &fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../schemas/completion.schema.json"
        ))
        .expect("committed schema"),
    )
    .expect("committed schema JSON");
    assert_eq!(schema, committed);

    let terminal = terminal("system-schemas");
    let mut session = terminal
        .launch(LaunchSpec::new(shell("printf READY; IFS= read -r _")))
        .expect("launch");
    wait_text(&session, "READY");
    stop(&mut session);
    let completion: Value = serde_json::from_slice(
        &fs::read(session.path().join("completion.json")).expect("completion.json"),
    )
    .expect("completion JSON");
    let properties = schema["properties"].as_object().expect("properties");
    for key in completion.as_object().expect("completion object").keys() {
        assert!(properties.contains_key(key), "{key} is not in the schema");
    }
    for required in schema["required"].as_array().expect("required") {
        assert!(
            completion.get(required.as_str().expect("name")).is_some(),
            "{required} missing from completion.json"
        );
    }

    assert!(SystemService::schema("no-such.schema.json").is_err());
}
