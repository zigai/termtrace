use crate::support::*;

#[test]
fn benchmark_wrapper_keeps_command_substitutions_in_paths_literal() {
    let temporary = TempDir::new().expect("temporary directory");
    let directory = temporary.path().join("$(touch unexpected)");
    fs_err::create_dir(&directory).expect("literal directory");
    let binary = directory.join("termtrace");
    fs_err::write(
        &binary,
        b"#!/usr/bin/env bash\ncase \"$1\" in\nrecord|status) exit 0;;\nwait) printf wait > \"$0.invoked\"; exit 17;;\n*) exit 18;;\nesac\n",
    )
    .expect("fixture executable");

    fs_err::set_permissions(&binary, Permissions::from_mode(0o700))
        .expect("executable permissions");

    let output = StdCommand::new("bash")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("benchmarks/run-agent-workflows.sh"))
        .current_dir(temporary.path())
        .env("TERMTRACE_BINARY", &binary)
        .env("TMPDIR", &directory)
        .env("OUTPUT", temporary.path().join("results.jsonl"))
        .env_remove("CONTRACT")
        .output()
        .expect("run benchmark script");

    assert!(!temporary.path().join("unexpected").exists());
    assert_eq!(output.status.code(), Some(17), "{output:?}");
    assert_eq!(
        fs_err::read(directory.join("termtrace.invoked")).expect("intended executable ran"),
        b"wait"
    );
}
