use super::*;

pub(in crate::cli) fn run_start(
    response: &mut ResponseWriter,
    arguments: StartArgs,
) -> Result<ExitCode> {
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
    let registry = SessionRegistry::open_default().into_diagnostic()?;
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
    let paths = registry.allocate_detached_paths().into_diagnostic()?;
    let lease = Duration::from_millis(startup_timeout_ms).saturating_add(Duration::from_secs(5));
    let reservation = registry
        .reserve(&name, &paths.session, lease)
        .into_diagnostic()?;
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
        .env_remove("TERMTRACE_RESPONSE")
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
            return promote_completed_start(
                response,
                &registry,
                &reservation,
                &paths,
                &synchronization,
            );
        }
        if paths.session.join("session.json").is_file()
            && termtrace_engine::registry::recorder_identity(&paths.session).is_some()
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
                return report_start_outcome(response, &entry, &paths, None, &synchronization);
            }
        }
        if let Some(status) = recorder.try_wait().into_diagnostic()? {
            if paths.session.join("completion.json").is_file() {
                return promote_completed_start(
                    response,
                    &registry,
                    &reservation,
                    &paths,
                    &synchronization,
                );
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

pub(super) fn release_start_reservation(
    registry: &SessionRegistry,
    reservation: &AliasReservation,
) {
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
    response: &mut ResponseWriter,
    registry: &SessionRegistry,
    reservation: &AliasReservation,
    paths: &DetachedSessionPaths,
    synchronization: &StartSynchronization,
) -> Result<ExitCode> {
    let completion = wait_for_completion(&paths.session, Duration::ZERO).into_diagnostic()?;
    let entry = registry.promote(reservation).map_err(|error| {
        return miette::miette!(
            "recorder completed but owned alias promotion failed; reservation remains for {}; stdout log: {}; stderr log: {}; {error}",
            paths.session.display(),
            paths.stdout_log.display(),
            paths.stderr_log.display(),
        );
    })?;
    return report_start_outcome(response, &entry, paths, Some(&completion), synchronization);
}

fn report_start_outcome(
    response: &mut ResponseWriter,
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
        let (matched, postcondition) = if completion.is_some() {
            (
                false,
                serde_json::json!({
                    "matched": false,
                    "ended": true,
                    "quality": "completed_before_match",
                }),
            )
        } else {
            match start_postcondition(&entry.session_path(), synchronization) {
                Ok((matched, postcondition)) => (matched, postcondition),
                Err(error) => (
                    false,
                    serde_json::json!({
                        "matched": false,
                        "ended": entry.session_path().join("completion.json").is_file(),
                        "error": error.to_string(),
                    }),
                ),
            }
        };
        synchronization_ok = matched;
        output
            .as_object_mut()
            .ok_or_else(|| miette::miette!("start response is not an object"))?
            .insert("postcondition".to_owned(), postcondition);
    }
    write_json_line(response, &output)?;
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
) -> Result<(bool, serde_json::Value)> {
    if let Some(stable_for) = synchronization.until_idle {
        let data = send_request(
            session,
            &ControlRequest::new(ControlCommand::CaptureStable {
                stable_for_ns: u64::try_from(stable_for.as_nanos()).unwrap_or(u64::MAX),
                timeout_ns: u64::try_from(synchronization.timeout.as_nanos()).unwrap_or(u64::MAX),
                projection: ScreenProjectionRequest::default(),
            }),
        )
        .into_diagnostic()?;
        let ControlData::StableSnapshot { frame, quality, .. } = data else {
            return Err(miette::miette!(
                "actor returned the wrong stable-capture payload"
            ));
        };
        let matched_ok = quality == StableCaptureQuality::Stable;
        return Ok((
            matched_ok,
            serde_json::json!({
                "matched": quality == StableCaptureQuality::Stable,
                "kind": "idle",
                "quality": quality,
                "state_hash": frame.state_hash,
                "frame_hash": frame.frame_hash,
                "frame": frame,
            }),
        ));
    }
    let (kind, predicate) = match (
        &synchronization.until_screen,
        &synchronization.until_screen_regex,
    ) {
        (Some(text), _) => ("screen", ScreenPredicate::Contains { text: text.clone() }),
        (_, Some(pattern)) => (
            "screen_regex",
            ScreenPredicate::MatchesRegex {
                pattern: pattern.clone(),
            },
        ),
        (None, None) => unreachable!("start postcondition was requested"),
    };
    let matched = wait_for_screen(session, predicate, Duration::ZERO, synchronization.timeout)
        .into_diagnostic()?;
    let matched_ok = matched.matched;
    return Ok((
        matched_ok,
        serde_json::json!({
            "matched": matched.matched,
            "kind": kind,
            "state": matched.frame.state_hash,
            "frame_hash": matched.frame.frame_hash,
            "quality": matched.quality,
            "size": [matched.frame.geometry.cols, matched.frame.geometry.rows],
            "ended": matches!(
                matched.quality,
                ScreenWaitQuality::ChildCompleted | ScreenWaitQuality::OutputClosed
            ),
            "frame": matched.frame,
        }),
    ));
}
pub(super) fn detached_record_arguments(
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
    if let Some(value) = recording.max_duration {
        push_record_option(
            &mut arguments,
            "--max-duration",
            OsStr::new(&format!("{}ms", value.as_millis())),
        );
    }
    let idle_timeout = recording.idle_timeout.unwrap_or(Duration::from_secs(15 * 60));
    if !idle_timeout.is_zero() {
        push_record_option(
            &mut arguments,
            "--idle-timeout",
            OsStr::new(&format!("{}ms", idle_timeout.as_millis())),
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

pub(super) fn push_record_option(arguments: &mut Vec<OsString>, name: &str, value: &OsStr) {
    arguments.push(OsString::from(name));
    arguments.push(value.to_os_string());
}

pub(super) fn push_optional_u16(arguments: &mut Vec<OsString>, name: &str, value: Option<u16>) {
    if let Some(value) = value {
        push_record_option(arguments, name, OsStr::new(&value.to_string()));
    }
}

pub(in crate::cli) fn open_detached_log(path: &Path) -> Result<File> {
    return OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| {
            return miette::miette!(
                "could not create detached recorder log {}: {source}",
                path.display()
            );
        });
}
