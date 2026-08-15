use std::{process::Command, time::Duration};

use super::{
    geometry::{solve_geometry_with, validate_capture_geometry},
    process::{ManagedChild, wait_for_playback_handshake},
};
use termtrace_evidence::artifact::write_secure;
use termtrace_model::{
    artifact::playback::PlaybackFailure,
    execution::session::{CaptureTarget, GeometrySource, SessionGeometry},
    value::geometry::TerminalGeometryReport,
};

#[test]
fn capture_geometry_error_reports_actual_and_recorded_grids() {
    let error = validate_capture_geometry(
        SessionGeometry {
            cols: 180,
            rows: 50,
            pixel_width: 1620,
            pixel_height: 900,
        },
        GeometrySource::Manual,
        TerminalGeometryReport {
            cols: 230,
            rows: 56,
            cell_width: 7,
            cell_height: 16,
            text_width: 1610,
            text_height: 896,
        },
        1620,
        900,
    )
    .expect_err("mismatched grid must fail");
    let message = error.to_string();
    assert!(message.contains("renders 230x56 cells"));
    assert!(message.contains("requires 180x50 cells"));
    assert!(message.contains("--geometry fit --viewport 1620x900"));
}

#[test]
fn geometry_solver_converges_and_independently_verifies_exact_grid() {
    let report = solve_geometry_with(
        100,
        25,
        CaptureTarget {
            width: 1280,
            height: 720,
        },
        |target| {
            let text_width = target.width.saturating_sub(20);
            let text_height = target.height.saturating_sub(40);
            return Ok(TerminalGeometryReport {
                cols: text_width / 8,
                rows: text_height / 16,
                cell_width: 8,
                cell_height: 16,
                text_width,
                text_height,
            });
        },
    )
    .expect("solve exact grid");
    assert_eq!(
        report.selected,
        CaptureTarget {
            width: 820,
            height: 440,
        }
    );
    assert_eq!(report.attempts.len(), 3);
    assert_eq!(report.attempts.get(1), report.attempts.get(2));
}

#[test]
fn geometry_solver_reports_cycles_with_measured_candidates() {
    let error = solve_geometry_with(
        100,
        25,
        CaptureTarget {
            width: 800,
            height: 400,
        },
        |target| {
            let text_width = target.width.saturating_sub(20);
            let text_height = target.height.saturating_sub(20);
            return Ok(TerminalGeometryReport {
                cols: 99,
                rows: 24,
                cell_width: 8,
                cell_height: 16,
                text_width,
                text_height,
            });
        },
    )
    .expect_err("cycle must fail");
    assert!(error.to_string().contains("search cycled"));
    assert!(error.to_string().contains("800x400px=>99x24"));
}

#[test]
fn playback_failure_handshake_preempts_timeout() {
    let temporary = tempfile::TempDir::new().expect("temporary directory");
    let timing_path = temporary.path().join("timing.json");
    let error_path = temporary.path().join("playback-error.json");
    write_secure(
        &error_path,
        &serde_json::to_vec(&PlaybackFailure {
            message: "recorded 180x50 but rendered 230x56".to_owned(),
        })
        .expect("failure JSON"),
    )
    .expect("failure handshake");
    let child = Command::new("/bin/sleep")
        .arg("5")
        .spawn()
        .expect("sleep process");
    let mut child = ManagedChild::new("playback-test", child);
    let error = wait_for_playback_handshake(
        &timing_path,
        &error_path,
        &mut child,
        Duration::from_secs(1),
    )
    .expect_err("failure handshake must win");
    assert!(error.to_string().contains("rendered 230x56"));
}
