//! Ordinary public SDK adapter. The external runner owns expectations and certification.
use std::{env, ffi::OsString, fs, path::PathBuf, time::Duration};
use termtrace_sdk::{
    output::OutputService,
    screen::{FrameDetail, ScreenPredicate, ScreenProjectionRequest},
    session::{LaunchSpec, StopRequest},
    ActRequest, ErrorKind, MutationEffect, Retry, Terminal,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(env::var("SDK_CONFORMANCE_ROOT")?);
    let fixture: serde_json::Value =
        serde_json::from_slice(&fs::read(env::var("SDK_CONFORMANCE_FIXTURE")?)?)?;
    let mut command: Vec<OsString> = fixture["command"]
        .as_array()
        .ok_or("command")?
        .iter()
        .map(|s| OsString::from(s.as_str().unwrap()))
        .collect();
    command.push(root.join("child.pid").into_os_string());
    let timeout = Duration::from_millis(fixture["timeout_ms"].as_u64().unwrap());
    let terminal = Terminal::new(env::var("TERMTRACE_BIN")?, root.join("evidence"));
    let mut session = terminal.start(LaunchSpec::new(command).geometry(
        fixture["cols"].as_u64().unwrap() as u16,
        fixture["rows"].as_u64().unwrap() as u16,
    ))?;
    let observation = (|| -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        assert!(session
            .wait_screen(
                ScreenPredicate::Contains {
                    text: fixture["text"].as_str().unwrap().into()
                },
                Duration::ZERO,
                timeout
            )?
            .is_satisfied());
        let frame = session.frame(ScreenProjectionRequest {
            detail: FrameDetail::Text,
            region: None,
        })?;
        let text = termtrace_sdk::screen::frame_text(&frame);
        let error = session
            .act(
                ActRequest::new()
                    .text("FORBIDDEN\n")
                    .if_state(fixture["guard"].as_str().unwrap()),
            )
            .expect_err("stale guard must fail");
        assert_eq!(error.kind(), ErrorKind::StaleState);
        assert_eq!(error.effect(), MutationEffect::NotApplied);
        assert_eq!(error.retry(), Retry::NewRequest);
        let output = OutputService::read_page(session.path(), None, 4096)?;
        let output_hex: String = output.bytes.iter().map(|b| format!("{b:02x}")).collect();
        Ok(
            serde_json::json!({"version":1,"language":"rust","case_id":"foundation","session_path":session.path(),"rows":frame.geometry.rows,"cols":frame.geometry.cols,"text":text,"output_hex":output_hex,"stale":{"kind":error.kind().as_str(),"code":error.code(),"effect":error.effect(),"retry":error.retry()}}),
        )
    })();
    let cleanup = session.stop(StopRequest::default(), Some(timeout));
    if let Err(error) = &cleanup {
        eprintln!(
            "SDK adapter cleanup failed (primary observation error, if any, is retained): {error}"
        );
    }
    let mut observation = observation?;
    cleanup?;
    observation["active"] = false.into();
    fs::write(
        root.join("observation.json"),
        serde_json::to_vec_pretty(&observation)?,
    )?;
    Ok(())
}
