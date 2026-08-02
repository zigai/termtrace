use std::{env::temp_dir, path::Path, time::Duration};

use uuid::Uuid;

use super::{
    FittedGeometry, GHOSTTY_EFFECTIVE_CONFIG_NAME, GeometryProbeAttempt, GeometrySolveReport,
};
use crate::{
    Error, Result,
    artifact::{create_private_directory, write_secure},
    geometry::TerminalGeometryReport,
    profile::{
        ResolvedGhosttyProfile, materialize_profile, resolve_ghostty_profile,
        restore_ghostty_profile,
    },
    session::{
        CaptureTarget, GHOSTTY_REPLAY_PROFILE_NAME, GeometrySource, GhosttyVisualOverrides,
        SessionGeometry, SessionManifest, read_manifest,
    },
};

use super::process::{
    DisplayLease, ghostty_overrides, spawn_geometry_probe, spawn_xvfb, wait_for_json,
    wait_for_success, wait_for_x_socket,
};

/// Resolve the recorded native profile and solve an exact-grid capture target.
///
/// A caller-supplied viewport is returned as a strict assertion and is still
/// independently checked by capture. An absent viewport runs the bounded
/// geometry solver against the exact recorded grid.
///
/// # Errors
///
/// Returns an error for tampered profile provenance or failed native probes.
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
    let profile = capture_profile(session, &manifest)?;
    let report = solve_ghostty_geometry(
        manifest.geometry.cols,
        manifest.geometry.rows,
        initial,
        &profile,
    )?;
    return Ok((report.selected, Some(report)));
}

fn capture_profile(session: &Path, manifest: &SessionManifest) -> Result<ResolvedGhosttyProfile> {
    let Some(provenance) = &manifest.ghostty_visual else {
        return resolve_ghostty_profile(None, GhosttyVisualOverrides::default());
    };
    if provenance.replay_profile != GHOSTTY_REPLAY_PROFILE_NAME {
        return Err(Error::InvalidReplay {
            reason: format!(
                "unsupported recorded Ghostty profile path `{}`",
                provenance.replay_profile
            ),
        });
    }
    let path = session.join(GHOSTTY_REPLAY_PROFILE_NAME);
    let replay_profile = fs_err::read(&path).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?;
    return restore_ghostty_profile(replay_profile, provenance.clone()).map_err(|error| {
        return Error::InvalidReplay {
            reason: format!(
                "recorded Ghostty profile at {} is invalid: {error}",
                path.display()
            ),
        };
    });
}

/// Measure Ghostty's real grid and cell dimensions at a capture viewport.
///
/// # Errors
///
/// Returns an error when Ghostty, Xvfb, D-Bus, terminal queries, or temporary
/// evidence persistence fails.
pub fn probe_ghostty_geometry(
    target: CaptureTarget,
    profile: &ResolvedGhosttyProfile,
) -> Result<FittedGeometry> {
    if target.width < 320 || target.height < 200 {
        return Err(Error::InvalidReplay {
            reason: "fitted geometry requires a viewport of at least 320x200 pixels".to_owned(),
        });
    }
    let staging = temp_dir().join(format!("termtrace-geometry-{}", Uuid::now_v7()));
    create_private_directory(&staging)?;
    let result = probe_ghostty_geometry_staged(target, profile, &staging);
    match result {
        Ok(fitted) => {
            fs_err::remove_dir_all(&staging).map_err(|source| {
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

/// Find and independently verify a viewport that produces the exact canonical grid.
///
/// # Errors
///
/// Returns an error when probing fails, the search cycles, dimensions overflow,
/// or no exact grid is found within eight measured attempts.
pub fn solve_ghostty_geometry(
    required_cols: u16,
    required_rows: u16,
    initial: CaptureTarget,
    profile: &ResolvedGhosttyProfile,
) -> Result<GeometrySolveReport> {
    return solve_geometry_with(required_cols, required_rows, initial, |target| {
        return probe_ghostty_geometry(target, profile).map(|fitted| return fitted.report);
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
    let mut attempts = Vec::new();
    let mut target = initial;
    for _attempt in 0..8 {
        if attempts
            .iter()
            .any(|attempt: &GeometryProbeAttempt| return attempt.target == target)
        {
            return Err(geometry_solver_error(
                required_cols,
                required_rows,
                &attempts,
                "search cycled",
            ));
        }
        let report = probe(target)?;
        attempts.push(GeometryProbeAttempt { target, report });
        if report.cols == required_cols && report.rows == required_rows {
            // A second measurement at the chosen target distinguishes a stable
            // exact fit from a transient geometry report.
            let verified = probe(target)?;
            attempts.push(GeometryProbeAttempt {
                target,
                report: verified,
            });
            if verified.cols == required_cols && verified.rows == required_rows {
                return Ok(GeometrySolveReport {
                    required_cols,
                    required_rows,
                    attempts,
                    selected: target,
                });
            }
            continue;
        }
        let horizontal_overhead = target.width.saturating_sub(report.text_width);
        let vertical_overhead = target.height.saturating_sub(report.text_height);
        let width = required_cols
            .checked_mul(report.cell_width)
            .and_then(|text| return text.checked_add(horizontal_overhead))
            .ok_or_else(|| {
                return geometry_solver_error(
                    required_cols,
                    required_rows,
                    &attempts,
                    "width overflowed",
                );
            })?;
        let height = required_rows
            .checked_mul(report.cell_height)
            .and_then(|text| return text.checked_add(vertical_overhead))
            .ok_or_else(|| {
                return geometry_solver_error(
                    required_cols,
                    required_rows,
                    &attempts,
                    "height overflowed",
                );
            })?;
        target = CaptureTarget {
            width: width.max(320),
            height: height.max(200),
        };
    }
    return Err(geometry_solver_error(
        required_cols,
        required_rows,
        &attempts,
        "eight-attempt bound exhausted",
    ));
}

fn geometry_solver_error(
    required_cols: u16,
    required_rows: u16,
    attempts: &[GeometryProbeAttempt],
    reason: &str,
) -> Error {
    let measured = attempts
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
            "could not fit native viewport to {required_cols}x{required_rows} grid ({reason}); probes: {measured}"
        ),
    };
}

fn probe_ghostty_geometry_staged(
    target: CaptureTarget,
    profile: &ResolvedGhosttyProfile,
    staging: &Path,
) -> Result<FittedGeometry> {
    write_secure(
        &staging.join(GHOSTTY_EFFECTIVE_CONFIG_NAME),
        &profile.effective_config,
    )?;
    let profile_root =
        materialize_profile(&staging.join("ghostty-profile"), &profile.replay_config)?;
    let lease = DisplayLease::acquire()?;
    let display = format!(":{}", lease.number);
    let mut xvfb = spawn_xvfb(&lease, target.width, target.height, staging)?;
    wait_for_x_socket(&lease, &mut xvfb, Duration::from_secs(5))?;
    let report_path = staging.join("geometry.json");
    let overrides = ghostty_overrides();
    let mut ghostty = spawn_geometry_probe(
        staging,
        &display,
        &report_path,
        &overrides,
        &profile_root,
        "ghostty",
    )?;
    let report: TerminalGeometryReport =
        wait_for_json(&report_path, &mut ghostty, Duration::from_secs(20))?;
    wait_for_success(&mut ghostty, Duration::from_secs(5), staging)?;
    xvfb.terminate();
    // Ghostty may enforce a minimum window and report a text area larger than
    // the requested Xvfb target. Preserve that measurement for the solver; it
    // is a search signal, not a reason to discard the probe.
    let _geometry = report.session_geometry()?;
    drop(lease);
    return Ok(FittedGeometry { report });
}

pub(super) fn validate_capture_geometry(
    expected: SessionGeometry,
    source: GeometrySource,
    reported: TerminalGeometryReport,
    viewport_width: u16,
    viewport_height: u16,
) -> Result<()> {
    let actual = reported.session_geometry()?;
    if actual.cols != expected.cols || actual.rows != expected.rows {
        return Err(Error::InvalidReplay {
            reason: format!(
                "capture viewport {viewport_width}x{viewport_height} renders {}x{} cells ({}x{} text pixels), but the recording requires {}x{} cells; re-record with `--geometry fit --viewport {viewport_width}x{viewport_height}` or choose a compatible viewport/profile",
                actual.cols,
                actual.rows,
                actual.pixel_width,
                actual.pixel_height,
                expected.cols,
                expected.rows
            ),
        });
    }
    if source == GeometrySource::GhosttyFit
        && (reported.text_width != expected.pixel_width
            || reported.text_height != expected.pixel_height)
    {
        return Err(Error::InvalidReplay {
            reason: format!(
                "capture viewport reproduces the {}x{} grid but its text area is {}x{} pixels instead of the fitted {}x{} pixels; use the recorded viewport and Ghostty profile",
                expected.cols,
                expected.rows,
                reported.text_width,
                reported.text_height,
                expected.pixel_width,
                expected.pixel_height
            ),
        });
    }
    return Ok(());
}
