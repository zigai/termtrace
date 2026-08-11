use super::*;

const DEFAULT_DURABILITY_MAX_INTERVAL_MS: u64 = 50;
const DEFAULT_DURABILITY_MAX_UNCOMMITTED_BYTES: u64 = 65_536;

pub(super) fn run_record(arguments: RecordArgs) -> Result<ExitCode> {
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
        Some(parse_environment_spec(
            &bytes,
            Path::new("<environment-spec-stdin>"),
        )?)
    } else if let Some(path) = arguments.environment_spec.as_deref() {
        Some(read_environment_spec(path)?)
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
        return report_record_outcome(record_for_response(&options)?);
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
        Some(resolve_ghostty_profile(
            arguments.ghostty_config.as_deref(),
            overrides,
        )?)
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
    let colors = ghostty_profile.as_ref().map_or_else(
        || return resolved_terminal_colors(),
        |profile| return terminal_colors_for_profile(profile),
    )?;
    let outcome = record_for_response(&RecordOptions {
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
    })?;
    return report_record_outcome(outcome);
}

fn durability_policy_for(arguments: &RecordingArgs) -> Result<DurabilityPolicy> {
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

fn record_for_response(options: &RecordOptions) -> Result<RecordOutcome> {
    let mut output = response_output();
    return Ok(record_with_output(options, &mut output)?);
}

fn report_record_outcome(outcome: RecordOutcome) -> Result<ExitCode> {
    writeln!(io::stderr(), "recording: {}", outcome.event_log.display()).into_diagnostic()?;
    if outcome.resource_limit.is_some() {
        return Ok(ExitCode::FAILURE);
    }
    return match (outcome.exit_code, outcome.signal_name) {
        (Some(code), None) => u8::try_from(code).map(ExitCode::from).into_diagnostic(),
        _ => Ok(ExitCode::FAILURE),
    };
}

pub(super) fn run_start(arguments: StartArgs) -> Result<ExitCode> {
    let StartArgs {
        name,
        startup_timeout_ms,
        until_idle,
        until_screen,
        until_screen_regex,
        timeout,
        recording,
    } = arguments;
    let synchronization = StartSynchronization {
        until_idle,
        until_screen,
        until_screen_regex,
        timeout,
    };
    if startup_timeout_ms == 0 {
        return Err(miette::miette!(
            "--startup-timeout-ms must be greater than zero"
        ));
    }
    let _durability_policy = durability_policy_for(&recording)?;
    let registry = SessionRegistry::open_default()?;
    let executable = env::current_exe().into_diagnostic()?;
    let environment_stdin = if recording.environment_spec_stdin {
        let mut bytes = Vec::new();
        io::stdin()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .into_diagnostic()?;
        Some(bytes)
    } else {
        None
    };
    let paths = registry.allocate_detached_paths()?;
    let lease = Duration::from_millis(startup_timeout_ms).saturating_add(Duration::from_secs(5));
    let reservation = registry.reserve(&name, &paths.session, lease)?;
    let stdout = match open_detached_log(&paths.stdout_log) {
        Ok(stdout) => stdout,
        Err(error) => {
            release_start_reservation(&registry, &reservation);
            return Err(error);
        }
    };
    let stderr = match open_detached_log(&paths.stderr_log) {
        Ok(stderr) => stderr,
        Err(error) => {
            release_start_reservation(&registry, &reservation);
            return Err(error);
        }
    };
    let record_arguments = detached_record_arguments(&name, &paths.session, &recording);
    let mut command = ProcessCommand::new("setsid");
    command
        .arg(executable)
        .args(record_arguments)
        .stdin(if environment_stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let mut recorder = match command.spawn() {
        Ok(recorder) => recorder,
        Err(source) => {
            release_start_reservation(&registry, &reservation);
            return Err(source).into_diagnostic();
        }
    };
    if let Some(bytes) = environment_stdin {
        let mut stdin = recorder
            .stdin
            .take()
            .ok_or_else(|| miette::miette!("detached environment pipe was not created"))?;
        stdin.write_all(&bytes).into_diagnostic()?;
        drop(stdin);
    }
    let deadline = Instant::now() + Duration::from_millis(startup_timeout_ms);
    let mut ready_since: Option<Instant> = None;
    loop {
        if paths.session.join("completion.json").is_file() {
            return promote_completed_start(&registry, &reservation, &paths, &synchronization);
        }
        if paths.session.join("session.json").is_file()
            && registry.recorder_identity(&paths.session).is_some()
        {
            let ready_at = ready_since.get_or_insert_with(Instant::now);
            if ready_at.elapsed() >= Duration::from_millis(30) {
                let entry = registry.promote(&reservation).map_err(|error| {
                    return miette::miette!(
                        "recorder became ready but owned alias promotion failed; reservation remains for {}; stdout log: {}; stderr log: {}; {error}",
                        paths.session.display(),
                        paths.stdout_log.display(),
                        paths.stderr_log.display(),
                    );
                })?;
                return report_start_outcome(&entry, &paths, None, &synchronization);
            }
        }
        if let Some(status) = recorder.try_wait().into_diagnostic()? {
            if paths.session.join("completion.json").is_file() {
                return promote_completed_start(&registry, &reservation, &paths, &synchronization);
            }
            release_start_reservation(&registry, &reservation);
            return Err(miette::miette!(
                "detached recorder exited before publishing valid completion evidence with {status}; session candidate: {}; stdout log: {}; stderr log: {}",
                paths.session.display(),
                paths.stdout_log.display(),
                paths.stderr_log.display(),
            ));
        }
        if Instant::now() >= deadline {
            return Err(miette::miette!(
                "detached recorder did not become ready within {startup_timeout_ms} ms; its owned alias reservation and pending session remain recoverable: {}; stdout log: {}; stderr log: {}",
                paths.session.display(),
                paths.stdout_log.display(),
                paths.stderr_log.display(),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn release_start_reservation(registry: &SessionRegistry, reservation: &AliasReservation) {
    if let Err(error) = registry.release(reservation) {
        tracing::warn!(%error, alias = %reservation.alias, "failed to release startup reservation");
    }
}

#[derive(Debug)]
struct StartSynchronization {
    until_idle: Option<Duration>,
    until_screen: Option<String>,
    until_screen_regex: Option<String>,
    timeout: Duration,
}

impl StartSynchronization {
    fn requested(&self) -> bool {
        return self.until_idle.is_some()
            || self.until_screen.is_some()
            || self.until_screen_regex.is_some();
    }
}

fn promote_completed_start(
    registry: &SessionRegistry,
    reservation: &AliasReservation,
    paths: &DetachedSessionPaths,
    synchronization: &StartSynchronization,
) -> Result<ExitCode> {
    let completion = wait_for_completion(&paths.session, Duration::ZERO)?;
    let entry = registry.promote(reservation).map_err(|error| {
        return miette::miette!(
            "recorder completed but owned alias promotion failed; reservation remains for {}; stdout log: {}; stderr log: {}; {error}",
            paths.session.display(),
            paths.stdout_log.display(),
            paths.stderr_log.display(),
        );
    })?;
    return report_start_outcome(&entry, paths, Some(&completion), synchronization);
}

fn report_start_outcome(
    entry: &RegistryEntry,
    paths: &DetachedSessionPaths,
    completion: Option<&SessionCompletion>,
    synchronization: &StartSynchronization,
) -> Result<ExitCode> {
    let startup_state = match completion {
        None => "live",
        Some(completion) if completion.exit_code == Some(0) && completion.signal_name.is_none() => {
            "completed_success"
        }
        Some(_) => "completed_failure",
    };
    let canonical_session_display = entry.session_path().display().to_string();
    let mut output = serde_json::json!({
        "kind": "started",
        "startup_state": startup_state,
        "alias": entry.alias,
        "canonical_session": entry.canonical_session,
        "canonical_session_display": canonical_session_display,
        "session_id": entry.session_id,
        "recorder_pid": entry.recorder_pid,
        "recorder_process_start_ticks": entry.recorder_process_start_ticks,
        "stdout_log": paths.stdout_log,
        "stderr_log": paths.stderr_log,
        "ready": completion.is_none(),
        "completion": completion,
    });
    let mut synchronization_ok = true;
    if synchronization.requested() {
        let postcondition = if completion.is_some() {
            synchronization_ok = false;
            serde_json::json!({
                "matched": false,
                "ended": true,
                "quality": "completed_before_match",
            })
        } else {
            match start_postcondition(&entry.session_path(), synchronization) {
                Ok(postcondition) => {
                    synchronization_ok = postcondition
                        .get("matched")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false);
                    postcondition
                }
                Err(error) => {
                    synchronization_ok = false;
                    serde_json::json!({
                        "matched": false,
                        "ended": entry.session_path().join("completion.json").is_file(),
                        "error": error.to_string(),
                    })
                }
            }
        };
        output
            .as_object_mut()
            .ok_or_else(|| miette::miette!("start response is not an object"))?
            .insert("postcondition".to_owned(), postcondition);
    }
    write_json_line(&output)?;
    if !synchronization_ok {
        return Ok(ExitCode::FAILURE);
    }
    let Some(completion) = completion else {
        return Ok(ExitCode::SUCCESS);
    };
    return Ok(match (completion.exit_code, &completion.signal_name) {
        (Some(code), None) => u8::try_from(code).map_or(ExitCode::FAILURE, ExitCode::from),
        _ => ExitCode::FAILURE,
    });
}

fn start_postcondition(
    session: &Path,
    synchronization: &StartSynchronization,
) -> Result<serde_json::Value> {
    if let Some(stable_for) = synchronization.until_idle {
        let data = send_request(
            session,
            &ControlRequest::new(ControlCommand::CaptureStable {
                stable_for_ns: u64::try_from(stable_for.as_nanos()).unwrap_or(u64::MAX),
                timeout_ns: u64::try_from(synchronization.timeout.as_nanos()).unwrap_or(u64::MAX),
            }),
        )?;
        let ControlData::StableSnapshot {
            viewport_text,
            state_hash,
            cols,
            rows,
            quality,
            ..
        } = data
        else {
            return Err(miette::miette!(
                "recording actor returned an unexpected stable-capture payload"
            ));
        };
        return Ok(serde_json::json!({
            "matched": quality == StableCaptureQuality::Stable,
            "kind": "idle",
            "state": state_hash,
            "quality": quality,
            "size": [cols, rows],
            "screen": viewport_text,
        }));
    }
    let (kind, predicate) = match (
        &synchronization.until_screen,
        &synchronization.until_screen_regex,
    ) {
        (Some(text), None) => ("screen", ScreenPredicate::Contains { text: text.clone() }),
        (None, Some(pattern)) => (
            "screen_regex",
            ScreenPredicate::MatchesRegex {
                pattern: pattern.clone(),
            },
        ),
        _ => {
            return Err(miette::miette!(
                "exactly one start postcondition is required"
            ));
        }
    };
    let matched = wait_for_screen(session, predicate, Duration::ZERO, synchronization.timeout)?;
    let mut postcondition = serde_json::json!({
        "matched": true,
        "kind": kind,
        "state": matched.state_hash,
        "quality": matched.completion_quality,
        "size": [matched.cols, matched.rows],
        "ended": matched.child_completed,
    });
    if let Ok(ControlData::Snapshot {
        viewport_text,
        state_hash,
        ..
    }) = request(session, ControlCommand::Snapshot)
        && state_hash == matched.state_hash
    {
        postcondition
            .as_object_mut()
            .ok_or_else(|| miette::miette!("start postcondition is not an object"))?
            .insert(
                "screen".to_owned(),
                serde_json::Value::String(viewport_text),
            );
    }
    return Ok(postcondition);
}

pub(super) fn run_restart(arguments: RestartArgs) -> Result<ExitCode> {
    if arguments.timeout_ms == 0 {
        return Err(miette::miette!("--timeout-ms must be greater than zero"));
    }
    let registry = SessionRegistry::open_default()?;
    let overrides = restart_overrides_from_args(&arguments)?;
    let encoded_overrides = serde_json::to_string(&overrides).into_diagnostic()?;
    let predecessor_entry = registry.resolve(&arguments.name)?;
    let predecessor = predecessor_entry.session_path();
    let timeout = Duration::from_millis(arguments.timeout_ms);
    if !predecessor.join("completion.json").is_file() {
        request(&predecessor, ControlCommand::Stop)?;
    }
    let completion = wait_for_completion(&predecessor, timeout)?;
    let predecessor_manifest_bytes =
        fs_err::read(predecessor.join("session.json")).into_diagnostic()?;
    let expected_lineage = SessionLineage {
        predecessor_session_id: predecessor_entry.session_id,
        predecessor_manifest_hash: manifest_hash(&predecessor_manifest_bytes),
        predecessor_final_event_hash: completion.final_event_hash,
        reason: "restart".to_owned(),
    };

    let paths = registry.allocate_detached_paths()?;
    let stdout = open_detached_log(&paths.stdout_log)?;
    let stderr = open_detached_log(&paths.stderr_log)?;
    let executable = env::current_exe().into_diagnostic()?;
    let mut recorder = ProcessCommand::new("setsid")
        .arg(executable)
        .args(restart_record_arguments(
            &arguments.name,
            &paths.session,
            &predecessor,
            &encoded_overrides,
            arguments.environment_spec.as_deref(),
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .into_diagnostic()?;
    let deadline = Instant::now() + timeout;
    loop {
        if paths.session.join("session.json").is_file()
            && registry.recorder_identity(&paths.session).is_some()
        {
            let successor_manifest = read_manifest(&paths.session)?;
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
            return write_json_line(&serde_json::json!({
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
            }));
        }
        if let Some(status) = recorder.try_wait().into_diagnostic()? {
            return Err(miette::miette!(
                "restart successor exited before readiness with {status}; alias remains bound to predecessor; recoverable session candidate: {}; stdout log: {}; stderr log: {}",
                paths.session.display(),
                paths.stdout_log.display(),
                paths.stderr_log.display()
            ));
        }
        if Instant::now() >= deadline {
            return Err(miette::miette!(
                "restart successor did not become ready within {} ms; alias remains bound to predecessor; recoverable orphan: {}; stdout log: {}; stderr log: {}",
                arguments.timeout_ms,
                paths.session.display(),
                paths.stdout_log.display(),
                paths.stderr_log.display()
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn restart_record_arguments(
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

fn restarted_record_options(
    output_directory: PathBuf,
    creation_alias: Option<String>,
    predecessor: &Path,
    overrides: &RestartOverrides,
    environment_override: Option<EnvironmentSpec>,
) -> Result<RecordOptions> {
    let manifest_bytes = fs_err::read(predecessor.join("session.json")).into_diagnostic()?;
    let manifest = read_manifest(predecessor)?;
    let completion = wait_for_completion(predecessor, Duration::ZERO)?;
    let event_log = EventLog::read(predecessor.join(&manifest.event_log))?;
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
            if provenance.replay_profile != GHOSTTY_REPLAY_PROFILE_NAME {
                return Err(miette::miette!(
                    "unsupported recorded Ghostty profile path `{}`",
                    provenance.replay_profile
                ));
            }
            return fs_err::read(predecessor.join(GHOSTTY_REPLAY_PROFILE_NAME))
                .map(|replay_profile| {
                    return restore_ghostty_profile(replay_profile, provenance.clone());
                })
                .into_diagnostic()?
                .into_diagnostic();
        })
        .transpose()?;
    let term = event_log.header.term.clone();
    let environment = if let Some(environment) = environment_override {
        let effective = environment.resolve(&term)?;
        if !overrides.allow_environment_change
            && effective.policy().effective_digest != manifest.child_environment.effective_digest
        {
            return Err(miette::miette!(
                "supplied environment does not match the predecessor commitment; pass --allow-environment-change for an intentional successor override"
            ));
        }
        environment
    } else {
        restart_environment(&manifest, &term)?
    };
    let mut options = RecordOptions {
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
        },
    };
    apply_restart_overrides(&mut options, overrides)?;
    return Ok(options);
}

fn restart_environment(manifest: &SessionManifest, term: &str) -> Result<EnvironmentSpec> {
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
    let effective = environment.resolve(term)?;
    if effective.policy().effective_digest != policy.effective_digest {
        return Err(miette::miette!(
            "current environment does not match the predecessor's committed effective environment; exact restart refused"
        ));
    }
    return Ok(environment);
}

fn empty_restart_overrides() -> RestartOverrides {
    return RestartOverrides {
        cwd: None,
        cols: None,
        rows: None,
        cell_width: None,
        cell_height: None,
        term: None,
        durability: None,
        durability_max_interval_ms: None,
        durability_max_uncommitted_bytes: None,
        provenance: None,
        provenance_environment: Vec::new(),
        output_byte_budget: None,
        clear_output_byte_budget: false,
        allow_environment_change: false,
        command: None,
    };
}

fn restart_overrides_from_args(arguments: &RestartArgs) -> Result<RestartOverrides> {
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

fn apply_restart_overrides(
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
        let interval = overrides
            .durability_max_interval_ms
            .or(options.durability_policy.maximum_interval_ms)
            .unwrap_or(50);
        let bytes = overrides
            .durability_max_uncommitted_bytes
            .or(options.durability_policy.maximum_uncommitted_bytes)
            .unwrap_or(65_536);
        options.durability_policy = mode.policy(interval, bytes);
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

fn detached_record_arguments(
    alias: &str,
    session: &Path,
    recording: &RecordingArgs,
) -> Vec<OsString> {
    let mut arguments = vec![
        OsString::from("record"),
        OsString::from("--no-input"),
        OsString::from("--out"),
        session.as_os_str().to_os_string(),
        OsString::from("--session-alias"),
        OsString::from(alias),
    ];
    if let Some(cwd) = &recording.cwd {
        push_record_option(&mut arguments, "--cwd", cwd.as_os_str());
    }
    push_record_option(
        &mut arguments,
        "--geometry",
        OsStr::new(match recording.geometry {
            GeometryMode::Manual => "manual",
            GeometryMode::Inherit => "inherit",
            GeometryMode::Fit => "fit",
        }),
    );
    if let Some(viewport) = recording.viewport {
        push_record_option(
            &mut arguments,
            "--viewport",
            OsStr::new(&format!("{}x{}", viewport.width, viewport.height)),
        );
    }
    if let Some(path) = &recording.ghostty_config {
        push_record_option(&mut arguments, "--ghostty-config", path.as_os_str());
    }
    if let Some(value) = &recording.font_family {
        push_record_option(&mut arguments, "--font-family", OsStr::new(value));
    }
    if let Some(value) = &recording.font_size {
        push_record_option(&mut arguments, "--font-size", OsStr::new(value));
    }
    push_optional_u16(&mut arguments, "--padding-x", recording.padding_x);
    push_optional_u16(&mut arguments, "--padding-y", recording.padding_y);
    if let Some(value) = &recording.foreground {
        push_record_option(&mut arguments, "--foreground", OsStr::new(value));
    }
    if let Some(value) = &recording.background {
        push_record_option(&mut arguments, "--background", OsStr::new(value));
    }
    push_record_option(
        &mut arguments,
        "--cols",
        OsStr::new(&recording.cols.to_string()),
    );
    push_record_option(
        &mut arguments,
        "--rows",
        OsStr::new(&recording.rows.to_string()),
    );
    push_record_option(
        &mut arguments,
        "--cell-width",
        OsStr::new(&recording.cell_width.to_string()),
    );
    push_record_option(
        &mut arguments,
        "--cell-height",
        OsStr::new(&recording.cell_height.to_string()),
    );
    push_record_option(&mut arguments, "--term", OsStr::new(&recording.term));
    if let Some(path) = &recording.environment_spec {
        push_record_option(&mut arguments, "--environment-spec", path.as_os_str());
    }
    if recording.environment_spec_stdin {
        arguments.push(OsString::from("--environment-spec-stdin"));
    }
    if recording.clear_env {
        arguments.push(OsString::from("--clear-env"));
    }
    for name in &recording.copy_env {
        push_record_option(&mut arguments, "--copy-env", OsStr::new(name));
    }
    for name in &recording.unset_env {
        push_record_option(&mut arguments, "--unset-env", OsStr::new(name));
    }
    if let Some(policy) = recording.color_policy {
        push_record_option(
            &mut arguments,
            "--color-policy",
            OsStr::new(match policy {
                ColorPolicyArg::Auto => "auto",
                ColorPolicyArg::Always => "always",
                ColorPolicyArg::Never => "never",
            }),
        );
    }
    if let Some(value) = recording.output_byte_budget {
        push_record_option(
            &mut arguments,
            "--output-byte-budget",
            OsStr::new(&value.to_string()),
        );
    }
    push_record_option(
        &mut arguments,
        "--durability",
        OsStr::new(match recording.durability {
            DurabilityModeArg::Strict => "strict",
            DurabilityModeArg::Bounded => "bounded",
            DurabilityModeArg::Throughput => "throughput",
        }),
    );
    if let Some(value) = recording.durability_max_interval_ms {
        push_record_option(
            &mut arguments,
            "--durability-max-interval-ms",
            OsStr::new(&value.to_string()),
        );
    }
    if let Some(value) = recording.durability_max_uncommitted_bytes {
        push_record_option(
            &mut arguments,
            "--durability-max-uncommitted-bytes",
            OsStr::new(&value.to_string()),
        );
    }
    push_record_option(
        &mut arguments,
        "--provenance",
        OsStr::new(match recording.provenance {
            ProvenancePolicyArg::Minimal => "minimal",
            ProvenancePolicyArg::Reproducible => "reproducible",
            ProvenancePolicyArg::Custom => "custom",
        }),
    );
    for (name, capture) in &recording.provenance_environment {
        let capture = match capture {
            ProvenanceCapture::Value => "value",
            ProvenanceCapture::Hash => "hash",
            ProvenanceCapture::Name => "name",
            ProvenanceCapture::NotCaptured => "not-captured",
        };
        push_record_option(
            &mut arguments,
            "--provenance-env",
            OsStr::new(&format!("{name}={capture}")),
        );
    }
    arguments.push(OsString::from("--"));
    arguments.extend(recording.command.iter().cloned());
    return arguments;
}

fn push_record_option(arguments: &mut Vec<OsString>, name: &str, value: &OsStr) {
    arguments.push(OsString::from(name));
    arguments.push(value.to_os_string());
}

fn push_optional_u16(arguments: &mut Vec<OsString>, name: &str, value: Option<u16>) {
    if let Some(value) = value {
        push_record_option(arguments, name, OsStr::new(&value.to_string()));
    }
}

pub(super) fn open_detached_log(path: &Path) -> Result<File> {
    return OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })
        .map_err(Into::into);
}

fn resolve_record_geometry(
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
            let geometry = inherited_geometry(arguments.cell_width, arguments.cell_height)?;
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
            let fitted = probe_ghostty_geometry(target, profile)?;
            let geometry = fitted.report.session_geometry()?;
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
