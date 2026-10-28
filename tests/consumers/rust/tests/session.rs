mod support;

use std::{
    fs,
    time::{Duration, Instant},
};
use support::{complete, shell, stop, terminal, wait_text, Launch, TIMEOUT};
use termtrace_sdk::{session::LaunchSpec, ByteString, EnvironmentBase, EnvironmentSpec};

#[test]
fn environment_replaces_the_child_environment() {
    let terminal = terminal("session-environment");
    let mut environment = EnvironmentSpec {
        base: EnvironmentBase::Clear,
        ..EnvironmentSpec::default()
    };
    environment.set.insert(
        "SAMPLE_VALUE".to_owned(),
        ByteString::new(b"alpha-42".to_vec()),
    );
    let mut session = terminal
        .launch(
            LaunchSpec::new(shell(
                "printf 'VALUE=%s HOME=%s END' \"$SAMPLE_VALUE\" \"${HOME:-absent}\"; IFS= read -r _",
            ))
            .environment(environment),
        )
        .expect("launch");

    wait_text(&session, "VALUE=alpha-42 HOME=absent END");
    stop(&mut session);
}

#[test]
fn working_directory_sets_the_child_cwd() {
    let terminal = terminal("session-working-directory");
    let directory = terminal.evidence_root().with_file_name("child-cwd");
    fs::create_dir_all(&directory).expect("child directory");
    let directory = directory.canonicalize().expect("canonical directory");
    let mut session = terminal
        .launch(
            LaunchSpec::new(shell("printf 'CWD=%s;' \"$(pwd -P)\"; IFS= read -r _"))
                .working_directory(&directory),
        )
        .expect("launch");

    let expected = format!("CWD={};", directory.display());
    session
        .output()
        .wait_for(expected.as_bytes(), TIMEOUT)
        .expect("child printed the working directory");
    stop(&mut session);
}

#[test]
fn resource_limits_end_the_session_with_their_kind() {
    let terminal = terminal("session-resource-limits");
    for (kind, spec) in [
        (
            "output_bytes",
            LaunchSpec::new(shell("while :; do printf 0123456789; done")).output_byte_budget(4096),
        ),
        (
            "max_duration",
            LaunchSpec::new(shell("printf READY; IFS= read -r _"))
                .max_duration(Duration::from_millis(400)),
        ),
        (
            "idle_timeout",
            LaunchSpec::new(shell("printf READY; IFS= read -r _"))
                .idle_timeout(Duration::from_millis(400)),
        ),
    ] {
        let started = Instant::now();
        let mut session = terminal.launch(spec).expect("launch");
        let completion = complete(&mut session);
        let limit = completion.resource_limit.expect("resource limit outcome");

        assert_eq!(serde_json::to_value(limit.kind).expect("kind"), kind);
        assert!(limit.observed >= limit.limit, "{kind}: {limit:?}");
        assert!(started.elapsed() < Duration::from_secs(8), "{kind}");
        if kind == "output_bytes" {
            assert_eq!(limit.limit, 4096);
            assert!(completion.output_bytes >= 4096);
        } else {
            assert_eq!(limit.limit, 400);
        }
    }
}

#[test]
fn discard_completed_removes_only_completed_sessions() {
    let terminal = terminal("session-discard");
    let mut session = terminal
        .launch(LaunchSpec::new(shell("printf READY; IFS= read -r _")))
        .expect("launch");
    wait_text(&session, "READY");

    assert!(session.discard_completed().is_err());
    assert!(session.path().join("session.json").is_file());

    stop(&mut session);
    let path = session.path().to_path_buf();
    session.discard_completed().expect("discard");

    assert!(!path.exists());
}
