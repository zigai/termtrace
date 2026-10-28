use super::*;

pub(super) fn run_detached_record(
    response: &mut ResponseWriter,
    arguments: RecordArgs,
) -> Result<ExitCode> {
    let RecordArgs {
        out,
        detach,
        startup_timeout_ms,
        until_idle,
        until_screen,
        until_screen_regex,
        timeout,
        json,
        recording,
        ..
    } = arguments;
    let mut recording = recording;
    let name = if let Some(alias) = arguments.session_alias {
        alias
    } else if let Some(d) = detach.filter(|s| return !s.is_empty()) {
        d
    } else {
        if recording.command.is_empty() {
            return Err(miette::miette!(
                "detached recording requires a session alias and command"
            ));
        }

        recording.command.remove(0).to_string_lossy().to_string()
    };

    if recording
        .command
        .first()
        .is_some_and(|arg| return arg == "--")
    {
        recording.command.remove(0);
    }

    let synchronization = StartSynchronization {
        until_idle,
        until_screen,
        until_screen_regex,
        timeout: timeout.unwrap_or(defaults::WAIT_TIMEOUT),
    };

    if matches!(recording.geometry, GeometryMode::Manual) {
        manual_geometry(&recording)?;
    }

    let startup_timeout = millis_or(startup_timeout_ms, defaults::STARTUP_TIMEOUT);
    let _durability_policy = durability_policy_for(&recording)?;
    let registry = RegistryService::open_default().into_diagnostic()?;
    let executable = env::current_exe().into_diagnostic()?;
    let stdin = if recording.environment_spec_stdin {
        let mut bytes = Vec::new();
        io::stdin()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .into_diagnostic()?;

        Some(bytes)
    } else {
        None
    };

    let started = registry
        .start_detached(
            &executable,
            DetachedStart {
                alias: name.clone(),
                session: out,
                startup_timeout,
                stdin,
            },
            |session| return detached_record_arguments(&name, session, &recording),
        )
        .into_diagnostic()?;

    return report_start_outcome(
        response,
        &started.entry,
        &started.paths,
        started.completion.as_deref(),
        &synchronization,
        json,
    );
}

#[derive(Debug)]
pub(super) struct StartSynchronization {
    pub(super) until_idle: Option<Duration>,
    pub(super) until_screen: Option<String>,
    pub(super) until_screen_regex: Option<String>,
    pub(super) timeout: Duration,
}

impl StartSynchronization {
    fn requested(&self) -> bool {
        return self.until_idle.is_some()
            || self.until_screen.is_some()
            || self.until_screen_regex.is_some();
    }
}

pub(super) fn report_start_outcome(
    response: &mut ResponseWriter,
    entry: &RegistryEntry,
    paths: &DetachedSessionPaths,
    completion: Option<&SessionCompletion>,
    synchronization: &StartSynchronization,
    json: bool,
) -> Result<ExitCode> {
    let startup_state = match completion {
        None => "live",
        Some(completion)
            if completion.exit_code == Some(0)
                && completion.signal_name.is_none()
                && completion.resource_limit.is_none() =>
        {
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
                        "ended": attach_session(&entry.session_path())
                            .and_then(|session| return session.completion().into_diagnostic())
                            .is_ok_and(|completion| return completion.is_some()),
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

    if json {
        write_json_pretty(response, &output)?;
    } else {
        write_stdout(response, format!("Started session {}\n", entry.alias))?;
    }

    if !synchronization_ok {
        return Ok(ExitCode::FAILURE);
    }

    let Some(completion) = completion else {
        return Ok(ExitCode::SUCCESS);
    };
    if completion.resource_limit.is_some() {
        return Ok(ExitCode::FAILURE);
    }
    return Ok(match (completion.exit_code, &completion.signal_name) {
        (Some(code), None) => u8::try_from(code).map_or(ExitCode::FAILURE, ExitCode::from),
        _ => ExitCode::FAILURE,
    });
}

fn start_postcondition(
    session: &Path,
    synchronization: &StartSynchronization,
) -> Result<(bool, serde_json::Value)> {
    let session = attach_session(session)?;

    if let Some(stable_for) = synchronization.until_idle {
        let capture = session
            .capture_stable(StableCaptureOptions {
                stable_for,
                timeout: synchronization.timeout,
                projection: ScreenProjectionRequest::default(),
                allow_incomplete: true,
                reaction: None,
            })
            .into_diagnostic()?;

        let matched = capture.quality.is_settled();

        return Ok((
            matched,
            serde_json::json!({
                "matched": matched,
                "kind": "idle",
                "quality": capture.quality,
                "state_hash": capture.frame.state_hash,
                "frame_hash": capture.frame.frame_hash,
                "screen": screen_text(&capture.frame),
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
        (None, None) => {
            return Err(miette::miette!(
                "start postcondition requires until-idle, until-screen, or until-screen-regex"
            ));
        }
    };

    let waited = session
        .wait_screen(predicate, Duration::ZERO, synchronization.timeout)
        .into_diagnostic()?;

    let matched = waited.is_satisfied();

    return Ok((
        matched,
        serde_json::json!({
            "matched": matched,
            "kind": kind,
            "state": waited.frame.state_hash,
            "frame_hash": waited.frame.frame_hash,
            "quality": waited.quality,
            "size": [waited.frame.geometry.cols, waited.frame.geometry.rows],
            "ended": matches!(
                waited.quality,
                ScreenWaitQuality::ChildCompleted
                    | ScreenWaitQuality::OutputClosed
                    | ScreenWaitQuality::Completed
            ),
            "screen": screen_text(&waited.frame),
        }),
    ));
}

fn screen_text(frame: &ScreenFrame) -> String {
    return frame_text(frame)
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end_matches('\n')
        .to_owned();
}

pub(in crate::cli) fn detached_record_arguments(
    alias: &str,
    session: &Path,
    recording: &RecordingArgs,
) -> Vec<OsString> {
    return record_arguments(Some(alias), session, recording, false);
}

pub(super) fn record_arguments(
    alias: Option<&str>,
    session: &Path,
    recording: &RecordingArgs,
    forward_stdin: bool,
) -> Vec<OsString> {
    let mut arguments = vec![
        OsString::from("record"),
        OsString::from("--out"),
        session.as_os_str().to_os_string(),
    ];

    if !forward_stdin {
        arguments.push(OsString::from("--no-input"));
    }

    if let Some(alias) = alias {
        push_record_option(&mut arguments, "--session-alias", OsStr::new(alias));
    }

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

    if let Some(profile) = &recording.visual_profile {
        let json = serde_json::to_string(profile).unwrap_or_default();
        push_record_option(&mut arguments, "--visual-profile", OsStr::new(&json));
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

    let idle_timeout = recording.idle_timeout.unwrap_or(if forward_stdin {
        Duration::ZERO
    } else {
        Duration::from_secs(15 * 60)
    });

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
