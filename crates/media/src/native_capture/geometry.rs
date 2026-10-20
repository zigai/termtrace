use super::{
    process::{
        ghostty_overrides, spawn_geometry_probe, spawn_xvfb, wait_for_json, wait_for_success,
        wait_for_x_socket, DisplayLease,
    },
    profile::materialize,
    Error, Result, MAX_WINDOW_PADDING_POINTS, MIN_VIEWPORT_HEIGHT, MIN_VIEWPORT_WIDTH,
};
use std::{collections::HashMap, env::temp_dir, fs, path::Path, time::Duration};
use termtrace_engine::{
    analysis::profile::ResolvedGhosttyProfile,
    recording::{resolve_ghostty_profile, restore_ghostty_profile},
};
use termtrace_evidence::{
    artifact::{create_private_directory, write_secure},
    event_log::EventLogCursor,
    session::read_manifest,
};
use termtrace_model::execution::layout::{self, SessionLayout};
use termtrace_model::{
    artifact::visual::{GeometryProbeAttempt, GeometrySolveReport, GHOSTTY_EFFECTIVE_CONFIG_NAME},
    execution::event::{Event, OperationStatus},
    execution::session::{
        CaptureTarget, GeometrySource, GhosttyVisualOverrides, SessionGeometry, WindowPadding,
    },
    value::geometry::TerminalGeometryReport,
};

/// Resolve the native capture viewport requested by a session or explicit caller override.
///
/// An explicit viewport is returned unchanged. Otherwise the recorded Ghostty profile is restored
/// and the viewport is solved to reproduce the session's canonical terminal grid.
/// `helper` is the Termtrace executable providing the geometry-probe subprocess protocol.
///
/// # Errors
///
/// Returns an error when the session manifest cannot be read, the recorded Ghostty profile is
/// unsupported or cannot be restored, or no probed viewport reproduces the session's terminal grid.
pub fn native_capture_target(
    helper: &Path,
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
            return CaptureTarget::new(
                manifest
                    .geometry
                    .pixel_width
                    .saturating_add(128)
                    .max(MIN_VIEWPORT_WIDTH),
                manifest
                    .geometry
                    .pixel_height
                    .saturating_add(128)
                    .max(MIN_VIEWPORT_HEIGHT),
            );
        });

    let profile = if let Some(provenance) = &manifest.ghostty_visual {
        if provenance.replay_profile != layout::GHOSTTY_REPLAY_PROFILE {
            return Err(Error::InvalidReplay {
                reason: format!(
                    "unsupported recorded Ghostty profile path `{}`",
                    provenance.replay_profile
                ),
                source: None,
            });
        }

        let path = SessionLayout::new(session).ghostty_replay_profile();
        let bytes = fs::read(&path).map_err(|source| return Error::Filesystem { path, source })?;

        restore_ghostty_profile(bytes, provenance.clone())?
    } else {
        resolve_ghostty_profile(None, GhosttyVisualOverrides::default())?
    };

    let report = solve_ghostty_geometry(
        helper,
        manifest.geometry.cols,
        manifest.geometry.rows,
        initial,
        &profile,
    )?;

    return Ok((report.selected, Some(report)));
}

pub(super) fn session_canvas(session: &Path, target: CaptureTarget) -> Result<CaptureTarget> {
    let manifest = read_manifest(session)?;
    let initial = (manifest.geometry.cols, manifest.geometry.rows);
    let mut largest = initial;
    let mut intents = HashMap::new();
    let mut cursor = EventLogCursor::open(SessionLayout::new(session).event_log())?;

    while let Some(record) = cursor.next_record()? {
        match record.event {
            Event::ResizeIntent {
                operation_id,
                cols,
                rows,
                ..
            } => {
                intents.insert(operation_id, (cols, rows));
            }
            Event::ResizeResult {
                operation_id,
                status: OperationStatus::Applied,
                ..
            } => {
                if let Some((cols, rows)) = intents.remove(&operation_id) {
                    largest = (largest.0.max(cols), largest.1.max(rows));
                }
            }
            Event::ResizeResult { operation_id, .. } => {
                intents.remove(&operation_id);
            }
            _ => {}
        }
    }

    return Ok(CaptureTarget {
        width: scaled_axis(target.width, largest.0, initial.0)?,
        height: scaled_axis(target.height, largest.1, initial.1)?,
        padding: target.padding,
    });
}

fn scaled_axis(size: u16, largest: u16, initial: u16) -> Result<u16> {
    if largest <= initial || initial == 0 {
        return Ok(size);
    }

    let scaled = (u32::from(size) * u32::from(largest)).div_ceil(u32::from(initial));

    return u16::try_from(scaled).map_err(|source| {
        return Error::InvalidReplay {
            reason: "native capture display for the largest session grid overflowed".to_owned(),
            source: Some(Box::new(source)),
        };
    });
}

/// Measure the terminal geometry produced by one Ghostty viewport and replay profile.
///
/// The probe runs in an isolated display. Failed probes retain their staging directory in
/// [`Error::CaptureFailed`] for diagnosis.
/// `helper` is the Termtrace executable providing the geometry-probe subprocess protocol.
///
/// # Errors
///
/// Returns `Error::InvalidReplay` for a viewport smaller than the native capture minimum,
/// `Error::CaptureFailed` retaining the staging directory when the probe fails, and a filesystem
/// error when the staging directory cannot be created or removed.
pub fn probe_ghostty_geometry(
    helper: &Path,
    target: CaptureTarget,
    profile: &ResolvedGhosttyProfile,
) -> Result<TerminalGeometryReport> {
    if target.width < MIN_VIEWPORT_WIDTH || target.height < MIN_VIEWPORT_HEIGHT {
        return Err(Error::InvalidReplay {
            reason: format!(
                "fitted geometry requires a viewport of at least {MIN_VIEWPORT_WIDTH}x{MIN_VIEWPORT_HEIGHT} pixels"
            ),
            source: None,
        });
    }

    let staging = temp_dir().join(format!("termtrace-geometry-{}", uuid::Uuid::now_v7()));
    create_private_directory(&staging)?;

    match probe_staged(helper, target, profile, &staging) {
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
/// `helper` is the Termtrace executable providing the geometry-probe subprocess protocol.
///
/// # Errors
///
/// Returns an error when the required grid is empty, a probe fails, or the measured viewports
/// cannot reproduce `required_cols` by `required_rows` exactly.
pub fn solve_ghostty_geometry(
    helper: &Path,
    required_cols: u16,
    required_rows: u16,
    initial: CaptureTarget,
    profile: &ResolvedGhosttyProfile,
) -> Result<GeometrySolveReport> {
    return solve_geometry_with(required_cols, required_rows, initial, |target| {
        return probe_ghostty_geometry(helper, target, profile);
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
            source: None,
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

        let (width, padding_x) = next_axis(
            &axis_samples(&attempts, |target, report| {
                return (
                    target.width,
                    target.padding.x,
                    report.cols,
                    report.cell_width,
                    report.text_width,
                );
            }),
            required_cols,
        )
        .ok_or_else(|| {
            return solver_error(required_cols, required_rows, &attempts, "width overflowed");
        })?;
        let (height, padding_y) = next_axis(
            &axis_samples(&attempts, |target, report| {
                return (
                    target.height,
                    target.padding.y,
                    report.rows,
                    report.cell_height,
                    report.text_height,
                );
            }),
            required_rows,
        )
        .ok_or_else(|| {
            return solver_error(required_cols, required_rows, &attempts, "height overflowed");
        })?;
        target = CaptureTarget {
            width: width.max(MIN_VIEWPORT_WIDTH),
            height: height.max(MIN_VIEWPORT_HEIGHT),
            padding: WindowPadding {
                x: padding_x.unwrap_or(0),
                y: padding_y.unwrap_or(0),
            },
        };
    }

    return Err(solver_error(
        required_cols,
        required_rows,
        &attempts,
        "eight-attempt bound exhausted",
    ));
}

pub(super) fn validate_capture(
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
            ), source: None,
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
            source: None,
        });
    }
    return Ok(());
}

fn probe_staged(
    helper: &Path,
    target: CaptureTarget,
    profile: &ResolvedGhosttyProfile,
    staging: &Path,
) -> Result<TerminalGeometryReport> {
    write_secure(
        &staging.join(GHOSTTY_EFFECTIVE_CONFIG_NAME),
        &profile.effective_config,
    )?;

    let root = materialize(&staging.join("ghostty-profile"), &profile.replay_config)?;
    let lease = DisplayLease::acquire()?;
    let display = format!(":{}", lease.number);
    let scaling = if target.width >= 2880 || target.height >= 1800 {
        Some(super::capture::DisplayScaling::from_scale(2.0)?)
    } else if target.width >= 2560 || target.height >= 1440 {
        Some(super::capture::DisplayScaling::from_scale(1.5)?)
    } else {
        None
    };

    let mut xvfb = spawn_xvfb(
        Some(helper),
        &lease,
        target.width,
        target.height,
        staging,
        scaling.map(|s| return s.dpi),
    )?;
    wait_for_x_socket(&lease, &mut xvfb, Duration::from_secs(5))?;

    let report_path = staging.join("geometry.json");
    let mut ghostty = spawn_geometry_probe(
        helper,
        staging,
        &display,
        &report_path,
        &ghostty_overrides(target.padding),
        &root,
        scaling,
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
            source: None,
        });
    }
    return Ok(());
}

#[derive(Clone, Copy, Debug)]
struct AxisSample {
    size: u16,
    padding: Option<u16>,
    cells: u16,
    cell: u16,
    text: u16,
}

impl AxisSample {
    fn is_pinned(&self, samples: &[AxisSample]) -> bool {
        return self.text > self.size
            || samples.iter().any(|other| {
                return other.padding == self.padding
                    && other.text == self.text
                    && other.size.abs_diff(self.size) >= self.cell;
            });
    }
}

fn axis_samples(
    attempts: &[GeometryProbeAttempt],
    project: impl Fn(&CaptureTarget, &TerminalGeometryReport) -> (u16, u16, u16, u16, u16),
) -> Vec<AxisSample> {
    return attempts
        .iter()
        .map(|attempt| {
            let (size, padding, cells, cell, text) = project(&attempt.target, &attempt.report);

            return AxisSample {
                size,
                padding: (!attempt.target.padding.is_none()).then_some(padding),
                cells,
                cell,
                text,
            };
        })
        .collect();
}

fn next_axis(samples: &[AxisSample], required: u16) -> Option<(u16, Option<u16>)> {
    let current = *samples.last()?;
    if current.cells == required {
        return Some((current.size, current.padding));
    }

    let padding_mode = current.padding.is_some_and(|padding| return padding > 0);
    if !padding_mode && !current.is_pinned(samples) {
        let overhead = current.size.saturating_sub(current.text);
        let size = required.checked_mul(current.cell)?.checked_add(overhead)?;

        return Some((size, current.padding));
    }

    let size = if padding_mode {
        current.size
    } else {
        let overhead = samples
            .iter()
            .filter(|sample| return sample.padding.is_none() && !sample.is_pinned(samples))
            .map(|sample| return sample.size.saturating_sub(sample.text))
            .min()
            .unwrap_or(current.cell);

        current
            .text
            .checked_add(overhead)?
            .checked_add(current.cell)?
    };

    let padding = i32::from(current.padding.unwrap_or(0));
    // Ghostty converts padding points to pixels by display scale, so the ratio is measured from
    // two padded probes and starts at 1.5 pixels per side, the 96 DPI value.
    let (pixels, points) = samples
        .iter()
        .filter(|sample| return sample.size == size)
        .find_map(|sample| {
            let other = i32::from(sample.padding.filter(|other| return *other > 0)?);
            let pixels = (i32::from(sample.text) - i32::from(current.text)).abs();

            return (other != padding && pixels > 0)
                .then(|| return (pixels, (padding - other).abs()));
        })
        .unwrap_or((3, 1));

    let surplus = i32::from(current.text) - i32::from(required) * i32::from(current.cell);
    let step = match surplus * points / pixels {
        0 => surplus.signum(),
        step => step,
    };

    let next = (padding + step).clamp(1, i32::from(MAX_WINDOW_PADDING_POINTS));

    return Some((size, Some(u16::try_from(next).ok()?)));
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
        source: None,
    };
}
