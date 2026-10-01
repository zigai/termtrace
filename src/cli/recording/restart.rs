use super::*;
use termtrace_model::execution::layout;

pub(in crate::cli) fn run_restart(
    response: &mut ResponseWriter,
    arguments: RestartArgs,
) -> Result<ExitCode> {
    let registry = SessionRegistry::open_default().into_diagnostic()?;
    let overrides = overrides_from_args(&arguments)?;
    let encoded_overrides = serde_json::to_string(&overrides).into_diagnostic()?;
    let predecessor_entry = registry.resolve(&arguments.name).into_diagnostic()?;
    let predecessor = predecessor_entry.session_path();
    let timeout = millis_or(arguments.timeout_ms, defaults::STARTUP_TIMEOUT);
    let StopOutcome::Completed(completion) = attach_session(&predecessor)?
        .stop(StopRequest::default(), Some(timeout))
        .into_diagnostic()?
    else {
        return Err(miette::miette!(
            "predecessor stop did not wait for completion"
        ));
    };

    let predecessor_manifest_bytes =
        fs_err::read(SessionLayout::new(&predecessor).manifest()).into_diagnostic()?;

    let expected_lineage = SessionLineage {
        predecessor_session_id: predecessor_entry.session_id,
        predecessor_manifest_hash: manifest_hash(&predecessor_manifest_bytes),
        predecessor_final_event_hash: completion.final_event_hash,
        reason: "restart".to_owned(),
    };

    let paths = registry.allocate_detached_paths().into_diagnostic()?;
    let executable = env::current_exe().into_diagnostic()?;
    let mut recorder = spawn_detached(
        &executable,
        record_arguments(
            &arguments.name,
            &paths.session,
            &predecessor,
            &encoded_overrides,
            arguments.environment_spec.as_deref(),
        ),
        &paths,
        None,
    )
    .into_diagnostic()?;

    let deadline = Instant::now() + timeout;

    match await_ready(&paths.session, &mut recorder, deadline).into_diagnostic()? {
        Readiness::Live | Readiness::Completed(_) => {}
        Readiness::Exited(status) => {
            return Err(miette::miette!(
                "restart successor exited before readiness with {status}; alias remains bound to predecessor; recoverable session candidate: {}; stdout log: {}; stderr log: {}",
                paths.session.display(),
                paths.stdout_log.display(),
                paths.stderr_log.display()
            ));
        }
        Readiness::TimedOut => {
            return Err(miette::miette!(
                "restart successor did not become ready within {} ms; alias remains bound to predecessor; recoverable orphan: {}; stdout log: {}; stderr log: {}",
                timeout.as_millis(),
                paths.session.display(),
                paths.stdout_log.display(),
                paths.stderr_log.display()
            ));
        }
    }

    let successor_manifest = read_manifest(&paths.session).into_diagnostic()?;
    if successor_manifest.lineage.as_ref() != Some(&expected_lineage) {
        return Err(miette::miette!(
            "successor lineage does not match the completed predecessor; alias remains unchanged; recoverable orphan: {}",
            paths.session.display()
        ));
    }

    let successor = registry
        .retarget(
            &arguments.name,
            predecessor_entry.session_id,
            &paths.session,
        )
        .map_err(|error| {
            return miette::miette!(
                "successor became ready but alias retargeting failed; alias remains unchanged; recoverable orphan: {}; stdout log: {}; stderr log: {}; {error}",
                paths.session.display(),
                paths.stdout_log.display(),
                paths.stderr_log.display(),
            );
        })?;

    let predecessor_display = predecessor.display().to_string();
    let successor_display = successor.session_path().display().to_string();

    let payload = serde_json::json!({
        "alias": arguments.name,
        "predecessor": {
            "canonical_session": predecessor_entry.canonical_session,
            "canonical_session_display": predecessor_display,
            "session_id": predecessor_entry.session_id,
        },
        "successor": {
            "canonical_session": successor.canonical_session,
            "canonical_session_display": successor_display,
            "session_id": successor.session_id,
            "recorder_pid": successor.recorder_pid,
            "recorder_process_start_ticks": successor.recorder_process_start_ticks,
        },
        "stdout_log": paths.stdout_log,
        "stderr_log": paths.stderr_log,
        "ready": true,
    });

    if arguments.json {
        return write_json_pretty(response, &payload);
    }
    return write_stdout(response, format!("Restarted session {}\n", arguments.name));
}

pub(super) fn record_arguments(
    alias: &str,
    session: &Path,
    predecessor: &Path,
    overrides: &str,
    environment_spec: Option<&Path>,
) -> Vec<OsString> {
    let mut arguments = vec![
        OsString::from("record"),
        OsString::from("--no-input"),
        OsString::from("--out"),
        session.as_os_str().to_os_string(),
        OsString::from("--session-alias"),
        OsString::from(alias),
        OsString::from("--restart-source"),
        predecessor.as_os_str().to_os_string(),
        OsString::from("--restart-overrides"),
        OsString::from(overrides),
    ];

    if let Some(path) = environment_spec {
        arguments.push(OsString::from("--environment-spec"));
        arguments.push(path.as_os_str().to_os_string());
    }

    arguments.push(OsString::from("--"));
    arguments.push(OsString::from("/bin/true"));

    return arguments;
}

pub(super) fn restarted_record_options(
    output_directory: PathBuf,
    creation_alias: Option<String>,
    predecessor: &Path,
    overrides: &RestartOverrides,
    environment_override: Option<EnvironmentSpec>,
) -> Result<RecordOptions> {
    let manifest_bytes =
        fs_err::read(SessionLayout::new(predecessor).manifest()).into_diagnostic()?;

    let manifest = read_manifest(predecessor).into_diagnostic()?;
    let completion = wait_for_completion(predecessor, Duration::ZERO).into_diagnostic()?;
    let event_log =
        EventLogSummary::read(predecessor.join(&manifest.event_log)).into_diagnostic()?;

    let working_directory = manifest
        .working_directory
        .as_ref()
        .map(|value| return PathBuf::from(OsString::from_vec(value.as_bytes().to_vec())))
        .ok_or_else(|| miette::miette!("predecessor has no effective working directory"))?;

    if manifest.geometry.cols == 0
        || manifest.geometry.rows == 0
        || manifest.geometry.pixel_width % manifest.geometry.cols != 0
        || manifest.geometry.pixel_height % manifest.geometry.rows != 0
    {
        return Err(miette::miette!(
            "predecessor pixel geometry is not an integral cell grid"
        ));
    }

    let ghostty_profile = manifest
        .ghostty_visual
        .as_ref()
        .map(|provenance| {
            if provenance.replay_profile != layout::GHOSTTY_REPLAY_PROFILE {
                return Err(miette::miette!(
                    "unsupported recorded Ghostty profile path `{}`",
                    provenance.replay_profile
                ));
            }
            return fs_err::read(SessionLayout::new(predecessor).ghostty_replay_profile())
                .map(|replay_profile| {
                    return restore_ghostty_profile(replay_profile, provenance.clone());
                })
                .into_diagnostic()?
                .into_diagnostic();
        })
        .transpose()?;

    let term = event_log.header.term.clone();
    let environment = if let Some(environment) = environment_override {
        let effective = EffectiveEnvironment::resolve(&environment, &term).into_diagnostic()?;
        if !overrides.allow_environment_change
            && !matches_predecessor_environment(effective, &manifest, predecessor)?
        {
            return Err(miette::miette!(
                "supplied environment does not match the predecessor commitment; pass --allow-environment-change for an intentional successor override"
            ));
        }

        environment
    } else {
        environment(&manifest, &term, predecessor)?
    };

    let mut options = RecordOptions {
        failure_policy: manifest.failure_policy,
        output_directory,
        creation_alias,
        lineage: Some(SessionLineage {
            predecessor_session_id: manifest.session_id,
            predecessor_manifest_hash: manifest_hash(&manifest_bytes),
            predecessor_final_event_hash: completion.final_event_hash,
            reason: "restart".to_owned(),
        }),
        import_provenance: None,
        tmux_pane: None,
        command: manifest
            .command
            .iter()
            .map(|value| {
                return OsString::from_vec(value.as_bytes().to_vec());
            })
            .collect(),
        working_directory: Some(working_directory),
        cols: manifest.geometry.cols,
        rows: manifest.geometry.rows,
        cell_width_px: manifest.geometry.pixel_width / manifest.geometry.cols,
        cell_height_px: manifest.geometry.pixel_height / manifest.geometry.rows,
        geometry_provenance: manifest.geometry_provenance,
        ghostty_profile,
        term,
        environment,
        colors: event_log.header.colors,
        forward_stdin: false,
        continue_after_output_mirror_closes: false,
        durability_policy: manifest.durability_policy,
        provenance_policy: manifest.execution_provenance.policy,
        resource_limits: ResourceLimits {
            output_bytes: if overrides.clear_output_byte_budget {
                None
            } else {
                overrides
                    .output_byte_budget
                    .or(manifest.resource_limits.output_bytes)
            },
            max_duration_ms: if overrides.clear_max_duration {
                None
            } else {
                overrides
                    .max_duration_ms
                    .or(manifest.resource_limits.max_duration_ms)
            },
            idle_timeout_ms: if overrides.clear_idle_timeout {
                None
            } else {
                overrides
                    .idle_timeout_ms
                    .or(manifest.resource_limits.idle_timeout_ms)
            },
        },
    };
    apply_restart_overrides(&mut options, overrides)?;

    return Ok(options);
}

pub(super) fn environment(
    manifest: &SessionManifest,
    term: &str,
    predecessor: &Path,
) -> Result<EnvironmentSpec> {
    let policy = &manifest.child_environment;
    let mut environment = EnvironmentSpec {
        base: policy.base,
        copy: policy.copied_names.clone(),
        unset: policy.unset_names.clone(),
        color: policy.color,
        ..EnvironmentSpec::default()
    };

    for name in &policy.explicit_names {
        let value = env::var_os(name).ok_or_else(|| {
            return miette::miette!(
                "exact restart requires environment variable `{name}` to be supplied again"
            );
        })?;
        environment
            .set
            .insert(name.clone(), ByteString::from(value.as_os_str().as_bytes()));
    }

    let effective = EffectiveEnvironment::resolve(&environment, term).into_diagnostic()?;
    if !matches_predecessor_environment(effective, manifest, predecessor)? {
        return Err(miette::miette!(
            "current environment does not match the predecessor's committed effective environment; exact restart refused"
        ));
    }
    return Ok(environment);
}

fn matches_predecessor_environment(
    mut effective: EffectiveEnvironment,
    manifest: &SessionManifest,
    predecessor: &Path,
) -> Result<bool> {
    // Older recordings predate the recorder-owned instrumentation variable.
    if effective.policy().effective_digest == manifest.child_environment.effective_digest {
        return Ok(true);
    }

    effective
        .set_recording_session(predecessor)
        .into_diagnostic()?;

    return Ok(effective.policy().effective_digest == manifest.child_environment.effective_digest);
}

pub(super) fn overrides_from_args(arguments: &RestartArgs) -> Result<RestartOverrides> {
    if arguments
        .command
        .first()
        .is_some_and(|value| return value.is_empty())
    {
        return Err(miette::miette!("restart command must not be empty"));
    }
    return Ok(RestartOverrides {
        cwd: arguments
            .cwd
            .as_ref()
            .map(|path| return ByteString::from(path.as_os_str().as_bytes())),
        cols: arguments.cols,
        rows: arguments.rows,
        cell_width: arguments.cell_width,
        cell_height: arguments.cell_height,
        term: arguments.term.clone(),
        durability: arguments.durability,
        durability_max_interval_ms: arguments.durability_max_interval_ms,
        durability_max_uncommitted_bytes: arguments.durability_max_uncommitted_bytes,
        provenance: arguments.provenance,
        provenance_environment: arguments.provenance_environment.clone(),
        output_byte_budget: arguments.output_byte_budget,
        clear_output_byte_budget: arguments.clear_output_byte_budget,
        max_duration_ms: arguments
            .max_duration
            .map(|d| return u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
        clear_max_duration: arguments.clear_max_duration,
        idle_timeout_ms: arguments
            .idle_timeout
            .map(|d| return u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
        clear_idle_timeout: arguments.clear_idle_timeout,
        allow_environment_change: arguments.allow_environment_change,
        command: (!arguments.command.is_empty()).then(|| {
            return arguments
                .command
                .iter()
                .map(|value| return ByteString::from(value.as_os_str().as_bytes()))
                .collect();
        }),
    });
}

pub(super) fn apply_restart_overrides(
    options: &mut RecordOptions,
    overrides: &RestartOverrides,
) -> Result<()> {
    if let Some(cwd) = &overrides.cwd {
        options.working_directory =
            Some(PathBuf::from(OsString::from_vec(cwd.as_bytes().to_vec())));
    }

    if let Some(command) = &overrides.command {
        options.command = command
            .iter()
            .map(|value| return OsString::from_vec(value.as_bytes().to_vec()))
            .collect();
    }

    options.cols = overrides.cols.unwrap_or(options.cols);
    options.rows = overrides.rows.unwrap_or(options.rows);
    options.cell_width_px = overrides.cell_width.unwrap_or(options.cell_width_px);
    options.cell_height_px = overrides.cell_height.unwrap_or(options.cell_height_px);

    if options.cols == 0
        || options.rows == 0
        || options.cell_width_px == 0
        || options.cell_height_px == 0
    {
        return Err(miette::miette!(
            "restart geometry values must be greater than zero"
        ));
    }

    if overrides.cols.is_some()
        || overrides.rows.is_some()
        || overrides.cell_width.is_some()
        || overrides.cell_height.is_some()
    {
        options.geometry_provenance = GeometryProvenance::default();
    }

    if let Some(term) = &overrides.term {
        options.term.clone_from(term);
    }

    if overrides.durability_max_interval_ms == Some(0)
        || overrides.durability_max_uncommitted_bytes == Some(0)
    {
        return Err(miette::miette!(
            "bounded durability thresholds must be greater than zero"
        ));
    }

    if let Some(mode) = overrides.durability {
        if !matches!(mode, DurabilityModeArg::Bounded)
            && (overrides.durability_max_interval_ms.is_some()
                || overrides.durability_max_uncommitted_bytes.is_some())
        {
            return Err(miette::miette!(
                "durability thresholds require --durability bounded"
            ));
        }

        let interval_ms = overrides
            .durability_max_interval_ms
            .or(options.durability_policy.maximum_interval_ms)
            .unwrap_or(50);

        let bytes = overrides
            .durability_max_uncommitted_bytes
            .or(options.durability_policy.maximum_uncommitted_bytes)
            .unwrap_or(65_536);
        options.durability_policy = mode.policy(interval_ms, bytes);
    } else if overrides.durability_max_interval_ms.is_some()
        || overrides.durability_max_uncommitted_bytes.is_some()
    {
        if !matches!(options.durability_policy.mode, DurabilityMode::Bounded) {
            return Err(miette::miette!(
                "durability thresholds require bounded predecessor policy or --durability bounded"
            ));
        }

        if let Some(value) = overrides.durability_max_interval_ms {
            options.durability_policy.maximum_interval_ms = Some(value);
        }

        if let Some(value) = overrides.durability_max_uncommitted_bytes {
            options.durability_policy.maximum_uncommitted_bytes = Some(value);
        }
    }

    if !overrides.provenance_environment.is_empty()
        && !matches!(overrides.provenance, Some(ProvenancePolicyArg::Custom))
    {
        return Err(miette::miette!(
            "--provenance-env requires --provenance custom"
        ));
    }

    if let Some(kind) = overrides.provenance {
        options.provenance_policy = ProvenancePolicy {
            kind: kind.into(),
            environment: overrides.provenance_environment.iter().cloned().collect(),
        };
    }

    return Ok(());
}
