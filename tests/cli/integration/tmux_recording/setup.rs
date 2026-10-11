use super::*;
use termtrace_tmux::{
    artifact::{TmuxRecordingCoverage, TmuxSocket},
    client::TmuxClient,
    scope::TmuxScopePolicy,
    setup::{GhosttyProfileMode, PreparedTmuxSetup},
};

#[test]
fn watchdog_keeps_tmux_format_expressions_in_executable_paths() {
    let temporary = TempDir::new().expect("temporary directory");
    let server = PrivateTmux::unmanaged(&temporary, "/bin/sh");
    let root = temporary.path().join("case");
    fs_err::create_dir(&root).expect("case directory");
    let executable = root.join("literal-#{pid}-");
    fs_err::write(
        &executable,
        b"#!/bin/sh\nprintf '%s\\000' \"$TERMTRACE_TMUX_BINARY\" \"$@\" > \"$0.result\"\n",
    )
    .expect("write capture executable");
    fs_err::set_permissions(&executable, Permissions::from_mode(0o700))
        .expect("executable permissions");
    let setup = PreparedTmuxSetup {
        launch_hook: false,
        termtrace: executable.clone(),
        tmux: executable,
        output_root: root.clone(),
        profile_mode: GhosttyProfileMode::Off,
        ghostty_config: None,
        coverage: TmuxRecordingCoverage::Warn,
        aggregate_manifest: None,
        cell_width: 8,
        cell_height: 16,
        term: String::new(),
        scope: TmuxScopePolicy::all(),
    };
    let client = TmuxClient::new(
        server.binary.clone(),
        TmuxSocket::Name(ByteString::from(server.socket.as_bytes())),
    );
    let command = setup.watchdog_command();
    client
        .run([OsStr::new("run-shell"), command.as_os_str()])
        .expect("consume watchdog command");

    let mut expected = setup.tmux.as_os_str().as_bytes().to_vec();
    expected.extend_from_slice(b"\0tmux\0watchdog\0-q\0");
    assert_eq!(
        fs_err::read(root.join("literal-#{pid}-.result")).expect("literal executable ran"),
        expected
    );
}
