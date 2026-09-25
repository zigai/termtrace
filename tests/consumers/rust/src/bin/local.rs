//! Full local mode consumer testing representative workflows from outside the workspace.

use std::{env, error::Error, ffi::OsString, path::PathBuf, time::Duration};
use termtrace_sdk::{
    analysis::AnalysisService,
    artifacts::ArtifactService,
    events::EventsService,
    input::{ActRequest, KeyChord},
    output::OutputService,
    registry::RegistryService,
    screen::ScreenPredicate,
    session::{LaunchSpec, StopRequest},
    testing::{run_terminal_test, TerminalTestConfig},
    BundleProfile, BundleVerificationPolicy, StatePoint, Terminal,
};

fn main() -> Result<(), Box<dyn Error>> {
    let evidence_root = PathBuf::from(
        env::args_os()
            .nth(1)
            .ok_or("expected evidence root directory argument")?,
    );
    let binary = env::var_os("TERMTRACE_BIN")
        .map(PathBuf::from)
        .ok_or("expected TERMTRACE_BIN")?;

    let _ = std::fs::remove_dir_all(&evidence_root);
    std::fs::create_dir_all(&evidence_root)?;

    // 1. launch -> wait -> guarded action -> finish -> verify
    let terminal = Terminal::new(&binary, evidence_root.join("launch-evidence"));
    let mut live_session = terminal.start(
        LaunchSpec::new([
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from("printf READY; IFS= read -r line; printf '<%s>' \"$line\"; sleep 2"),
        ])
        .geometry(90, 25),
    )?;

    let ready = live_session.wait_screen(
        ScreenPredicate::Contains {
            text: "READY".to_owned(),
        },
        Duration::ZERO,
        Duration::from_secs(5),
    )?;
    assert!(ready.satisfied());

    let frame = live_session.frame(Default::default())?;
    let enter = KeyChord::parse("enter")?.ok_or("enter is a key")?;
    let action_result = live_session.act(
        ActRequest::new()
            .text("hello")
            .key(enter)
            .if_state(frame.state_hash)
            .until_idle(Duration::from_millis(20), Duration::from_secs(2)),
    )?;
    assert_eq!(action_result.input.atom_count, 2);

    let _completion = live_session.stop(StopRequest::default(), Some(Duration::from_secs(5)))?;
    assert!(live_session.path().join("completion.json").is_file());

    // 2. attach -> observe any state of the completed session
    let attached = terminal.attach(live_session.path())?;
    assert!(
        attached
            .observe(StatePoint::LatestPresented, false)?
            .source_completed
    );
    let state_0 = attached.frame_at(
        StatePoint::StateId { state_id: 0 },
        false,
        Default::default(),
    )?;
    let state_latest = attached.frame(Default::default())?;
    assert!(!state_0.state_hash.is_empty());
    assert!(!state_latest.state_hash.is_empty());

    // 3. completed recording -> diff -> render
    let logical_session = &attached;

    let (diff, _, _) = AnalysisService::diff(
        logical_session.path(),
        StatePoint::StateId { state_id: 0 },
        StatePoint::LatestPresented,
    )?;
    assert_eq!(diff.from.state_id, 0);

    // 4. bounded event/output iteration
    let output_page = OutputService::read_page(logical_session.path(), None, 128)?;
    assert!(!output_page.bytes.is_empty());
    let event_page = EventsService::read_page(logical_session.path(), None, 10)?;
    assert!(!event_page.events.is_empty());

    // 5. bundle plan -> inspect -> execute -> verify published source binding
    let bundle_dir = evidence_root.join("bundle-dest");
    let bundle_manifest = ArtifactService::create_bundle(
        logical_session.path(),
        &bundle_dir,
        BundleProfile::Forensic,
    )?;
    assert!(bundle_dir.join("manifest.json").is_file());
    let verified = ArtifactService::verify_bundle(
        &bundle_dir,
        BundleVerificationPolicy::new(logical_session.path(), BundleProfile::Forensic),
    )?;
    assert_eq!(verified.profile, BundleProfile::Forensic);
    assert_eq!(bundle_manifest.profile, BundleProfile::Forensic);

    // 6. registry resolution and planned alias management
    let reg_root = evidence_root.join("reg-root");
    let registry = RegistryService::open(&reg_root)?;
    let reg_entry = registry.register("consumer-alias", logical_session.path())?;
    assert_eq!(reg_entry.alias, "consumer-alias");
    let resolved = registry.resolve("consumer-alias")?;
    assert_eq!(resolved.session_path(), logical_session.path());

    // 7. programmatic scenario/test with failure evidence
    let test_terminal = Terminal::new(&binary, evidence_root.join("test-evidence"));
    let config = TerminalTestConfig::new(
        test_terminal,
        vec![
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from("trap 'exit 0' TERM; printf PASS; while :; do sleep 1; done"),
        ],
    );
    let outcome = run_terminal_test(config, |test| -> Result<(), Box<dyn Error>> {
        test.session()
            .ok_or("session not available")?
            .output()
            .wait_for(b"PASS", Duration::from_secs(5))?;
        Ok(())
    });
    if let Err(err) = &outcome {
        eprintln!("test failure details: {err:?}");
    }
    assert!(outcome.is_ok());

    println!("local consumer verification: OK");
    Ok(())
}
