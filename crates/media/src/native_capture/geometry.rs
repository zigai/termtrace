use std::{env::temp_dir, fs, path::Path, time::Duration};

use termtrace_engine::analysis::profile::ResolvedGhosttyProfile;
use termtrace_evidence::{
    artifact::{create_private_directory, write_secure},
    session::read_manifest,
};
use termtrace_model::{
    artifact::visual::{GHOSTTY_EFFECTIVE_CONFIG_NAME, GeometryProbeAttempt, GeometrySolveReport},
    execution::session::{
        CaptureTarget, GHOSTTY_REPLAY_PROFILE_NAME, GeometrySource, GhosttyVisualOverrides,
        SessionGeometry,
    },
    value::geometry::TerminalGeometryReport,
};

use super::{
    Error, Result,
    process::{
        DisplayLease, ghostty_overrides, spawn_geometry_probe, spawn_xvfb, wait_for_json,
        wait_for_success, wait_for_x_socket,
    },
    profile::{materialize_profile, resolve_ghostty_profile, restore_ghostty_profile},
};

/// Resolve the native capture viewport requested by a session or explicit caller override.
///
/// An explicit viewport is returned unchanged. Otherwise the recorded Ghostty profile is restored
/// and the viewport is solved to reproduce the session's canonical terminal grid.
pub fn native_capture_target(
    session: &Path,
    viewport: Option<CaptureTarget>,
) -> Result<(CaptureTarget, Option<GeometrySolveReport>)> {
    if let Some(target) = viewport {
        return Ok((target, None));
    }
    let manifest = read_manifest(session)?;
    let initial = manifest
        .geometry_provenance
        .capture_target
        .unwrap_or_else(|| {
            return CaptureTarget {
                width: manifest.geometry.pixel_width.saturating_add(128).max(320),
                height: manifest.geometry.pixel_height.saturating_add(128).max(200),
            };
        });
    let profile = if let Some(provenance) = &manifest.ghostty_visual {
        if provenance.replay_profile != GHOSTTY_REPLAY_PROFILE_NAME {
            return Err(Error::InvalidReplay {
                reason: format!(
                    "unsupported recorded Ghostty profile path `{}`",
                    provenance.replay_profile
                ),
            });
        }
        let path = session.join(GHOSTTY_REPLAY_PROFILE_NAME);
        let bytes = fs::read(&path).map_err(|source| return Error::Filesystem { path, source })?;
        restore_ghostty_profile(bytes, provenance.clone())?
    } else {
        resolve_ghostty_profile(None, GhosttyVisualOverrides::default())?
    };
    let report = solve_ghostty_geometry(
        manifest.geometry.cols,
        manifest.geometry.rows,
        initial,
        &profile,
    )?;
    return Ok((report.selected, Some(report)));
}

/// Measure the terminal geometry produced by one Ghostty viewport and replay profile.
///
/// The probe runs in an isolated display. Failed probes retain their staging directory in
/// [`Error::CaptureFailed`] for diagnosis.
pub fn probe_ghostty_geometry(
    target: CaptureTarget,
    profile: &ResolvedGhosttyProfile,
) -> Result<TerminalGeometryReport> {
    if target.width < 320 || target.height < 200 {
        return Err(Error::InvalidReplay {
            reason: "fitted geometry requires a viewport of at least 320x200 pixels".to_owned(),
        });
    }
    let staging = temp_dir().join(format!("termtrace-geometry-{}", uuid::Uuid::now_v7()));
    create_private_directory(&staging)?;
    match probe_staged(target, profile, &staging) {
        Ok(fitted) => {
            fs::remove_dir_all(&staging).map_err(|source| {
                return Error::Filesystem {
                    path: staging,
                    source,
                };
            })?;
            return Ok(fitted);
        }
        Err(source) => {
            return Err(Error::CaptureFailed {
                evidence: staging,
                source: Box::new(source),
            });
        }
    }
}

/// Find a Ghostty viewport that exactly reproduces the required terminal cell grid.
///
/// The returned report records every measured viewport and the selected exact fit.
pub fn solve_ghostty_geometry(
    required_cols: u16,
    required_rows: u16,
    initial: CaptureTarget,
    profile: &ResolvedGhosttyProfile,
) -> Result<GeometrySolveReport> {
    return solve_geometry_with(required_cols, required_rows, initial, |target| {
        return probe_ghostty_geometry(target, profile);
    });
}

pub(super) fn solve_geometry_with(
    required_cols: u16,
    required_rows: u16,
    initial: CaptureTarget,
    mut probe: impl FnMut(CaptureTarget) -> Result<TerminalGeometryReport>,
) -> Result<GeometrySolveReport> {
    if required_cols == 0 || required_rows == 0 {
        return Err(Error::InvalidReplay {
            reason: "native geometry solver requires a nonzero canonical grid".to_owned(),
        });
    }
    let mut attempts: Vec<GeometryProbeAttempt> = Vec::new();
    let mut target = initial;
    for _ in 0..8 {
        if attempts
            .iter()
            .any(|attempt| return attempt.target == target)
        {
            return Err(solver_error(
                required_cols,
                required_rows,
                &attempts,
                "search cycled",
            ));
        }
        let report = probe(target)?;
        attempts.push(GeometryProbeAttempt { target, report });
        if (report.cols, report.rows) == (required_cols, required_rows) {
            let verified = probe(target)?;
            attempts.push(GeometryProbeAttempt {
                target,
                report: verified,
            });
            if (verified.cols, verified.rows) == (required_cols, required_rows) {
                return Ok(GeometrySolveReport {
                    required_cols,
                    required_rows,
                    attempts,
                    selected: target,
                });
            }
            continue;
        }
        let overhead_width = target.width.saturating_sub(report.text_width);
        let overhead_height = target.height.saturating_sub(report.text_height);
        let width = required_cols
            .checked_mul(report.cell_width)
            .and_then(|value| return value.checked_add(overhead_width))
            .ok_or_else(|| {
                return solver_error(required_cols, required_rows, &attempts, "width overflowed");
            })?;
        let height = required_rows
            .checked_mul(report.cell_height)
            .and_then(|value| return value.checked_add(overhead_height))
            .ok_or_else(|| {
                return solver_error(required_cols, required_rows, &attempts, "height overflowed");
            })?;
        target = CaptureTarget {
            width: width.max(320),
            height: height.max(200),
        };
    }
    return Err(solver_error(
        required_cols,
        required_rows,
        &attempts,
        "eight-attempt bound exhausted",
    ));
}

pub(super) fn validate_capture_geometry(
    expected: SessionGeometry,
    source: GeometrySource,
    reported: TerminalGeometryReport,
    viewport_width: u16,
    viewport_height: u16,
) -> Result<()> {
    validate_report(reported)?;
    if (reported.cols, reported.rows) != (expected.cols, expected.rows) {
        return Err(Error::InvalidReplay {
            reason: format!(
                "capture viewport {viewport_width}x{viewport_height} renders {}x{} cells, but recording requires {}x{} cells; recapture with `--geometry fit --viewport {viewport_width}x{viewport_height}`",
                reported.cols, reported.rows, expected.cols, expected.rows
            ),
        });
    }
    if source == GeometrySource::GhosttyFit
        && (reported.text_width, reported.text_height)
            != (expected.pixel_width, expected.pixel_height)
    {
        return Err(Error::InvalidReplay {
            reason: format!(
                "capture text area is {}x{} pixels instead of fitted {}x{} pixels",
                reported.text_width,
                reported.text_height,
                expected.pixel_width,
                expected.pixel_height
            ),
        });
    }
    return Ok(());
}

fn probe_staged(
    target: CaptureTarget,
    profile: &ResolvedGhosttyProfile,
    staging: &Path,
) -> Result<TerminalGeometryReport> {
    write_secure(
        &staging.join(GHOSTTY_EFFECTIVE_CONFIG_NAME),
        &profile.effective_config,
    )?;
    let root = materialize_profile(&staging.join("ghostty-profile"), &profile.replay_config)?;
    let lease = DisplayLease::acquire()?;
    let display = format!(":{}", lease.number);
    let mut xvfb = spawn_xvfb(&lease, target.width, target.height, staging)?;
    wait_for_x_socket(&lease, &mut xvfb, Duration::from_secs(5))?;
    let report_path = staging.join("geometry.json");
    let mut ghostty = spawn_geometry_probe(
        staging,
        &display,
        &report_path,
        &ghostty_overrides(),
        &root,
        "ghostty",
    )?;
    let report = wait_for_json(&report_path, &mut ghostty, Duration::from_secs(20))?;
    wait_for_success(&mut ghostty, Duration::from_secs(5), staging)?;
    xvfb.terminate();
    validate_report(report)?;
    return Ok(report);
}

fn validate_report(report: TerminalGeometryReport) -> Result<()> {
    if report.cols == 0
        || report.rows == 0
        || report.cell_width == 0
        || report.cell_height == 0
        || report.cols.checked_mul(report.cell_width) != Some(report.text_width)
        || report.rows.checked_mul(report.cell_height) != Some(report.text_height)
    {
        return Err(Error::InvalidReplay {
            reason: "Ghostty reported inconsistent grid, cell, and text-area dimensions".to_owned(),
        });
    }
    return Ok(());
}

fn solver_error(cols: u16, rows: u16, attempts: &[GeometryProbeAttempt], reason: &str) -> Error {
    let probes = attempts
        .iter()
        .map(|attempt| {
            return format!(
                "{}x{}px=>{}x{}",
                attempt.target.width,
                attempt.target.height,
                attempt.report.cols,
                attempt.report.rows
            );
        })
        .collect::<Vec<_>>()
        .join(", ");
    return Error::InvalidReplay {
        reason: format!(
            "could not fit native viewport to {cols}x{rows} grid ({reason}); probes: {probes}"
        ),
    };
}
