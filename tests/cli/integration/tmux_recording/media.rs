use super::*;

#[test]
fn tmux_video_uses_shared_optional_presentation_frame_rates() {
    let temporary = TempDir::new().expect("temporary directory");
    let output = temporary.path().join("video.mp4");
    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["tmux", "video", "missing", "--fps", "25", "-o"])
        .arg(&output)
        .assert()
        .failure()
        .stderr(predicate::str::contains("video FPS must be 24, 30, or 60"));

    Command::cargo_bin("termtrace")
        .expect("binary should build")
        .args(["tmux", "video", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[default: 30]").not());
}
