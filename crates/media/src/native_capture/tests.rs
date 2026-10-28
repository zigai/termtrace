use super::{
    geometry::{solve_geometry_with, validate_capture, PaddingSearch},
    process::{wait_for_playback_handshake, ManagedChild},
};
use std::{process::Command, time::Duration};
use termtrace_evidence::artifact::write_secure;
use termtrace_model::{
    artifact::playback::PlaybackFailure,
    execution::session::{CaptureTarget, GeometrySource, SessionGeometry, WindowPadding},
    value::geometry::TerminalGeometryReport,
};

#[test]
fn capture_geometry_error_reports_actual_and_recorded_grids() {
    let error = validate_capture(
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
        CaptureTarget::new(1280, 720),
        PaddingSearch::Free,
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
    assert_eq!(report.selected, CaptureTarget::new(820, 440));
    assert_eq!(report.attempts.len(), 3);
    assert_eq!(report.attempts.get(1), report.attempts.get(2));
}

#[derive(Clone, Copy)]
struct SimulatedWindow {
    minimum: (u16, u16),
    chrome: (u16, u16),
    profile_padding: (u16, u16),
    pixels_per_point: (u16, u16),
    cell: (u16, u16),
}

impl SimulatedWindow {
    fn report(self, target: CaptureTarget) -> TerminalGeometryReport {
        let padding = |profile: u16, points: u16| {
            if target.padding.is_none() {
                return profile;
            }
            return 2 * points * self.pixels_per_point.0 / self.pixels_per_point.1;
        };
        let width = target.width.max(self.minimum.0)
            - self.chrome.0
            - padding(self.profile_padding.0, target.padding.x);

        let height = target.height.max(self.minimum.1)
            - self.chrome.1
            - padding(self.profile_padding.1, target.padding.y);

        let cols = width / self.cell.0;
        let rows = height / self.cell.1;

        return TerminalGeometryReport {
            cols,
            rows,
            cell_width: self.cell.0,
            cell_height: self.cell.1,
            text_width: cols * self.cell.0,
            text_height: rows * self.cell.1,
        };
    }

    fn solve(self, cols: u16, rows: u16, initial: CaptureTarget) -> CaptureTarget {
        let report = solve_geometry_with(cols, rows, initial, PaddingSearch::Free, |target| {
            return Ok(self.report(target));
        })
        .expect("solve grid below the minimum window");

        let selected = self.report(report.selected);
        assert_eq!((selected.cols, selected.rows), (cols, rows));
        assert!(report.selected.width >= self.minimum.0);
        assert!(report.selected.height >= self.minimum.1);

        return report.selected;
    }
}

const SMALL_WINDOW: SimulatedWindow = SimulatedWindow {
    minimum: (360, 130),
    chrome: (0, 50),
    profile_padding: (4, 4),
    pixels_per_point: (3, 2),
    cell: (7, 14),
};

#[test]
fn geometry_solver_pads_grids_narrower_than_the_minimum_window() {
    let selected = SMALL_WINDOW.solve(40, 18, CaptureTarget::new(608, 560));
    assert!(selected.padding.x > 0);
}

#[test]
fn geometry_solver_pads_both_axes_of_a_tiny_grid() {
    let window = SimulatedWindow {
        minimum: (360, 220),
        ..SMALL_WINDOW
    };
    let selected = window.solve(20, 6, CaptureTarget::new(368, 272));
    assert!(selected.padding.x > 0 && selected.padding.y > 0);
}

#[test]
fn geometry_solver_detects_a_minimum_window_inside_the_display() {
    let window = SimulatedWindow {
        minimum: (420, 130),
        ..SMALL_WINDOW
    };

    window.solve(50, 18, CaptureTarget::new(700, 400));
}

#[test]
fn geometry_solver_corrects_padding_for_high_density_displays() {
    let window = SimulatedWindow {
        pixels_per_point: (3, 1),
        ..SMALL_WINDOW
    };

    window.solve(30, 18, CaptureTarget::new(500, 400));
}

#[test]
fn fixed_profile_padding_is_never_overridden_and_impossible_fits_fail() {
    let padded_initial = CaptureTarget {
        padding: WindowPadding { x: 6, y: 9 },
        ..CaptureTarget::new(700, 400)
    };
    let fitted = solve_geometry_with(50, 18, padded_initial, PaddingSearch::Fixed, |target| {
        return Ok(SMALL_WINDOW.report(target));
    })
    .expect("a grid above the minimum window fits by size alone");

    let selected = SMALL_WINDOW.report(fitted.selected);
    assert_eq!((selected.cols, selected.rows), (50, 18));
    assert!(fitted
        .attempts
        .iter()
        .all(|attempt| return attempt.target.padding.is_none()));

    let mut probed = Vec::new();
    let error = solve_geometry_with(
        40,
        18,
        CaptureTarget::new(608, 560),
        PaddingSearch::Fixed,
        |target| {
            probed.push(target);

            return Ok(SMALL_WINDOW.report(target));
        },
    )
    .expect_err("a grid below the minimum window needs other padding");
    assert!(error.to_string().contains("fixed padding"), "{error}");
    assert!(probed.iter().all(|target| return target.padding.is_none()));
}

#[test]
fn geometry_solver_reports_cycles_with_measured_candidates() {
    let error = solve_geometry_with(
        100,
        25,
        CaptureTarget::new(800, 400),
        PaddingSearch::Free,
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

    let mut child =
        ManagedChild::spawn_logged("playback-test", Command::new("/bin/sleep").arg("5"))
            .expect("sleep process");
    let error = wait_for_playback_handshake(
        &timing_path,
        &error_path,
        &mut child,
        Duration::from_secs(1),
    )
    .expect_err("failure handshake must win");
    assert!(error.to_string().contains("rendered 230x56"));
}
