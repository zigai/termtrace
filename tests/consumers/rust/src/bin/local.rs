//! Full local mode consumer testing representative workflows from outside the workspace.

use serde_json::Value;
use std::{
    env,
    error::Error,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
use termtrace_sdk::{
    analysis::AnalysisService,
    artifacts::ArtifactService,
    events::EventsService,
    input::{ActRequest, KeyChord},
    locator::{
        LocatorCondition, LocatorError, LocatorSpec, LocatorWaitOptions, ScreenFrameQuery,
        ScreenMatch, WaitCancellation, CAPTURE_QUANTUM,
    },
    output::OutputService,
    registry::RegistryService,
    screen::ScreenFrame,
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
    assert!(ready.is_satisfied());

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

    // 8. locators, rich frames, and cancellable waits against the shared parity corpus
    locator_workflows(&binary, &evidence_root.join("locator-evidence"))?;

    println!("local consumer verification: OK");
    Ok(())
}

const PARITY: &str = include_str!("../../../../../fixtures/conformance/locators/parity.json");
const PARITY_FRAME: &str =
    include_str!("../../../../../fixtures/conformance/locators/parity-frame.json");

fn parity_spec(spec: &Value) -> Result<LocatorSpec, Box<dyn Error>> {
    let mut locator = if let Some(text) = spec.get("text").and_then(Value::as_str) {
        LocatorSpec::text(text)?
    } else if let Some(text) = spec.get("exact").and_then(Value::as_str) {
        LocatorSpec::exact(text)?
    } else if let Some(pattern) = spec.get("regex").and_then(Value::as_str) {
        LocatorSpec::native_regex(pattern)?
    } else {
        LocatorSpec::default()
    };
    if let Some(row) = spec.get("row").and_then(Value::as_u64) {
        locator = locator.in_row(u16::try_from(row)?);
    }
    if let Some(region) = spec.get("region") {
        locator = locator.in_region(serde_json::from_value(region.clone())?)?;
    }
    if let Some(style) = spec.get("style") {
        locator = locator.with_style(serde_json::from_value(style.clone())?);
    }
    if spec.get("cross_soft_wraps").and_then(Value::as_bool) == Some(true) {
        locator = locator.across_soft_wraps(true);
    }
    Ok(locator)
}

fn check_case(frame: &Arc<ScreenFrame>, case: &Value) -> Result<(), Box<dyn Error>> {
    let name = case["name"].as_str().ok_or("case name")?;
    let outcome = parity_spec(&case["spec"]).and_then(|spec| {
        let result = frame.query(spec)?;
        let selected: Vec<ScreenMatch> = match case.get("operation").and_then(Value::as_str) {
            None => result.all().to_vec(),
            Some("single") => vec![result.single()?.clone()],
            Some("first") => vec![result.first()?.clone()],
            Some("nth") => {
                let index = case["index"].as_u64().ok_or("nth index")?;
                vec![result.nth(usize::try_from(index)?)?.clone()]
            }
            Some(other) => return Err(format!("unknown operation {other}").into()),
        };
        Ok(selected)
    });

    let expect = &case["expect"];
    match outcome {
        Ok(found) => {
            let expected = expect["matches"]
                .as_array()
                .ok_or_else(|| format!("{name}: unexpected success"))?;
            assert_eq!(found.len(), expected.len(), "{name}");
            for (actual, expected) in found.iter().zip(expected) {
                let ranges: Vec<Value> = actual
                    .ranges
                    .iter()
                    .map(|r| serde_json::json!([r.row, r.start_column, r.end_column]))
                    .collect();
                let widths: Vec<Value> = actual
                    .covered_cells
                    .iter()
                    .map(|cell| Value::from(cell.width))
                    .collect();
                assert_eq!(actual.text.as_ref(), expected["text"], "{name}");
                assert_eq!(Value::Array(ranges), expected["ranges"], "{name}");
                assert_eq!(Value::Array(widths), expected["widths"], "{name}");
                assert_eq!(actual.frame.frame_hash, frame.frame_hash, "{name}");
            }
        }
        Err(error) => {
            let query = error
                .downcast_ref::<termtrace_sdk::locator::QueryError>()
                .ok_or_else(|| format!("{name}: {error}"))?;
            let details = serde_json::to_value(query)?;
            let expected = &expect["error"];
            assert_eq!(details["kind"], expected["kind"], "{name}");
            for field in ["count", "index", "state_hash"] {
                if let Some(value) = expected.get(field) {
                    assert_eq!(&details[field], value, "{name}: {field}");
                }
            }
            if let Some(ranges) = expected.get("ranges") {
                let actual: Vec<Value> = details["ranges"]
                    .as_array()
                    .ok_or("ranges")?
                    .iter()
                    .map(|r| serde_json::json!([r["row"], r["start_column"], r["end_column"]]))
                    .collect();
                assert_eq!(&Value::Array(actual), ranges, "{name}");
            }
        }
    }
    Ok(())
}

fn shell(script: &str) -> [OsString; 3] {
    [
        OsString::from("/bin/sh"),
        OsString::from("-c"),
        OsString::from(script),
    ]
}

fn locator_workflows(binary: &Path, evidence_root: &Path) -> Result<(), Box<dyn Error>> {
    let corpus: Value = serde_json::from_str(PARITY)?;

    let offline: Arc<ScreenFrame> = Arc::new(serde_json::from_str(PARITY_FRAME)?);
    for case in corpus["cases"].as_array().ok_or("cases")? {
        check_case(&offline, case)?;
    }

    let live = &corpus["live"];
    let command: Vec<OsString> = live["command"]
        .as_array()
        .ok_or("live command")?
        .iter()
        .map(|part| part.as_str().map(OsString::from).ok_or("command part"))
        .collect::<Result<_, _>>()?;
    let terminal = Terminal::new(binary, evidence_root);
    let mut session = terminal.start(LaunchSpec::new(command).geometry(
        u16::try_from(live["cols"].as_u64().ok_or("cols")?)?,
        u16::try_from(live["rows"].as_u64().ok_or("rows")?)?,
    ))?;
    let ready = session
        .get_by_text(live["ready"].as_str().ok_or("ready")?)?
        .wait(Duration::from_secs(10), Duration::from_millis(20))?;
    let frame = ready.frame;
    for case in live["cases"].as_array().ok_or("live cases")? {
        check_case(&frame, case)?;
    }

    // Legacy delegates keep their signatures and still return the satisfying frame.
    let stable = session.get_by_text("Cancel")?.wait_stable(
        Duration::from_secs(5),
        Duration::from_millis(20),
        Duration::from_millis(50),
    )?;
    assert_eq!(stable.matches.len(), 1);

    let exact_count = session.get_by_text("Save")?.wait_for(LocatorWaitOptions {
        condition: LocatorCondition::Count(2),
        timeout: Duration::from_secs(5),
        interval: Duration::from_millis(20),
        stable_for: Duration::ZERO,
        cancel: None,
    })?;
    assert_eq!(exact_count.matches.len(), 2);

    // A never-satisfied wait times out with its last frame within the documented bound.
    let started = Instant::now();
    let timeout = session
        .get_by_text("NEVER-SHOWN")?
        .wait_for(LocatorWaitOptions {
            condition: LocatorCondition::Exists,
            timeout: Duration::from_secs(1),
            interval: Duration::from_millis(20),
            stable_for: Duration::ZERO,
            cancel: None,
        });
    let elapsed = started.elapsed();
    match timeout {
        Err(LocatorError::Timeout { frame, .. }) => assert!(frame.is_some()),
        other => return Err(format!("expected a locator timeout, got {other:?}").into()),
    }
    assert!(
        elapsed <= Duration::from_secs(1) + CAPTURE_QUANTUM + Duration::from_millis(500),
        "timeout settled after {elapsed:?}"
    );

    // Cancelling a 30 s wait from another thread joins the waiting thread within 1.5 s.
    let token = WaitCancellation::new();
    let waiter_token = token.clone();
    let waiter_terminal = terminal.clone();
    let waiter_path = session.path().to_path_buf();
    let waiter = thread::spawn(move || -> Result<LocatorError, String> {
        let attached = waiter_terminal
            .attach(&waiter_path)
            .map_err(|e| e.to_string())?;
        let result = attached
            .get_by_text("NEVER-SHOWN")
            .map_err(|e| e.to_string())?
            .wait_for(LocatorWaitOptions {
                condition: LocatorCondition::Exists,
                timeout: Duration::from_secs(30),
                interval: Duration::from_millis(20),
                stable_for: Duration::ZERO,
                cancel: Some(waiter_token),
            });
        result
            .err()
            .ok_or_else(|| "cancelled wait succeeded".to_owned())
    });
    thread::sleep(Duration::from_millis(300));
    let cancelled_at = Instant::now();
    token.cancel();
    while !waiter.is_finished() {
        assert!(
            cancelled_at.elapsed() < Duration::from_millis(1500),
            "cancelled wait did not return"
        );
        thread::sleep(Duration::from_millis(5));
    }
    let latency = cancelled_at.elapsed();
    let error = waiter.join().map_err(|_| "waiter panicked")??;
    assert!(
        matches!(&error, LocatorError::Cancelled { frame: Some(_), .. }),
        "{error:?}"
    );
    println!("locator cancellation latency: {latency:?}");
    // No capture is left outstanding: an immediate follow-up capture succeeds.
    assert!(session.get_by_text("END")?.count()? == 1);

    session.stop(StopRequest::default(), Some(Duration::from_secs(5)))?;

    // Absent succeeds once the text is erased and returns that frame.
    let mut erasing = terminal.start(
        LaunchSpec::new(shell(
            "printf LOADING; sleep 1; printf '\\r\\033[K'; sleep 30",
        ))
        .geometry(40, 5),
    )?;
    erasing
        .get_by_text("LOADING")?
        .wait(Duration::from_secs(5), Duration::from_millis(20))?;
    let absent = erasing
        .get_by_text("LOADING")?
        .wait_for(LocatorWaitOptions {
            condition: LocatorCondition::Absent,
            timeout: Duration::from_secs(5),
            interval: Duration::from_millis(20),
            stable_for: Duration::ZERO,
            cancel: None,
        })?;
    assert!(absent.matches.is_empty());
    assert!(!absent.frame.state_hash.is_empty());
    erasing.stop(StopRequest::default(), Some(Duration::from_secs(5)))?;
    Ok(())
}
