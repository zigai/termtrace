//! Original recording from outside the workspace: exact grid, named background attach, live
//! resize, published capture.

use std::{env, error::Error, path::PathBuf, sync::atomic::AtomicBool, time::Duration};
use termtrace_sdk::{
    contracts::ArtifactProvenance,
    input::MutationGuard,
    session::{
        CellGrid, OriginalGeometry, OriginalLaunchSpec, PixelCanvas, SessionTarget, StopRequest,
    },
    ErrorKind, Terminal,
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
    let terminal = Terminal::new(&binary, &evidence_root);

    let mut spec = OriginalLaunchSpec::new(["/bin/sh", "-c", "while :; do sleep 0.05; done"]);
    spec.geometry = OriginalGeometry::Grid(CellGrid { cols: 80, rows: 24 });
    spec.canvas = Some(PixelCanvas {
        width: 1280,
        height: 720,
    });
    spec.startup_timeout = Duration::from_secs(60);
    let state = evidence_root.join("state");
    spec.alias = Some("consumer-original".to_owned());
    spec.state_root = Some(state.clone());
    let started = terminal.start_original(&spec, &AtomicBool::new(false))?;
    let session_id = started.session().session_id();
    let _background = started.detach();
    let mut original = terminal.attach_original_target(
        &SessionTarget::Name("consumer-original".to_owned()),
        Some(&state),
    )?;
    assert_eq!(original.session().session_id(), session_id);

    original
        .session()
        .resize(100, 30, &MutationGuard::default())?;
    let rejected = original
        .session()
        .resize(400, 200, &MutationGuard::default())
        .err()
        .ok_or("an out-of-canvas resize must be rejected")?;
    assert_eq!(rejected.code(), "exceeds_native_canvas");

    let recording = original.finish(StopRequest::default(), Duration::from_secs(60))?;
    assert_eq!(
        recording.capture.provenance,
        ArtifactProvenance::OriginalExecution
    );
    assert!(!recording.canonical_success);

    let error = terminal
        .start_original(
            &OriginalLaunchSpec::new(Vec::<String>::new()),
            &AtomicBool::new(false),
        )
        .err()
        .ok_or("an empty command must be rejected")?;
    assert_eq!(error.kind(), ErrorKind::InvalidOperation);
    println!("original recording published");

    return Ok(());
}
