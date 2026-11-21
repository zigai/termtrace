use super::{action::frame_text, *};
use std::fmt::Write as _;

pub(in crate::cli) fn run_wait(
    response: &mut ResponseWriter,
    arguments: WaitArgs,
) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session.clone())?;
    if !matches!(resolved.phase, SessionPhase::Live | SessionPhase::Completed)
        || (arguments.require_live_stability && resolved.phase == SessionPhase::Completed)
    {
        return write_session_lifecycle(response, &resolved, "wait", ExitCode::FAILURE);
    }

    if let Some(stable_for) = arguments.idle {
        return run_idle_wait(response, &resolved, &arguments, stable_for);
    }

    let timeout = arguments.timeout.unwrap_or(defaults::WAIT_TIMEOUT);
    let attached = attach_session(&resolved.path)?;

    if let Some(output) = &arguments.output {
        if !arguments.stable_for.is_zero() {
            return Err(miette::miette!(
                "--stable-for applies only to screen predicates"
            ));
        }

        let outcome =
            wait_for_output(&resolved.path, output.as_bytes(), timeout).into_diagnostic()?;

        if arguments.json {
            let frame = attached.frame(text_projection()).into_diagnostic()?;
            let mut report = compact_wait_report(
                &arguments,
                true,
                "matched",
                serde_json::json!({"predicate": "output", "text": output}),
                &frame,
            )?;

            if arguments.report == WaitReport::Frame
                && let Some(object) = report.as_object_mut()
            {
                object.insert("event_seq".to_owned(), serde_json::json!(outcome.event_seq));
                object.insert(
                    "output_end_offset".to_owned(),
                    serde_json::json!(outcome.output_end_offset),
                );
                object.insert(
                    "monotonic_ns".to_owned(),
                    serde_json::json!(outcome.monotonic_ns),
                );
            }

            write_json_pretty(response, &report)?;
        } else {
            write_stdout(
                response,
                format!("Output matched in session {}\n", resolved.path.display()),
            )?;
        }

        return Ok(ExitCode::SUCCESS);
    }

    let predicate = parse_screen_predicate(&arguments)?;
    let outcome = attached
        .wait_screen(predicate, arguments.stable_for, timeout)
        .into_diagnostic()?;

    let satisfied = outcome.is_satisfied();

    if arguments.json {
        let mut report = compact_wait_report(
            &arguments,
            satisfied,
            &stable_enum_label(&outcome.quality)?,
            serde_json::to_value(&outcome.predicate).into_diagnostic()?,
            &outcome.frame,
        )?;

        if arguments.report == WaitReport::Frame
            && let Some(object) = report.as_object_mut()
        {
            object.insert(
                "first_match_monotonic_ns".to_owned(),
                serde_json::json!(outcome.first_match_monotonic_ns),
            );
            object.insert(
                "observed_monotonic_ns".to_owned(),
                serde_json::json!(outcome.observed_monotonic_ns),
            );
            object.insert(
                "stable_for_ns".to_owned(),
                serde_json::json!(outcome.stable_for_ns),
            );
        }

        write_json_pretty(response, &report)?;
    } else if satisfied {
        write_stdout(
            response,
            format!("Screen matched state {}\n", outcome.frame.state_hash),
        )?;
    }

    return Ok(if satisfied { ExitCode::SUCCESS } else { ExitCode::FAILURE });
}

fn text_projection() -> ScreenProjectionRequest {
    return ScreenProjectionRequest {
        detail: FrameDetail::Text,
        region: None,
    };
}

fn compact_wait_report(
    arguments: &WaitArgs,
    matched: bool,
    outcome: &str,
    predicate: serde_json::Value,
    frame: &ScreenFrame,
) -> Result<serde_json::Value> {
    let mut report = serde_json::json!({
        "matched": matched,
        "outcome": outcome,
        "predicate": predicate,
    });

    let Some(object) = report.as_object_mut() else {
        return Err(miette::miette!("wait report is not a JSON object"));
    };

    object.insert("state".to_owned(), serde_json::json!(frame.state_hash));
    object.insert(
        "size".to_owned(),
        serde_json::json!([frame.geometry.cols, frame.geometry.rows]),
    );

    if arguments.report != WaitReport::State {
        object.insert("screen".to_owned(), serde_json::json!(frame_text(frame)));
    }

    if arguments.report == WaitReport::Frame {
        object.insert("frame_hash".to_owned(), serde_json::json!(frame.frame_hash));
        object.insert(
            "frame".to_owned(),
            serde_json::to_value(frame).into_diagnostic()?,
        );
    }

    return Ok(report);
}

fn run_idle_wait(
    response: &mut ResponseWriter,
    resolved: &ResolvedSessionRef,
    arguments: &WaitArgs,
    stable_for: Duration,
) -> Result<ExitCode> {
    let capture = attach_session(&resolved.path)?
        .capture_stable(StableCaptureOptions {
            stable_for,
            timeout: arguments.timeout.unwrap_or(defaults::WAIT_TIMEOUT),
            projection: ScreenProjectionRequest::default(),
            allow_incomplete: true,
            reaction: None,
        })
        .into_diagnostic()?;

    if arguments.require_live_stability && capture.quality == StableCaptureQuality::Completed {
        let completed = resolve_session(arguments.session.clone())?;

        return write_session_lifecycle(response, &completed, "wait", ExitCode::FAILURE);
    }

    if !arguments.allow_incomplete && !capture.quality.is_settled() {
        return Err(miette::miette!(
            "stable capture ended with {}; pass --allow-incomplete to receive the typed snapshot",
            capture.quality.as_str()
        ));
    }

    let frame = &capture.frame;
    let changing_rows = capture.changing_rows.as_deref();

    if arguments.json {
        let mut report = compact_wait_report(
            arguments,
            capture.quality.is_settled(),
            capture.quality.as_str(),
            serde_json::json!({"predicate": "idle", "ms": duration_millis(stable_for)}),
            frame,
        )?;

        if let Some(object) = report.as_object_mut() {
            if let Some(rows) = changing_rows {
                object.insert("changing_rows".to_owned(), serde_json::json!(rows));
            }

            if arguments.report == WaitReport::Frame {
                object.insert(
                    "first_observed_monotonic_ns".to_owned(),
                    serde_json::json!(capture.first_observed_monotonic_ns),
                );
                object.insert(
                    "observed_monotonic_ns".to_owned(),
                    serde_json::json!(capture.observed_monotonic_ns),
                );
                object.insert(
                    "stable_for_ns".to_owned(),
                    serde_json::json!(capture.stable_for_ns),
                );
            }
        }

        if arguments.full_state
            && let (Some(fields), Some(state)) =
                (report.as_object_mut(), &capture.semantic_state_zstd)
        {
            fields.insert(
                "semantic_state_zstd".to_owned(),
                serde_json::to_value(state).into_diagnostic()?,
            );
        }

        return write_json_pretty(response, &report);
    }

    let mut summary = format!("Session idle at state {}\n", frame.state_hash);
    if let Some(rows) = changing_rows {
        writeln!(summary, "{}", changing_rows_notice(rows)).into_diagnostic()?;
    }

    return write_stdout(response, summary);
}

fn duration_millis(duration: Duration) -> u64 {
    return u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
}

pub(super) fn compact_boundaries(boundaries: &ActorBoundaries) -> serde_json::Value {
    if boundaries.durable == boundaries.observed {
        return serde_json::json!({
            "durable": true,
            "next_event_seq": boundaries.observed.next_event_seq,
            "event_hash": boundaries.observed.event_hash,
            "output_bytes": boundaries.observed.output_bytes,
        });
    }
    return serde_json::json!({
        "durable": false,
        "durable_boundary": boundaries.durable,
        "observed_boundary": boundaries.observed,
    });
}

fn parse_screen_predicate(arguments: &WaitArgs) -> Result<ScreenPredicate> {
    let mut predicate = None;
    let mut set = |pred: Result<ScreenPredicate>| -> Result<()> {
        if predicate.is_some() {
            return Err(miette::miette!("exactly one wait predicate is required"));
        }

        predicate = Some(pred?);

        return Ok(());
    };

    if let Some(text) = &arguments.screen {
        set(Ok(ScreenPredicate::Contains { text: text.clone() }))?;
    }

    if let Some(text) = &arguments.screen_absent {
        set(Ok(ScreenPredicate::Absent { text: text.clone() }))?;
    }

    if let Some(pattern) = &arguments.screen_regex {
        set(Ok(ScreenPredicate::MatchesRegex {
            pattern: pattern.clone(),
        }))?;
    }

    if let Some(value) = &arguments.row_contains {
        let (row, text) = parse_row_predicate(value)?;
        set(Ok(ScreenPredicate::RowContains { row, text }))?;
    }

    if let Some(value) = &arguments.region_contains {
        let (region, text) = parse_region_predicate(value)?;
        set(Ok(ScreenPredicate::RegionContains { region, text }))?;
    }

    if let Some(value) = &arguments.cursor_at {
        let (column, row) = parse_cursor_position(value)?;
        set(Ok(ScreenPredicate::Cursor {
            column: Some(column),
            row: Some(row),
            visible: None,
            shape: None,
        }))?;
    }

    if let Some(visible) = arguments.cursor_visible {
        set(Ok(ScreenPredicate::Cursor {
            column: None,
            row: None,
            visible: Some(visible),
            shape: None,
        }))?;
    }

    if let Some(shape) = arguments.cursor_shape {
        set(Ok(ScreenPredicate::Cursor {
            column: None,
            row: None,
            visible: None,
            shape: Some(shape.into()),
        }))?;
    }

    if let Some(mode) = arguments.mode {
        set(Ok(ScreenPredicate::Mode {
            mode: mode.into(),
            enabled: true,
        }))?;
    }

    if let Some(mode) = arguments.mode_disabled {
        set(Ok(ScreenPredicate::Mode {
            mode: mode.into(),
            enabled: false,
        }))?;
    }

    if let Some(hash) = &arguments.state_changed {
        set(Ok(ScreenPredicate::StateHash {
            hash: hash.clone(),
            comparison: StateHashComparison::Changed,
        }))?;
    }

    if let Some(hash) = &arguments.state_unchanged {
        set(Ok(ScreenPredicate::StateHash {
            hash: hash.clone(),
            comparison: StateHashComparison::Unchanged,
        }))?;
    }

    return predicate
        .ok_or_else(|| return miette::miette!("exactly one wait predicate is required"));
}
