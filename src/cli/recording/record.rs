use super::*;

pub(in crate::cli) fn run_record(
    response: &mut ResponseWriter,
    arguments: RecordArgs,
) -> Result<ExitCode> {
    let RecordArgs {
        out,
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
            .unwrap_or_else(empty_restart_overrides);
        let options = restarted_record_options(
            out,
            session_alias,
            &predecessor,
            &overrides,
            supplied_environment,
        )?;
        return report_record_outcome(record_for_response(response, &options)?);
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
    let ghostty_profile = if needs_profile {
        Some(
            resolve_ghostty_profile(arguments.ghostty_config.as_deref(), overrides)
                .into_diagnostic()?,
        )
    } else {
        None
    };
    let (geometry, geometry_provenance) =
        resolve_record_geometry(&arguments, ghostty_profile.as_ref())?;
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
        .map_or_else(builtin_terminal_colors, terminal_colors_for_profile)
        .into_diagnostic()?;
    let outcome = record_for_response(
        response,
        &RecordOptions {
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
            continue_after_output_mirror_closes: false,
            durability_policy,
            provenance_policy,
            resource_limits: ResourceLimits {
                output_bytes: arguments.output_byte_budget,
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
    let interval = arguments
        .durability_max_interval_ms
        .unwrap_or(DEFAULT_DURABILITY_MAX_INTERVAL_MS);
    let bytes = arguments
        .durability_max_uncommitted_bytes
        .unwrap_or(DEFAULT_DURABILITY_MAX_UNCOMMITTED_BYTES);
    if matches!(arguments.durability, DurabilityModeArg::Bounded) && (interval == 0 || bytes == 0) {
        return Err(miette::miette!(
            "bounded durability thresholds must be greater than zero"
        ));
    }
    return Ok(arguments.durability.policy(interval, bytes));
}

pub(super) fn record_for_response(
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
    return match (outcome.exit_code, outcome.signal_name) {
        (Some(code), None) => u8::try_from(code).map(ExitCode::from).into_diagnostic(),
        _ => Ok(ExitCode::FAILURE),
    };
}
