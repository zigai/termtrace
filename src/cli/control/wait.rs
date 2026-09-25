use super::*;

pub(in crate::cli) fn run_wait(
    response: &mut ResponseWriter,
    arguments: WaitArgs,
) -> Result<ExitCode> {
    let timeout = arguments.timeout;

    if let Some(stable_for) = arguments.idle {
        return run_idle_wait(response, arguments, stable_for);
    }

    let resolved = resolve_session(arguments.session.clone())?;
    if resolved.lifecycle != SessionPhase::Live {
        return write_session_lifecycle(response, &resolved, "wait", ExitCode::FAILURE);
    }

    let session = resolved.path;

    if let Some(output) = arguments.output {
        if !arguments.stable_for.is_zero() {
            return Err(miette::miette!(
                "--stable-for applies only to screen predicates"
            ));
        }

        let outcome = wait_for_output(&session, output.as_bytes(), timeout).into_diagnostic()?;
        if arguments.json {
            write_json_pretty(response, &outcome)?;
        } else {
            write_stdout(
                response,
                format!("Output matched in session {}\n", session.display()),
            )?;
        }

        return Ok(ExitCode::SUCCESS);
    }

    let predicate = parse_screen_predicate(&arguments)?;
    let outcome =
        wait_for_screen(session, predicate, arguments.stable_for, timeout).into_diagnostic()?;

    let matched = outcome.matched
        && if arguments.stable_for.is_zero() {
            outcome.quality == ScreenWaitQuality::MatchedUnsettled
        } else {
            outcome.quality == ScreenWaitQuality::Matched
        };

    if arguments.json {
        write_json_pretty(response, &outcome)?;
    } else if matched {
        write_stdout(
            response,
            format!("Screen matched state {}\n", outcome.frame.state_hash),
        )?;
    }

    return Ok(if matched { ExitCode::SUCCESS } else { ExitCode::FAILURE });
}

fn run_idle_wait(
    response: &mut ResponseWriter,
    arguments: WaitArgs,
    stable_for: Duration,
) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle == SessionPhase::Completed {
        if arguments.require_live_stability {
            return write_session_lifecycle(response, &resolved, "wait", ExitCode::FAILURE);
        }
        return write_completed_stable_snapshot(response, &resolved, arguments.full_state);
    }

    if resolved.lifecycle != SessionPhase::Live {
        return write_session_lifecycle(response, &resolved, "wait", ExitCode::FAILURE);
    }

    let data = send_request(
        resolved.path,
        &ControlRequest::new(ControlCommand::CaptureStable {
            stable_for_ns: u64::try_from(stable_for.as_nanos()).unwrap_or(u64::MAX),
            timeout_ns: u64::try_from(arguments.timeout.as_nanos()).unwrap_or(u64::MAX),
            projection: ScreenProjectionRequest::default(),
        }),
    )
    .into_diagnostic()?;

    let ControlData::StableSnapshot {
        frame,
        first_observed_monotonic_ns,
        observed_monotonic_ns,
        stable_for_ns,
        semantic_state_zstd: _,
        quality,
    } = &data
    else {
        return Err(miette::miette!(
            "recording actor returned an unexpected stable-capture payload"
        ));
    };
    if !arguments.allow_incomplete && *quality != StableCaptureQuality::Stable {
        return Err(miette::miette!(
            "stable capture ended with {}; pass --allow-incomplete to receive the typed snapshot",
            quality.as_str()
        ));
    }

    if arguments.full_state {
        if arguments.json {
            return write_json_pretty(response, &data);
        }
        return write_json_line(response, &data);
    }

    let payload = serde_json::json!({
        "kind": "stable_snapshot",
        "quality": quality,
        "session_state": "unknown",
        "state": frame.state_hash,
        "state_hash": frame.state_hash,
        "frame_hash": frame.frame_hash,
        "geometry": frame.geometry,
        "first_observed_monotonic_ns": first_observed_monotonic_ns,
        "observed_monotonic_ns": observed_monotonic_ns,
        "stable_for_ns": stable_for_ns,
        "frame": frame,
    });

    if arguments.json {
        return write_json_pretty(response, &payload);
    }
    return write_stdout(
        response,
        format!("Session idle at state {}\n", frame.state_hash),
    );
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

pub(super) fn write_completed_stable_snapshot(
    response: &mut ResponseWriter,
    resolved: &ResolvedSessionRef,
    full_state: bool,
) -> Result<ExitCode> {
    let observation =
        observe_session_with_options(&resolved.path, StatePoint::LatestPresented, false)
            .into_diagnostic()?;

    if full_state {
        return write_json_line(
            response,
            &serde_json::json!({
                "kind": "stable_snapshot",
                "quality": "completed",
                "observation": observation,
            }),
        );
    }
    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "stable_snapshot",
            "quality": "completed",
            "state_hash": observation.state_hash,
            "viewport_text": observation.frame.viewport_text,
            "geometry": {
                "cols": observation.frame.cols,
                "rows": observation.frame.rows,
            },
            "cursor": {
                "column": observation.frame.cursor_column,
                "row": observation.frame.cursor_row,
                "visible": observation.frame.cursor_visible,
            },
            "modes": observation.frame.modes,
            "boundary": observation.boundary,
        }),
    );
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
