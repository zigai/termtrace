use assert_cmd::Command;
use std::path::Path;

#[allow(clippy::expect_used, reason = "test setup")]
pub(super) fn record_session(path: &Path, script: &str) {
    Command::cargo_bin("termtrace")
        .expect("termtrace binary")
        .args(["record", "--no-input", "--out"])
        .arg(path)
        .args(["--", "/bin/sh", "-c", script])
        .assert()
        .success();
}
