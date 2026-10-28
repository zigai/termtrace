mod support;

use serde_json::Value;
use std::{
    env, fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};
use support::{scratch, shell, stop, terminal, wait_text};
use termtrace_sdk::{input::MutationGuard, session::LaunchSpec};

fn recorded(name: &str) -> PathBuf {
    let terminal = terminal(name);
    let mut session = terminal
        .start(LaunchSpec::new(shell("printf READY; IFS= read -r _")).geometry(40, 6))
        .expect("launch");
    wait_text(&session, "READY");
    session
        .mark("start", &MutationGuard::default())
        .expect("start marker");
    thread::sleep(Duration::from_millis(300));
    session
        .mark("end", &MutationGuard::default())
        .expect("end marker");
    stop(&mut session);

    return session.path().to_path_buf();
}

fn which(program: &str) -> PathBuf {
    return env::split_paths(&env::var_os("PATH").expect("PATH"))
        .map(|directory| return directory.join(program))
        .find(|candidate| return candidate.is_file())
        .expect("program on PATH");
}

fn without_vp9(root: &Path) -> PathBuf {
    let tools = root.join("tools");
    fs::create_dir_all(&tools).expect("tools directory");
    let ffmpeg = tools.join("ffmpeg");
    fs::write(
        &ffmpeg,
        format!(
            "#!/bin/sh\ncase \" $* \" in *libvpx-vp9*)\n    echo 'Unknown encoder libvpx-vp9' >&2\n    exit 1\n    ;;\nesac\nexec '{}' \"$@\"\n",
            which("ffmpeg").display()
        ),
    )
    .expect("ffmpeg wrapper");
    fs::set_permissions(&ffmpeg, fs::Permissions::from_mode(0o700)).expect("executable wrapper");

    return tools;
}

fn probe(session: &Path, out: &Path, tools: Option<&Path>) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_delivery"));
    command.arg(session).arg(out);
    if let Some(tools) = tools {
        let path = env::var_os("PATH").expect("PATH");
        let joined = env::join_paths(
            [tools.to_path_buf()]
                .into_iter()
                .chain(env::split_paths(&path)),
        )
        .expect("joined PATH");
        command.env("PATH", joined);
    }
    let output = command.output().expect("run the delivery consumer");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    return serde_json::from_slice(&output.stdout).expect("delivery JSON");
}

fn capability<'report>(report: &'report Value, id: &str) -> &'report Value {
    return report["report"]["capabilities"]
        .as_array()
        .expect("capabilities")
        .iter()
        .find(|capability| return capability["id"] == id)
        .unwrap_or_else(|| panic!("{id} missing from {report}"));
}

#[test]
fn gif_and_webm_delivery_are_ready_with_the_host_ffmpeg() {
    let root = scratch("delivery-ready");
    let session = recorded("delivery-ready-session");
    let out = root.join("ready.webm");

    let report = probe(&session, &out, None);

    assert_eq!(
        capability(&report, "gif-delivery")["status"],
        "ready",
        "{report}"
    );
    assert_eq!(
        capability(&report, "webm-delivery")["status"],
        "ready",
        "{report}"
    );
    assert!(report["require_error"].is_null(), "{report}");
    assert!(report["render_error"].is_null(), "{report}");
    assert!(out.is_file());
}

#[test]
fn an_ffmpeg_without_vp9_blocks_only_webm_delivery() {
    let root = scratch("delivery-blocked");
    let session = recorded("delivery-blocked-session");
    let out = root.join("blocked.webm");

    let report = probe(&session, &out, Some(&without_vp9(&root)));

    assert_eq!(
        capability(&report, "gif-delivery")["status"],
        "ready",
        "{report}"
    );
    let webm = capability(&report, "webm-delivery");
    assert_eq!(webm["status"], "blocked", "{report}");
    assert_eq!(webm["code"], "derivative_encoder_unavailable", "{report}");
    for error in [&report["require_error"], &report["render_error"]] {
        let message = error.as_str().expect("an error");
        assert!(message.contains("webm-delivery"), "{message}");
        assert!(
            message.contains("derivative_encoder_unavailable"),
            "{message}"
        );
    }
    assert!(!out.exists());
}
