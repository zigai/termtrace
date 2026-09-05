use super::*;
use std::env::current_exe;

pub(in crate::cli) fn run_geometry_probe(arguments: GeometryProbeArgs) -> Result<ExitCode> {
    let stdin = io::stdin();
    let original = tcgetattr(&stdin).into_diagnostic()?;
    let mut raw = original.clone();
    cfmakeraw(&mut raw);
    tcsetattr(&stdin, SetArg::TCSANOW, &raw).into_diagnostic()?;
    let report = query_terminal_geometry();
    let restored = tcsetattr(&stdin, SetArg::TCSANOW, &original).into_diagnostic();
    let report = match (report, restored) {
        (Ok(report), Ok(())) => report,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => return Err(error),
        (Err(primary), Err(cleanup)) => {
            return Err(miette::miette!(
                "{primary}; restoring terminal mode also failed: {cleanup}"
            ));
        }
    };
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&arguments.out)
        .into_diagnostic()?;
    serde_json::to_writer(&mut output, &report).into_diagnostic()?;
    output.write_all(b"\n").into_diagnostic()?;
    output.sync_all().into_diagnostic()?;
    return Ok(ExitCode::SUCCESS);
}

pub(super) fn query_terminal_geometry() -> Result<TerminalGeometryReport> {
    let mut output = io::stdout().lock();
    output
        .write_all(b"\x1b[14t\x1b[16t\x1b[18t")
        .and_then(|()| return output.flush())
        .into_diagnostic()?;
    drop(output);
    let stdin = io::stdin();
    let deadline = Instant::now() + GEOMETRY_QUERY_TIMEOUT;
    let mut bytes: Vec<u8> = Vec::with_capacity(128);
    let mut buffer = [0_u8; 128];
    loop {
        if let Some(report) = parse_terminal_geometry(&bytes)? {
            return Ok(report);
        }
        if Instant::now() >= deadline {
            return Err(miette::miette!(
                "timed out waiting for XTWINOPS geometry reports"
            ));
        }
        let mut descriptors = [PollFd::new(stdin.as_fd(), PollFlags::POLLIN)];
        match poll(&mut descriptors, PollTimeout::from(100_u16)) {
            Ok(0) | Err(Errno::EINTR) => continue,
            Ok(_) => {}
            Err(error) => return Err(error).into_diagnostic(),
        }
        match read(&stdin, &mut buffer) {
            Ok(0) | Err(Errno::EIO) => {
                return Err(miette::miette!("terminal closed before reporting geometry"));
            }
            Ok(length) => {
                let chunk = buffer
                    .get(..length)
                    .ok_or_else(|| miette::miette!("terminal read exceeded its buffer"))?;
                bytes.extend_from_slice(chunk);
            }
            Err(Errno::EINTR) => {}
            Err(error) => return Err(error).into_diagnostic(),
        }
        if bytes.len() > 4096 {
            return Err(miette::miette!(
                "terminal geometry response exceeded 4096 bytes"
            ));
        }
    }
}

pub(super) fn parse_terminal_geometry(bytes: &[u8]) -> Result<Option<TerminalGeometryReport>> {
    let mut text_size = None;
    let mut cell_size = None;
    let mut grid_size = None;
    for sequence in String::from_utf8_lossy(bytes).split('\x1b').skip(1) {
        let Some(body) = sequence.strip_prefix('[') else {
            continue;
        };
        let Some((parameters, _)) = body.split_once('t') else {
            continue;
        };
        let values = parameters
            .split(';')
            .map(str::parse::<u16>)
            .collect::<StdResult<Vec<u16>, _>>()
            .into_diagnostic()?;
        let Some((kind, dimensions)) = values.split_first() else {
            continue;
        };
        let (Some(first), Some(second)) = (dimensions.first(), dimensions.get(1)) else {
            continue;
        };
        match *kind {
            4 => text_size = Some((*second, *first)),
            6 => cell_size = Some((*second, *first)),
            8 => grid_size = Some((*second, *first)),
            _ => {}
        }
    }
    let (Some((text_width, text_height)), Some((cell_width, cell_height)), Some((cols, rows))) =
        (text_size, cell_size, grid_size)
    else {
        return Ok(None);
    };
    if cols == 0 || rows == 0 || cell_width == 0 || cell_height == 0 {
        return Err(miette::miette!(
            "terminal reported a zero geometry dimension"
        ));
    }
    return Ok(Some(TerminalGeometryReport {
        cols,
        rows,
        cell_width,
        cell_height,
        text_width,
        text_height,
    }));
}
pub(super) fn resolve_record_geometry(
    arguments: &RecordingArgs,
    ghostty_profile: Option<&ResolvedGhosttyProfile>,
) -> Result<(SessionGeometry, GeometryProvenance)> {
    match arguments.geometry {
        GeometryMode::Manual => {
            let pixel_width = arguments
                .cols
                .checked_mul(arguments.cell_width)
                .ok_or_else(|| miette::miette!("manual terminal width overflow"))?;
            let pixel_height = arguments
                .rows
                .checked_mul(arguments.cell_height)
                .ok_or_else(|| miette::miette!("manual terminal height overflow"))?;
            return Ok((
                SessionGeometry {
                    cols: arguments.cols,
                    rows: arguments.rows,
                    pixel_width,
                    pixel_height,
                },
                GeometryProvenance {
                    source: GeometrySource::Manual,
                    capture_target: arguments.viewport,
                },
            ));
        }
        GeometryMode::Inherit => {
            let geometry = inherited_geometry(arguments.cell_width, arguments.cell_height)
                .into_diagnostic()?;
            return Ok((
                geometry,
                GeometryProvenance {
                    source: GeometrySource::Inherited,
                    capture_target: arguments.viewport,
                },
            ));
        }
        GeometryMode::Fit => {
            let target = arguments.viewport.ok_or_else(|| {
                return miette::miette!("--geometry fit requires --viewport WIDTHxHEIGHT");
            })?;
            let profile = ghostty_profile.ok_or_else(|| {
                return miette::miette!("fitted geometry requires a resolved Ghostty profile");
            })?;
            let helper = current_exe().into_diagnostic()?;
            let report = probe_ghostty_geometry(&helper, target, profile).into_diagnostic()?;
            let geometry = SessionGeometry {
                cols: report.cols,
                rows: report.rows,
                pixel_width: report.text_width,
                pixel_height: report.text_height,
            };
            return Ok((
                geometry,
                GeometryProvenance {
                    source: GeometrySource::GhosttyFit,
                    capture_target: Some(target),
                },
            ));
        }
    }
}
