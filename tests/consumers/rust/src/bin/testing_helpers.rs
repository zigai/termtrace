//! Ordinary Rust test helper consumer: actions, snapshots, failure evidence and verification.
use std::{env, error::Error, ffi::OsString, fs, path::PathBuf, time::Duration};
use termtrace_sdk::{
    assertions::{assert_screen_text, SnapshotProfile},
    input::ActRequest,
    integrity::IntegrityService,
    testing::{run_terminal_test, EvidencePolicy, TerminalTestConfig, TestFailure},
    Terminal,
};
fn main() -> Result<(), Box<dyn Error>> {
    let root = env::var_os("TASK04_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join(format!("rust-helper-{}", std::process::id()));
    fs::create_dir_all(&root)?;
    let binary = env::var_os("TERMTRACE_BIN").ok_or("TERMTRACE_BIN is required")?;
    let terminal = Terminal::new(binary, root.join("evidence"));
    let mut config = TerminalTestConfig::new(
        terminal,
        [
            "/bin/sh",
            "-c",
            "printf READY; while IFS= read -r line; do printf 'INPUT:<%s>' \"$line\"; done",
        ]
        .into_iter()
        .map(OsString::from),
    );
    config.evidence_policy = EvidencePolicy::OnFailure;
    config.snapshot_root = root.join("reviewed");
    config.update_snapshots = true;
    let result = run_terminal_test(config, |test| -> Result<(), Box<dyn Error + Send + Sync>> {
        let session = test.session().ok_or("session closed")?;
        session
            .output()
            .wait_for(b"READY", Duration::from_secs(5))?;
        let action = session.act(ActRequest::new().text("rust\n").until_screen(
            termtrace_sdk::screen::ScreenPredicate::Contains {
                text: "INPUT:<rust>".to_owned(),
            },
            Duration::from_secs(5),
        ))?;
        let frame = action.frame().ok_or("missing terminating frame")?;
        assert_screen_text(frame, "INPUT:<rust>")?;
        test.assert_snapshot(
            "rust-ready",
            frame,
            SnapshotProfile::Spans,
            Default::default(),
        )?;
        return Err("PRIMARY_RUST_FAILURE".into());
    });
    let Err(TestFailure::Body {
        error,
        finalization,
    }) = result
    else {
        return Err("missing body failure".into());
    };
    assert_eq!(error.to_string(), "PRIMARY_RUST_FAILURE");
    assert!(finalization.retained);
    assert!(finalization.secondary_failures.is_empty());
    for artifact in &finalization.verified_artifacts {
        assert!(artifact.exists());
    }
    let canonical = finalization
        .verified_artifacts
        .iter()
        .find(|p| p.join("completion.json").is_file())
        .ok_or("canonical evidence absent")?;
    let verification = IntegrityService::verify(
        canonical,
        termtrace_sdk::integrity::VerificationLevel::Content,
        None,
    );
    assert!(verification.valid);
    fs::write(
        root.join("proof.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"report":finalization,"verification":verification}),
        )?,
    )?;
    println!("Rust helper verified: {}", root.display());
    return Ok(());
}
