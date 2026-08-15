use super::*;

#[test]
fn tmux_explicit_scope_records_only_forced_or_explicit_panes() {
    let temporary = TempDir::new().expect("temporary directory");
    let Some(server) =
        PrivateTmux::new_with_config_args(&temporary, "/bin/sh", &["--explicit-only"])
    else {
        return;
    };

    server.wait_for_process_marker("%0", "@termtrace-excluded-pid");
    assert!(server.pane_option("%0", "@termtrace-session").is_empty());

    let ordinary = tmux_new_window(&server, &[]);
    server.wait_for_process_marker(&ordinary, "@termtrace-excluded-pid");
    thread::sleep(Duration::from_millis(700));
    assert!(
        server.pane_exists(&ordinary),
        "strict enforcement closed an intentionally excluded pane"
    );
    assert!(
        server
            .pane_option(&ordinary, "@termtrace-session")
            .is_empty()
    );

    let executable = server.configured_executable();
    let forced = tmux_new_window(&server, &[executable.as_str(), "tmux", "run", "--force"]);
    let recording = server.pane_session(&forced);
    let sent = server
        .command()
        .args(["send-keys", "-t", forced.as_str(), "exit", "Enter"])
        .status()
        .expect("close forced recording");
    assert!(sent.success(), "forced pane did not accept input");
    wait_for_path(&recording.join("completion.json"));
    assert_eq!(
        read_manifest(&recording)
            .expect("forced pane manifest")
            .tmux_pane
            .expect("tmux pane provenance")
            .launch_kind,
        TmuxPaneLaunchKind::DefaultShell
    );
    assert!(server.pane_exists(&ordinary));
}

#[test]
fn tmux_session_scope_records_only_the_selected_session() {
    let temporary = TempDir::new().expect("temporary directory");
    let Some(server) =
        PrivateTmux::new_with_config_args(&temporary, "/bin/sh", &["--explicit-only"])
    else {
        return;
    };
    server.wait_for_process_marker("%0", "@termtrace-excluded-pid");

    let mut scope = Command::cargo_bin("termtrace").expect("binary should build");
    let changed = scope
        .args(["tmux", "scope", "--tmux"])
        .arg(&server.binary)
        .args(["-L", server.socket.as_str(), "--session", "recorded"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let changed = compact_json_line(&changed);
    assert_eq!(changed["changed"], true);
    assert_eq!(changed["future_processes_only"], true);
    assert_eq!(changed["scope"]["mode"], "selected");
    assert_eq!(changed["scope"]["session_ids"][0], "$0");

    let selected = tmux_new_window(&server, &[]);
    let recording = server.pane_session(&selected);

    let excluded = tmux_new_session(&server, "excluded", &[]);
    server.wait_for_process_marker(&excluded, "@termtrace-excluded-pid");
    thread::sleep(Duration::from_millis(700));
    assert!(server.pane_exists(&excluded));
    assert!(
        server
            .pane_option(&excluded, "@termtrace-session")
            .is_empty()
    );

    let sent = server
        .command()
        .args(["send-keys", "-t", selected.as_str(), "exit", "Enter"])
        .status()
        .expect("close session-scoped recording");
    assert!(sent.success());
    wait_for_path(&recording.join("completion.json"));
    assert!(server.pane_exists(&excluded));
}

#[test]
fn tmux_pane_scope_records_that_pane_after_its_process_restarts() {
    let temporary = TempDir::new().expect("temporary directory");
    let Some(server) =
        PrivateTmux::new_with_config_args(&temporary, "/bin/sh", &["--explicit-only"])
    else {
        return;
    };
    server.wait_for_process_marker("%0", "@termtrace-excluded-pid");
    let other = tmux_new_window(&server, &[]);
    server.wait_for_process_marker(&other, "@termtrace-excluded-pid");

    let mut scope = Command::cargo_bin("termtrace").expect("binary should build");
    let changed = scope
        .args(["tmux", "scope", "--tmux"])
        .arg(&server.binary)
        .args(["-L", server.socket.as_str(), "--pane", "%0"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let changed = compact_json_line(&changed);
    assert_eq!(changed["scope"]["mode"], "selected");
    assert_eq!(changed["scope"]["pane_ids"][0], "%0");
    assert!(
        server.pane_option("%0", "@termtrace-session").is_empty(),
        "scope changes must not attach to a running pane process"
    );

    let wrapper = server
        .command()
        .args(["show-options", "-gv", "default-command"])
        .output()
        .expect("query managed default command");
    assert!(wrapper.status.success());
    let restarted = server
        .command()
        .args(["respawn-pane", "-k", "-t", "%0"])
        .arg(OsString::from_vec(trim_newline(wrapper.stdout)))
        .status()
        .expect("restart selected pane through managed wrapper");
    assert!(restarted.success());
    let recording = server.pane_session("%0");
    let sent = server
        .command()
        .args(["send-keys", "-t", "%0", "exit", "Enter"])
        .status()
        .expect("close pane-scoped recording");
    assert!(sent.success());
    wait_for_path(&recording.join("completion.json"));
    assert!(server.pane_exists(&other));
    assert!(server.pane_option(&other, "@termtrace-session").is_empty());
}

#[test]
fn tmux_window_scope_records_new_panes_only_in_that_window() {
    let temporary = TempDir::new().expect("temporary directory");
    let Some(server) =
        PrivateTmux::new_with_config_args(&temporary, "/bin/sh", &["--explicit-only"])
    else {
        return;
    };
    server.wait_for_process_marker("%0", "@termtrace-excluded-pid");

    let mut scope = Command::cargo_bin("termtrace").expect("binary should build");
    let changed = scope
        .args(["tmux", "scope", "--tmux"])
        .arg(&server.binary)
        .args(["-L", server.socket.as_str(), "--window", "recorded:0"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let changed = compact_json_line(&changed);
    assert_eq!(changed["scope"]["mode"], "selected");
    assert_eq!(changed["scope"]["window_ids"][0], "@0");

    let selected = tmux_split_window(&server, "%0");
    let recording = server.pane_session(&selected);
    let excluded = tmux_new_window(&server, &[]);
    server.wait_for_process_marker(&excluded, "@termtrace-excluded-pid");
    thread::sleep(Duration::from_millis(700));
    assert!(server.pane_exists(&excluded));

    let sent = server
        .command()
        .args(["send-keys", "-t", selected.as_str(), "exit", "Enter"])
        .status()
        .expect("close window-scoped recording");
    assert!(sent.success());
    wait_for_path(&recording.join("completion.json"));
    assert!(server.pane_exists(&excluded));
}
