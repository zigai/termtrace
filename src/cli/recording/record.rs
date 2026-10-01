use super::*;
use std::fs;
use termtrace_model::execution::session::RecordingFailurePolicy;

pub(in crate::cli) fn run_record(
    response: &mut ResponseWriter,
    arguments: RecordArgs,
) -> Result<ExitCode> {
    if arguments.original_native {
        return super::original::run(response, arguments);
    }

    if arguments.detach.is_some() {
        return super::start::run_detached_record(response, arguments);
    }

    let RecordArgs {
        original_native: _,
        original_fps: _,
        original_backend: _,
        resolved_ghostty_profile_provenance,
        on_recording_error,
        out,
        detach: _,

        startup_timeout_ms: _,
        until_idle: _,
        until_screen: _,
        until_screen_regex: _,
        timeout: _,
        json: _,
        no_input,
        session_alias,
        restart_source,
        restart_overrides,
        import_source_kind,
        import_source_byte_count,
        import_source_blake3,
        import_source,
        recording: arguments,
    } = arguments;
    let out = out.ok_or_else(|| miette::miette!("--out is required for foreground recording"))?;

    let import_provenance = match (
        import_source_kind,
        import_source_byte_count,
        import_source_blake3,
        import_source,
    ) {
        (Some(source_kind), Some(byte_count), Some(blake3), Some(source)) => {
            if source_kind != "ansi_stream" || blake3.len() != 64 {
                return Err(miette::miette!("invalid internal ANSI import provenance"));
            }

            Some(ImportProvenance {
                source_kind,
                byte_count,
                blake3,
                source: ByteString::from(source.as_os_str().as_bytes()),
            })
        }
        (None, None, None, None) => None,
        _ => {
            return Err(miette::miette!(
                "incomplete internal ANSI import provenance"
            ));
        }
    };

    let supplied_environment = if arguments.environment_spec_stdin {
        if !no_input {
            return Err(miette::miette!(
                "--environment-spec-stdin requires --no-input so stdin cannot be consumed twice"
            ));
        }

        let mut bytes = Vec::new();
        io::stdin()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .into_diagnostic()?;

        Some(
            parse_environment_spec(&bytes, Path::new("<environment-spec-stdin>"))
                .into_diagnostic()?,
        )
    } else if let Some(path) = arguments.environment_spec.as_deref() {
        Some(read_environment_spec(path).into_diagnostic()?)
    } else if arguments.clear_env
        || !arguments.copy_env.is_empty()
        || !arguments.unset_env.is_empty()
        || arguments.color_policy.is_some()
    {
        Some(EnvironmentSpec {
            base: if arguments.clear_env {
                EnvironmentBase::Clear
            } else {
                EnvironmentBase::Inherit
            },
            copy: arguments.copy_env.iter().cloned().collect(),
            unset: arguments.unset_env.iter().cloned().collect(),
            color: arguments.color_policy.map_or(ColorPolicy::Auto, Into::into),
            ..EnvironmentSpec::default()
        })
    } else {
        None
    };

    if let Some(predecessor) = restart_source {
        if import_provenance.is_some() {
            return Err(miette::miette!(
                "restart cannot carry ANSI import provenance"
            ));
        }

        let overrides = restart_overrides
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .into_diagnostic()?
            .unwrap_or_default();

        let options = restarted_record_options(
            out,
            session_alias,
            &predecessor,
            &overrides,
            supplied_environment,
        )?;

        return report_record_outcome(run_with_response(response, &options)?);
    }

    let durability_policy = durability_policy_for(&arguments)?;

    if !matches!(arguments.provenance, ProvenancePolicyArg::Custom)
        && !arguments.provenance_environment.is_empty()
    {
        return Err(miette::miette!(
            "--provenance-env requires --provenance custom"
        ));
    }

    let provenance_policy = ProvenancePolicy {
        kind: arguments.provenance.into(),
        environment: arguments.provenance_environment.iter().cloned().collect(),
    };

    let environment = supplied_environment.unwrap_or_default();

    let overrides = GhosttyVisualOverrides {
        font_family: arguments.font_family.clone(),
        font_size: arguments.font_size.clone(),
        padding_x: arguments.padding_x,
        padding_y: arguments.padding_y,
        foreground: arguments.foreground.clone(),
        background: arguments.background.clone(),
    };
    let needs_profile = arguments.ghostty_config.is_some()
        || !overrides.is_empty()
        || matches!(arguments.geometry, GeometryMode::Fit);

    let ghostty_profile = if let Some(provenance) = resolved_ghostty_profile_provenance {
        if !overrides.is_empty() {
            return Err(miette::miette!(
                "resolved native profile cannot carry additional visual overrides"
            ));
        }

        let path = arguments
            .ghostty_config
            .as_deref()
            .ok_or_else(|| miette::miette!("resolved native profile requires a config file"))?;

        Some(
            restore_ghostty_profile(
                fs::read(path).into_diagnostic()?,
                serde_json::from_str(&provenance).into_diagnostic()?,
            )
            .into_diagnostic()?,
        )
    } else if needs_profile {
        Some(
            resolve_ghostty_profile(arguments.ghostty_config.as_deref(), overrides)
                .into_diagnostic()?,
        )
    } else {
        None
    };

    let (geometry, geometry_provenance) = resolve_record(&arguments, ghostty_profile.as_ref())?;

    let cell_width_px = geometry
        .pixel_width
        .checked_div(geometry.cols)
        .ok_or_else(|| miette::miette!("resolved terminal has no columns"))?;

    let cell_height_px = geometry
        .pixel_height
        .checked_div(geometry.rows)
        .ok_or_else(|| miette::miette!("resolved terminal has no rows"))?;

    let colors = ghostty_profile
        .as_ref()
        .map_or_else(builtin_terminal_colors, terminal_colors)
        .into_diagnostic()?;

    let outcome = run_with_response(
        response,
        &RecordOptions {
            failure_policy: on_recording_error.map_or(
                if no_input {
                    RecordingFailurePolicy::Stop
                } else {
                    RecordingFailurePolicy::Preserve
                },
                Into::into,
            ),
            output_directory: out,
            creation_alias: session_alias,
            lineage: None,
            import_provenance,
            tmux_pane: None,
            command: arguments.command,
            working_directory: arguments.cwd,
            cols: geometry.cols,
            rows: geometry.rows,
            cell_width_px,
            cell_height_px,
            geometry_provenance,
            ghostty_profile,
            term: arguments.term,
            environment,
            colors,
            forward_stdin: !no_input,
            continue_after_output_mirror_closes: true,
            durability_policy,
            provenance_policy,
            resource_limits: ResourceLimits {
                output_bytes: arguments.output_byte_budget,
                max_duration_ms: arguments
                    .max_duration
                    .map(|d| return u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
                idle_timeout_ms: arguments
                    .idle_timeout
                    .map(|d| return u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
            },
        },
    )?;

    return report_record_outcome(outcome);
}

pub(super) fn durability_policy_for(arguments: &RecordingArgs) -> Result<DurabilityPolicy> {
    let has_threshold = arguments.durability_max_interval_ms.is_some()
        || arguments.durability_max_uncommitted_bytes.is_some();

    if !matches!(arguments.durability, DurabilityModeArg::Bounded) && has_threshold {
        return Err(miette::miette!(
            "--durability-max-interval-ms and --durability-max-uncommitted-bytes require --durability bounded"
        ));
    }

    let interval_ms = arguments
        .durability_max_interval_ms
        .unwrap_or(DEFAULT_DURABILITY_MAX_INTERVAL_MS);

    let bytes = arguments
        .durability_max_uncommitted_bytes
        .unwrap_or(DEFAULT_DURABILITY_MAX_UNCOMMITTED_BYTES);

    if matches!(arguments.durability, DurabilityModeArg::Bounded)
        && (interval_ms == 0 || bytes == 0)
    {
        return Err(miette::miette!(
            "bounded durability thresholds must be greater than zero"
        ));
    }
    return Ok(arguments.durability.policy(interval_ms, bytes));
}

pub(super) fn run_with_response(
    response: &mut ResponseWriter,
    options: &RecordOptions,
) -> Result<RecordOutcome> {
    let mut output = response_output(response);

    return record_with_output(options, &mut output).into_diagnostic();
}

pub(super) fn report_record_outcome(outcome: RecordOutcome) -> Result<ExitCode> {
    writeln!(io::stderr(), "recording: {}", outcome.event_log.display()).into_diagnostic()?;

    if outcome.resource_limit.is_some() {
        return Ok(ExitCode::FAILURE);
    }

    if let Some(signal) = outcome.signal_name.as_deref() {
        return match signal {
            "Interrupt" | "SIGINT" | "INT" => Ok(ExitCode::from(130)),
            "Terminate" | "Terminated" | "SIGTERM" | "TERM" => Ok(ExitCode::from(143)),
            "Kill" | "Killed" | "SIGKILL" | "KILL" => Ok(ExitCode::from(137)),
            "Pipe" | "BrokenPipe" | "Broken pipe" | "SIGPIPE" | "PIPE" => Ok(ExitCode::from(141)),
            "Hangup" | "SIGHUP" | "HUP" => Ok(ExitCode::from(129)),
            "Quit" | "SIGQUIT" | "QUIT" => Ok(ExitCode::from(131)),
            _ => Ok(ExitCode::FAILURE),
        };
    }
    return outcome.exit_code.map_or_else(
        || return Ok(ExitCode::FAILURE),
        |code| return u8::try_from(code).map(ExitCode::from).into_diagnostic(),
    );
}
