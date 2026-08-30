use super::*;

pub(in crate::cli) fn run_wait(
    response: &mut ResponseWriter,
    arguments: WaitArgs,
) -> Result<ExitCode> {
    if arguments.idle.is_none()
        && (arguments.allow_incomplete || arguments.require_live_stability || arguments.full_state)
    {
        return Err(miette::miette!(
            "--allow-incomplete, --require-live-stability, and --full-state require --idle"
        ));
    }
    let timeout = arguments.timeout;
    if let Some(stable_for) = arguments.idle {
        return run_idle_wait(response, arguments, stable_for);
    }
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle != RegistryLifecycle::Live {
        return write_session_lifecycle(response, &resolved, "wait", ExitCode::FAILURE);
    }
    let session = resolved.path;
    if let Some(output) = arguments.output {
        if !arguments.stable_for.is_zero() {
            return Err(miette::miette!(
                "--stable-for applies only to screen predicates"
            ));
        }
        return write_json_line(
            response,
            &wait_for_output(session, output.as_bytes(), timeout).into_diagnostic()?,
        );
    }
    let screen = arguments.screen.or(arguments.screen_compat);
    let predicate = match (
        screen,
        arguments.screen_absent,
        arguments.screen_regex,
        arguments.row_contains,
        arguments.region_contains,
        arguments.cursor_at,
        arguments.cursor_visible,
        arguments.cursor_shape,
        arguments.mode,
        arguments.mode_disabled,
        arguments.state_changed,
        arguments.state_unchanged,
    ) {
        (Some(text), None, None, None, None, None, None, None, None, None, None, None) => {
            ScreenPredicate::Contains { text }
        }
        (None, Some(text), None, None, None, None, None, None, None, None, None, None) => {
            ScreenPredicate::Absent { text }
        }
        (None, None, Some(pattern), None, None, None, None, None, None, None, None, None) => {
            ScreenPredicate::MatchesRegex { pattern }
        }
        (None, None, None, Some(value), None, None, None, None, None, None, None, None) => {
            let (row, text) = parse_row_predicate(&value)?;
            ScreenPredicate::RowContains { row, text }
        }
        (None, None, None, None, Some(value), None, None, None, None, None, None, None) => {
            let (region, text) = parse_region_predicate(&value)?;
            ScreenPredicate::RegionContains { region, text }
        }
        (None, None, None, None, None, Some(value), None, None, None, None, None, None) => {
            let (column, row) = parse_cursor_position(&value)?;
            ScreenPredicate::Cursor {
                column: Some(column),
                row: Some(row),
                visible: None,
                shape: None,
            }
        }
        (None, None, None, None, None, None, Some(visible), None, None, None, None, None) => {
            ScreenPredicate::Cursor {
                column: None,
                row: None,
                visible: Some(visible),
                shape: None,
            }
        }
        (None, None, None, None, None, None, None, Some(shape), None, None, None, None) => {
            ScreenPredicate::Cursor {
                column: None,
                row: None,
                visible: None,
                shape: Some(shape.into()),
            }
        }
        (None, None, None, None, None, None, None, None, Some(mode), None, None, None) => {
            ScreenPredicate::Mode {
                mode: mode.into(),
                enabled: true,
            }
        }
        (None, None, None, None, None, None, None, None, None, Some(mode), None, None) => {
            ScreenPredicate::Mode {
                mode: mode.into(),
                enabled: false,
            }
        }
        (None, None, None, None, None, None, None, None, None, None, Some(hash), None) => {
            ScreenPredicate::StateHash {
                hash,
                comparison: StateHashComparison::Changed,
            }
        }
        (None, None, None, None, None, None, None, None, None, None, None, Some(hash)) => {
            ScreenPredicate::StateHash {
                hash,
                comparison: StateHashComparison::Unchanged,
            }
        }
        _ => return Err(miette::miette!("exactly one wait predicate is required")),
    };
    let outcome =
        wait_for_screen(session, predicate, arguments.stable_for, timeout).into_diagnostic()?;
    let matched = outcome.matched
        && if arguments.stable_for.is_zero() {
            outcome.quality == ScreenWaitQuality::MatchedUnsettled
        } else {
            outcome.quality == ScreenWaitQuality::Matched
        };
    write_json_line(response, &outcome)?;
    return Ok(if matched {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    });
}

fn run_idle_wait(
    response: &mut ResponseWriter,
    arguments: WaitArgs,
    stable_for: Duration,
) -> Result<ExitCode> {
    let resolved = resolve_session(arguments.session)?;
    if resolved.lifecycle == RegistryLifecycle::Completed {
        if arguments.require_live_stability {
            return write_session_lifecycle(response, &resolved, "wait", ExitCode::FAILURE);
        }
        return write_completed_stable_snapshot(response, &resolved, arguments.full_state);
    }
    if resolved.lifecycle != RegistryLifecycle::Live {
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
        return write_json_line(response, &data);
    }
    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "stable_snapshot",
            "quality": quality,
            "state_hash": frame.state_hash,
            "frame_hash": frame.frame_hash,
            "geometry": frame.geometry,
            "frame": frame,
            "first_observed_monotonic_ns": first_observed_monotonic_ns,
            "observed_monotonic_ns": observed_monotonic_ns,
            "stable_for_ns": stable_for_ns,
        }),
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
