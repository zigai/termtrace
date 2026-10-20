use super::*;
use crate::cli::{analysis::args::ShowFormat, output::stable_enum_label};
use std::io::{self, Write as _};
use termtrace_engine::observation::observe_completed_with_formats;
use termtrace_model::terminal::{
    diff::{row_moves, RowMove},
    observation::Observation,
};
use termtrace_terminal::Format as GhosttyFormat;

pub(in crate::cli) fn run_screen(
    response: &mut ResponseWriter,
    arguments: ScreenArgs,
) -> Result<ExitCode> {
    let is_historical = arguments.state.is_some()
        || arguments.state_hash.is_some()
        || arguments.event_seq.is_some()
        || arguments.at.is_some()
        || arguments.at_marker.is_some();

    if is_historical {
        let point = resolve_state_point(
            arguments.state,
            arguments.state_hash,
            arguments.event_seq,
            arguments.at,
            arguments.at_marker,
            Some(StatePoint::LatestPresented),
            "screen",
        )?;

        let session = resolve_session_path(arguments.session)?;
        let format = arguments.format.unwrap_or(if arguments.json {
            ShowFormat::Json
        } else {
            ShowFormat::Text
        });

        let engine_format = match format {
            ShowFormat::Vt => Some(GhosttyFormat::Vt),
            ShowFormat::Html => Some(GhosttyFormat::Html),
            _ => None,
        };

        let (observation, formatted) = match engine_format {
            Some(f) => {
                let (obs, rendered) =
                    observe_completed_with_formats(&session, point, arguments.include_hidden, &[f])
                        .into_diagnostic()?;

                let formatted = rendered
                    .into_iter()
                    .next()
                    .map(|(_, bytes)| return bytes)
                    .ok_or_else(|| miette::miette!("selected formatter produced no output"))?;
                (obs, Some(formatted))
            }
            None => {
                let obs = if arguments.recover_prefix {
                    observe_validated_prefix(&session, point, arguments.include_hidden)
                        .into_diagnostic()?
                } else {
                    attach_session(&session)?
                        .observe(point, arguments.include_hidden)
                        .into_diagnostic()?
                };
                (obs, None)
            }
        };

        if arguments.metadata {
            write_observation_metadata(&observation)?;
        }

        return match format {
            ShowFormat::Text => {
                write_stdout(response, format!("{}\n", observation.frame.viewport_text))
            }
            ShowFormat::Json => write_stdout(
                response,
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&observation).into_diagnostic()?
                ),
            ),
            ShowFormat::JsonCompact => write_json_line(response, &observation),
            ShowFormat::FrameJson => write_json_line(response, &observation.frame_envelope()),
            ShowFormat::Vt | ShowFormat::Html => {
                let bytes = formatted
                    .ok_or_else(|| miette::miette!("selected formatter produced no output"))?;

                write_stdout(response, bytes)
            }
        };
    }

    let resolved = resolve_session(arguments.session)?;
    let session = resolved.path.clone();
    let (mut observation, stable_hash) = if arguments.recover_prefix {
        let observation = observe_validated_prefix(
            &session,
            StatePoint::LatestPresented,
            arguments.include_hidden,
        )
        .into_diagnostic()?;

        (observation, None)
    } else {
        if !matches!(
            resolved.phase,
            SessionPhase::Live
                | SessionPhase::Completed
                | SessionPhase::Incomplete
                | SessionPhase::Stale
        ) {
            return write_session_lifecycle(response, &resolved, "screen", ExitCode::FAILURE);
        }

        let attached = attach_session(&session)?;
        let stable_hash = if arguments.stable_for.is_zero() {
            None
        } else {
            let capture = attached
                .capture_stable(StableCaptureOptions {
                    stable_for: arguments.stable_for,
                    timeout: arguments.timeout.unwrap_or(defaults::WAIT_TIMEOUT),
                    projection: hash_only_projection(),
                    allow_incomplete: false,
                    reaction: None,
                })
                .map_err(|error| {
                    let SessionError::StableCaptureIncomplete { result } = &error else {
                        return miette::miette!("live stability was not established: {error}");
                    };

                    return miette::miette!(
                        "live stability was not established before {:?}",
                        result.quality
                    );
                })?;

            (capture.quality == StableCaptureQuality::Stable).then_some(capture.frame.state_hash)
        };

        match attached.observe(StatePoint::LatestPresented, false) {
            Ok(observation) => (observation, stable_hash),
            Err(_) if resolved.phase != SessionPhase::Live => {
                return write_session_lifecycle(response, &resolved, "screen", ExitCode::FAILURE);
            }
            Err(error) => return Err(error).into_diagnostic(),
        }
    };

    let lifecycle =
        if observation.source_completed { SessionPhase::Completed } else { resolved.phase };

    if let Some(state_hash) = &stable_hash {
        if observation.state_hash != *state_hash {
            return Err(miette::miette!(
                "presented state changed between stability proof and observation; retry screen"
            ));
        }

        observation.capture_quality = CaptureQuality::Stable;
    }

    if arguments.metadata {
        write_observation_metadata(&observation)?;
    }

    if arguments.since.as_deref() == Some(observation.state_hash.as_str()) {
        let output = ScreenResponse {
            v: 1,
            ok: true,
            response_type: "screen",
            session: &session,
            lifecycle,
            state: &observation.state_hash,
            quality: observation.capture_quality,
            size: [observation.frame.cols, observation.frame.rows],
            ended: observation.source_completed,
            unchanged: Some(true),
            screen: None,
        };

        if arguments.json {
            return write_json_pretty(response, &output);
        }
        return write_stdout(
            response,
            human_screen_summary(&serde_json::to_value(&output).into_diagnostic()?),
        );
    }

    if let Some(format) = arguments.format {
        if matches!(format, ShowFormat::Vt | ShowFormat::Html) && !observation.source_completed {
            return Err(miette::miette!(
                "--format vt and --format html are supported only on completed sessions"
            ));
        }

        let (mut observation, formatted) = match format {
            ShowFormat::Vt | ShowFormat::Html => {
                let f =
                    if format == ShowFormat::Vt { GhosttyFormat::Vt } else { GhosttyFormat::Html };

                let (obs, rendered) = observe_completed_with_formats(
                    &session,
                    StatePoint::LatestPresented,
                    arguments.include_hidden,
                    &[f],
                )
                .into_diagnostic()?;

                let formatted = rendered
                    .into_iter()
                    .next()
                    .map(|(_, bytes)| return bytes)
                    .ok_or_else(|| miette::miette!("selected formatter produced no output"))?;
                (obs, Some(formatted))
            }
            _ => (observation, None),
        };

        if stable_hash.is_some() {
            observation.capture_quality = CaptureQuality::Stable;
        }

        return match format {
            ShowFormat::Text => {
                write_stdout(response, format!("{}\n", observation.frame.viewport_text))
            }
            ShowFormat::Json => write_stdout(
                response,
                format!(
                    "{}\n",
                    serde_json::to_string_pretty(&observation).into_diagnostic()?
                ),
            ),
            ShowFormat::JsonCompact => write_json_line(response, &observation),
            ShowFormat::FrameJson => write_json_line(response, &observation.frame_envelope()),
            ShowFormat::Vt | ShowFormat::Html => {
                let bytes = formatted
                    .ok_or_else(|| miette::miette!("selected formatter produced no output"))?;

                write_stdout(response, bytes)
            }
        };
    }

    let screen = match arguments.screen {
        ScreenEncoding::Exact => Some(serde_json::Value::String(observation.frame.viewport_text)),
        ScreenEncoding::Text => Some(serde_json::Value::String(
            observation
                .frame
                .viewport_text
                .trim_end_matches(|character: char| {
                    return character == ' ' || character == '\n' || character == '\r';
                })
                .to_owned(),
        )),
        ScreenEncoding::Sparse => Some(sparse(&observation.frame.viewport_text)),
        ScreenEncoding::Diff => {
            let since = arguments
                .since
                .ok_or_else(|| miette::miette!("--screen diff requires --since STATE_HASH"))?;

            let point = StatePoint::StateHash {
                state_hash: since.clone(),
            };
            let prior = if observation.source_completed {
                attach_session(&session)?
                    .observe(point, false)
                    .into_diagnostic()?
            } else {
                observe_validated_prefix(&session, point, false).into_diagnostic()?
            };

            Some(difference(
                &prior.frame.viewport_text,
                &observation.frame.viewport_text,
                since,
            ))
        }
        ScreenEncoding::None => None,
    };

    let output = ScreenResponse {
        v: 1,
        ok: true,
        response_type: "screen",
        session: &session,
        lifecycle,
        state: &observation.state_hash,
        quality: observation.capture_quality,
        size: [observation.frame.cols, observation.frame.rows],
        ended: observation.source_completed,
        unchanged: None,
        screen,
    };

    if arguments.json {
        return write_json_pretty(response, &output);
    }
    return write_stdout(
        response,
        human_screen_summary(&serde_json::to_value(&output).into_diagnostic()?),
    );
}

fn write_observation_metadata(observation: &Observation) -> Result<()> {
    writeln!(
        io::stderr(),
        "source={} resolution={} state_hash={} presented={} quality={} truth={}",
        stable_enum_label(&observation.source_kind)?,
        stable_enum_label(&observation.resolution)?,
        observation.state_hash,
        observation.presented,
        stable_enum_label(&observation.capture_quality)?,
        stable_enum_label(&observation.truth_layer)?,
    )
    .into_diagnostic()?;

    return Ok(());
}

#[derive(serde::Serialize)]
struct ScreenResponse<'response> {
    v: u32,
    ok: bool,
    #[serde(rename = "type")]
    response_type: &'static str,
    session: &'response Path,
    lifecycle: SessionPhase,
    state: &'response str,
    quality: CaptureQuality,
    size: [u16; 2],
    ended: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    unchanged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    screen: Option<serde_json::Value>,
}

#[derive(serde::Serialize)]
pub(super) struct SparseScreenResponse<'viewport> {
    encoding: &'static str,
    rows: Vec<(usize, &'viewport str)>,
    blank_rows_omitted: usize,
}

#[derive(serde::Serialize)]
pub(super) struct DiffScreenResponse<'viewport> {
    encoding: &'static str,
    since: String,
    moves: Vec<RowMove>,
    rows: Vec<(usize, &'viewport str)>,
}

pub(super) fn sparse(viewport_text: &str) -> serde_json::Value {
    let rows: Vec<(usize, &str)> = viewport_text
        .lines()
        .enumerate()
        .filter_map(|(row, text)| {
            let text = text.trim_end();

            return (!text.is_empty()).then(|| return (row, text));
        })
        .collect();
    let total_rows = viewport_text.lines().count();
    let response = SparseScreenResponse {
        encoding: "rows",
        blank_rows_omitted: total_rows.saturating_sub(rows.len()),
        rows,
    };

    #[expect(
        clippy::expect_used,
        reason = "SparseScreenResponse contains only primitive usize and string slices which cannot fail JSON serialization"
    )]
    return serde_json::to_value(&response).expect("sparse screen serialization");
}

pub(super) fn difference(before: &str, after: &str, since: String) -> serde_json::Value {
    let before_rows: Vec<&str> = before.lines().collect();
    let after_rows: Vec<&str> = after.lines().collect();
    let row_count = before_rows.len().max(after_rows.len());
    let moves = row_moves(&before_rows, &after_rows);
    let rows: Vec<(usize, &str)> = (0..row_count)
        .filter_map(|row| {
            let old = before_rows.get(row).copied().unwrap_or_default();
            let new = after_rows.get(row).copied().unwrap_or_default();
            let moved = u16::try_from(row).is_ok_and(|row| {
                return moves.iter().any(|moved| return moved.covers_later_row(row));
            });

            return (old != new && !moved).then(|| return (row, new.trim_end()));
        })
        .collect();

    let response = DiffScreenResponse {
        encoding: "diff_rows",
        since,
        moves,
        rows,
    };

    #[expect(
        clippy::expect_used,
        reason = "DiffScreenResponse contains only String and string slices which cannot fail JSON serialization"
    )]
    return serde_json::to_value(&response).expect("diff screen serialization");
}
