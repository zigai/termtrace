use super::*;

pub(in crate::cli) fn run_show(arguments: ShowArgs) -> Result<ExitCode> {
    if arguments.screen.is_some() || arguments.since.is_some() {
        let session = arguments.session.clone().ok_or_else(|| {
            return miette::miette!("compact show requires a canonical session source");
        })?;
        if arguments.input.is_some() || !arguments.command.is_empty() {
            return Err(miette::miette!(
                "compact show accepts only a canonical session source"
            ));
        }
        return run_screen(ScreenArgs {
            session,
            screen: arguments.screen.unwrap_or(ScreenEncoding::Text),
            since: arguments.since,
        });
    }
    let image_format = matches!(arguments.format, ShowFormat::Svg | ShowFormat::Png);
    if !image_format && arguments.renderer == ShowRenderer::Native {
        return Err(miette::miette!(
            "--renderer applies only to SVG and PNG show formats"
        ));
    }
    if arguments.renderer == ShowRenderer::Native && arguments.format != ShowFormat::Png {
        return Err(miette::miette!(
            "native show output is PNG; semantic SVG fallback is disabled"
        ));
    }
    let point = match (
        arguments.state,
        arguments.state_hash,
        arguments.event_seq,
        arguments.at,
        arguments.at_marker,
    ) {
        (Some(state_id), None, None, None, None) => StatePoint::StateId { state_id },
        (None, Some(state_hash), None, None, None) => StatePoint::StateHash { state_hash },
        (None, None, Some(event_seq), None, None) => StatePoint::EventSeq { event_seq },
        (None, None, None, Some(monotonic_ns), None) => StatePoint::MonotonicNs { monotonic_ns },
        (None, None, None, None, Some(label)) => StatePoint::Marker { label },
        (None, None, None, None, None) => StatePoint::LatestPresented,
        _ => return Err(miette::miette!("exactly one show selector may be supplied")),
    };
    let one_off_wait = match (&arguments.wait, &arguments.wait_regex) {
        (Some(text), None) => Some(ScreenPredicate::Contains { text: text.clone() }),
        (None, Some(pattern)) => Some(ScreenPredicate::MatchesRegex {
            pattern: pattern.clone(),
        }),
        (None, None) => None,
        _ => return Err(miette::miette!("exactly one one-off wait may be supplied")),
    };
    if arguments.record_out.is_some() && arguments.input.is_none() {
        return Err(miette::miette!("--record-out requires --input"));
    }
    if (arguments.out.is_some() || arguments.discard_evidence) && arguments.command.is_empty() {
        return Err(miette::miette!(
            "--out and --discard-evidence require a one-off command after --"
        ));
    }
    if arguments.discard_evidence && arguments.detach {
        return Err(miette::miette!(
            "--discard-evidence cannot remove a detached live one-off command"
        ));
    }
    if (one_off_wait.is_some() || arguments.detach) && arguments.command.is_empty() {
        return Err(miette::miette!(
            "--wait, --wait-regex, and --detach require a one-off command after --"
        ));
    }
    if arguments.detach {
        if arguments.session.is_some()
            || arguments.input.is_some()
            || point != StatePoint::LatestPresented
            || one_off_wait.is_some()
        {
            return Err(miette::miette!(
                "detached one-off show accepts only geometry and a command"
            ));
        }
        if arguments.renderer == ShowRenderer::Native {
            require_capability(NATIVE_SCREENSHOT_CAPABILITY)?;
        }
        let outcome = run_one_off_command(
            &arguments.command,
            arguments.cols,
            arguments.rows,
            None,
            Duration::ZERO,
            Duration::from_millis(arguments.timeout_ms),
            true,
            arguments.out.as_deref(),
        )?;
        let source_completed = outcome
            .observation
            .as_ref()
            .is_some_and(|observation| return observation.source_completed);
        return write_json_line(&serde_json::json!({
            "kind": "detached_one_off",
            "canonical_session": ByteString::from(outcome.session.as_os_str().as_bytes()),
            "detached": outcome.detached,
            "source_completed": source_completed,
        }));
    }
    if arguments.recover_prefix {
        let session = arguments.session.as_ref().ok_or_else(|| {
            return miette::miette!("--recover-prefix requires a session source");
        })?;
        if arguments.input.is_some()
            || !arguments.command.is_empty()
            || !arguments.stable_for.is_zero()
            || image_format
            || matches!(arguments.format, ShowFormat::Vt | ShowFormat::Html)
            || arguments.manifest.is_some()
        {
            return Err(miette::miette!(
                "prefix recovery supports session text, JSON, compact JSON, or frame JSON without live stability"
            ));
        }
        let session = resolve_session_ref(session.clone())?;
        let observation = observe_validated_prefix(&session, point, arguments.include_hidden)?;
        if arguments.metadata {
            write_observation_metadata(&observation)?;
        }
        return match arguments.format {
            ShowFormat::Text => write_stdout(format!("{}\n", observation.frame.viewport_text)),
            ShowFormat::Json => write_stdout(format!(
                "{}\n",
                serde_json::to_string_pretty(&observation).into_diagnostic()?
            )),
            ShowFormat::JsonCompact => write_json_line(&observation),
            ShowFormat::FrameJson => write_json_line(&observation.frame_envelope()),
            _ => Err(miette::miette!("unsupported prefix observation format")),
        };
    }
    let formatter = match arguments.format {
        ShowFormat::Vt => Some(GhosttyFormat::Vt),
        ShowFormat::Html => Some(GhosttyFormat::Html),
        _ => None,
    };
    let (observation, formatted, image, discard_session) = match (
        arguments.session,
        arguments.input,
        arguments.command.is_empty(),
    ) {
        (Some(session), None, true) => {
            let session = resolve_session_ref(session)?;
            let mut stable_hash = None;
            if !arguments.stable_for.is_zero() && !session.join("completion.json").is_file() {
                if point != StatePoint::LatestPresented || arguments.include_hidden {
                    return Err(miette::miette!(
                        "live stability is valid only for latest-presented observations"
                    ));
                }
                stable_hash = Some(
                    wait_for_screen(
                        &session,
                        ScreenPredicate::StateHash {
                            hash: "0".repeat(64),
                            comparison: StateHashComparison::Changed,
                        },
                        arguments.stable_for,
                        Duration::from_millis(arguments.timeout_ms),
                    )?
                    .state_hash,
                );
            }
            let mut observation =
                observe_session_with_options(&session, point.clone(), arguments.include_hidden)?;
            if let Some(state_hash) = stable_hash {
                if observation.state_hash != state_hash {
                    return Err(miette::miette!(
                        "presented state changed between stability proof and observation; retry show"
                    ));
                }
                observation.capture_quality = CaptureQuality::Stable;
            }
            let formatted = match formatter {
                Some(format) => {
                    if !session.join("completion.json").is_file() {
                        return Err(miette::miette!(
                            "live VT/HTML formatting requires completed evidence"
                        ));
                    }
                    Some(format_completed_state(&session, &point, format)?)
                }
                None => None,
            };
            let image = if image_format && arguments.renderer == ShowRenderer::Native {
                observation.truth_layer = TruthLayer::Native;
                Some(render_native_show(&session, &point)?)
            } else if image_format {
                let (state, state_hash) = semantic_state_with_hash_for_session(
                    &session,
                    &point,
                    arguments.include_hidden,
                )?;
                ensure_observation_state_hash(&observation, &state_hash)?;
                Some(render_show_image(&state, arguments.format)?)
            } else {
                None
            };
            (observation, formatted, image, None)
        }
        (None, Some(input), true) => {
            if arguments.renderer == ShowRenderer::Native {
                return Err(miette::miette!(
                    "native show rendering requires canonical session evidence; semantic fallback is disabled"
                ));
            }
            if !arguments.stable_for.is_zero() {
                return Err(miette::miette!(
                    "--stable-for requires a live session source"
                ));
            }
            if point != StatePoint::LatestPresented || arguments.include_hidden {
                return Err(miette::miette!(
                    "ANSI streams support only latest-presented selection"
                ));
            }
            let bytes = if input == Path::new("-") {
                let mut bytes = Vec::new();
                io::stdin().read_to_end(&mut bytes).into_diagnostic()?;
                bytes
            } else {
                fs_err::read(&input).into_diagnostic()?
            };
            if let Some(destination) = &arguments.record_out {
                record_ansi_import(&bytes, &input, destination, arguments.cols, arguments.rows)?;
                let mut observation =
                    observe_one_off_session(destination, StatePoint::LatestPresented, false)?;
                observation.source_kind = ObservationSourceKind::ImportedAnsi;
                let formatted = formatter
                    .map(|format| {
                        return format_completed_state(
                            destination,
                            &StatePoint::LatestPresented,
                            format,
                        );
                    })
                    .transpose()?;
                let image = if image_format {
                    let (state, state_hash) = semantic_state_with_hash_for_session(
                        destination,
                        &StatePoint::LatestPresented,
                        false,
                    )?;
                    ensure_observation_state_hash(&observation, &state_hash)?;
                    Some(render_show_image(&state, arguments.format)?)
                } else {
                    None
                };
                (observation, formatted, image, None)
            } else {
                let formatted = formatter
                    .map(|format| {
                        return format_ansi_stream(&bytes, arguments.cols, arguments.rows, format);
                    })
                    .transpose()?;
                let observation = observe_ansi_stream(&bytes, arguments.cols, arguments.rows)?;
                let image = if image_format {
                    let state = semantic_state_from_ansi(&bytes, arguments.cols, arguments.rows)?;
                    ensure_direct_observation_state(&observation, &state)?;
                    Some(render_show_image(&state, arguments.format)?)
                } else {
                    None
                };
                (observation, formatted, image, None)
            }
        }
        (None, None, false) => {
            if !arguments.stable_for.is_zero() && one_off_wait.is_none() {
                return Err(miette::miette!(
                    "one-off --stable-for requires --wait or --wait-regex"
                ));
            }
            if one_off_wait.is_some()
                && (point != StatePoint::LatestPresented
                    || arguments.include_hidden
                    || formatter.is_some()
                    || image_format)
            {
                return Err(miette::miette!(
                    "one-off waits return their exact compact/text/JSON observation and do not accept selectors, hidden states, or rendered formats"
                ));
            }
            if image_format && arguments.renderer == ShowRenderer::Native {
                require_capability(NATIVE_SCREENSHOT_CAPABILITY)?;
            }
            let outcome = run_one_off_command(
                &arguments.command,
                arguments.cols,
                arguments.rows,
                one_off_wait,
                arguments.stable_for,
                Duration::from_millis(arguments.timeout_ms),
                false,
                arguments.out.as_deref(),
            )?;
            let diagnostic_logs = outcome.diagnostic_logs;
            let session = outcome.session;
            let mut observation = match outcome.observation {
                Some(observation) => observation,
                None => observe_one_off_session(&session, point.clone(), arguments.include_hidden)?,
            };
            let formatted = formatter
                .map(|format| return format_completed_state(&session, &point, format))
                .transpose()?;
            let image = if image_format && arguments.renderer == ShowRenderer::Native {
                observation.truth_layer = TruthLayer::Native;
                Some(render_native_show(&session, &point)?)
            } else if image_format {
                let (state, state_hash) = semantic_state_with_hash_for_session(
                    &session,
                    &point,
                    arguments.include_hidden,
                )?;
                ensure_observation_state_hash(&observation, &state_hash)?;
                Some(render_show_image(&state, arguments.format)?)
            } else {
                None
            };
            let discard_session = arguments
                .discard_evidence
                .then_some((session, diagnostic_logs));
            (observation, formatted, image, discard_session)
        }
        _ => return Err(miette::miette!("exactly one show source is required")),
    };
    if arguments.metadata {
        write_observation_metadata(&observation)?;
    }
    if let (Some(path), Some(bytes)) = (&arguments.manifest, &image) {
        let renderer = match (arguments.format, arguments.renderer) {
            (ShowFormat::Svg, ShowRenderer::Semantic) => SEMANTIC_SVG_RENDERER,
            (ShowFormat::Png, ShowRenderer::Semantic) => SEMANTIC_PNG_RENDERER,
            (ShowFormat::Png, ShowRenderer::Native) => "ghostty-native-post-endpoint-v1",
            _ => return Err(miette::miette!("show manifest requires an image format")),
        };
        let manifest = serde_json::to_vec_pretty(&serde_json::json!({
            "format": "termtrace-show-image",
            "version": 1,
            "renderer": renderer,
            "truth_layer": if arguments.renderer == ShowRenderer::Native { "native" } else { "semantic_rendering" },
            "media_type": if matches!(arguments.format, ShowFormat::Svg) { "image/svg+xml" } else { "image/png" },
            "byte_count": bytes.len(),
            "blake3": blake3::hash(bytes).to_hex().to_string(),
            "observation": observation,
        }))
        .into_diagnostic()?;
        write_secure(path, &manifest)?;
    } else if arguments.manifest.is_some() {
        return Err(miette::miette!(
            "--manifest is valid only for SVG or PNG output"
        ));
    }
    let result = match arguments.format {
        ShowFormat::Text => write_stdout(format!("{}\n", observation.frame.viewport_text)),
        ShowFormat::Json => write_stdout(format!(
            "{}\n",
            serde_json::to_string_pretty(&observation).into_diagnostic()?
        )),
        ShowFormat::JsonCompact => write_json_line(&observation),
        ShowFormat::FrameJson => write_json_line(&observation.frame_envelope()),
        ShowFormat::Vt | ShowFormat::Html => write_stdout(formatted.ok_or_else(|| {
            return miette::miette!("selected formatter produced no output");
        })?),
        ShowFormat::Svg | ShowFormat::Png => write_stdout(image.ok_or_else(|| {
            return miette::miette!("selected image renderer produced no output");
        })?),
    };
    if result.is_ok()
        && let Some((session, diagnostic_logs)) = discard_session
    {
        fs_err::remove_dir_all(&session).into_diagnostic()?;
        for log in diagnostic_logs {
            if log.exists() {
                fs_err::remove_file(log).into_diagnostic()?;
            }
        }
        writeln!(
            io::stderr(),
            "termtrace: discarded one-off evidence {}; the returned observation is no longer independently replayable",
            session.display()
        )
        .into_diagnostic()?;
    }
    return result;
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

fn render_show_image(state: &SemanticState, format: ShowFormat) -> Result<Vec<u8>> {
    return match format {
        ShowFormat::Svg => Ok(render_svg(state)),
        ShowFormat::Png => render_png(state).into_diagnostic(),
        _ => Err(miette::miette!("show image renderer requires SVG or PNG")),
    };
}

fn ensure_direct_observation_state(observation: &Observation, state: &SemanticState) -> Result<()> {
    let state_hash = semantic_state_hash(state)?;
    return ensure_observation_state_hash(observation, &state_hash);
}

fn ensure_observation_state_hash(observation: &Observation, state_hash: &str) -> Result<()> {
    if observation.state_hash != state_hash {
        return Err(miette::miette!(
            "semantic image state changed after observation; retry show"
        ));
    }
    return Ok(());
}

fn render_native_show(session: &Path, point: &StatePoint) -> Result<Vec<u8>> {
    if !session.join("completion.json").is_file() {
        return Err(miette::miette!(
            "native show rendering requires completed canonical evidence"
        ));
    }
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(session, &log)?;
    let inspection = inspect_point(&log, point)?;
    if !inspection.record.presented {
        return Err(miette::miette!(
            "native show rendering rejects hidden semantic states"
        ));
    }
    let event_seq = inspection
        .record
        .event_seq
        .ok_or_else(|| return miette::miette!("the initial state has no native replay endpoint"))?;
    require_capability(NATIVE_SCREENSHOT_CAPABILITY)?;
    let (target, geometry_solver) = native_capture_target(session, None)?;
    let destination =
        env::temp_dir().join(format!(".termtrace-native-show-{}", uuid::Uuid::now_v7()));
    let outcome = capture(&CaptureOptions {
        session_directory: session.to_path_buf(),
        width: target.width,
        height: target.height,
        fps: 24,
        start_monotonic_ns: None,
        end_monotonic_ns: None,
        state_selection: StateSelection::Presented,
        geometry_solver,
        kind: CaptureKind::Screenshot {
            state_id: inspection.record.state_id,
            state_hash: inspection.record.state_hash,
            event_seq,
            event_byte_offset: inspection.record.event_byte_offset,
        },
        destination: Some(destination.clone()),
    })?;
    let image = fs_err::read(outcome.screenshot).into_diagnostic()?;
    fs_err::remove_dir_all(&destination)
        .map_err(|error| return miette::miette!("remove native show staging artifact: {error}"))?;
    return Ok(image);
}

fn format_completed_state(
    session: &Path,
    point: &StatePoint,
    format: GhosttyFormat,
) -> Result<Vec<u8>> {
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(session, &log)?;
    let inspection = inspect_point(&log, point)?;
    return Ok(format_state(
        &log,
        StateSelector::StateId(inspection.record.state_id),
        format,
    )?);
}

struct OneOffCommandOutcome {
    session: PathBuf,
    observation: Option<Observation>,
    detached: bool,
    diagnostic_logs: Vec<PathBuf>,
}

fn record_ansi_import(
    bytes: &[u8],
    source: &Path,
    destination: &Path,
    cols: u16,
    rows: u16,
) -> Result<()> {
    if destination.exists() {
        return Err(Error::ArtifactExists {
            path: destination.to_path_buf(),
        }
        .into());
    }
    let parent = destination
        .parent()
        .ok_or_else(|| return miette::miette!("ANSI import destination has no parent"))?;
    fs_err::create_dir_all(parent).into_diagnostic()?;
    let input = parent.join(format!(".ansi-import-{}.tmp", uuid::Uuid::now_v7()));
    write_secure(&input, bytes)?;
    let hash = blake3::hash(bytes).to_hex().to_string();
    let process = ProcessCommand::new(env::current_exe().into_diagnostic()?)
        .args(["record", "--no-input", "--out"])
        .arg(destination)
        .args(["--cols", &cols.to_string(), "--rows", &rows.to_string()])
        .args(["--import-source-kind", "ansi_stream"])
        .args(["--import-source-byte-count", &bytes.len().to_string()])
        .args(["--import-source-blake3", &hash])
        .arg("--import-source")
        .arg(source.as_os_str())
        .args([
            "--",
            "/bin/sh",
            "-c",
            "stty raw -echo < /dev/tty; exec /bin/cat -- \"$1\"",
            "termtrace-ansi-import",
        ])
        .arg(&input)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    let cleanup = fs_err::remove_file(&input);
    let process = process.into_diagnostic()?;
    cleanup.into_diagnostic()?;
    if !process.status.success() {
        return Err(miette::miette!(
            "ANSI import recorder failed with {}: {}",
            process.status,
            String::from_utf8_lossy(&process.stderr)
        ));
    }
    let log = EventLog::read(destination.join("events.jsonl"))?;
    validate_completed_log(destination, &log)?;
    let imported = log
        .events
        .iter()
        .filter_map(|record| {
            return match &record.event {
                Event::Output { data } => Some(data.as_bytes()),
                _ => None,
            };
        })
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    if imported != bytes {
        return Err(miette::miette!(
            "ANSI import canonical output does not match the exact source bytes"
        ));
    }
    let manifest = read_manifest(destination)?;
    if manifest
        .import_provenance
        .as_ref()
        .is_none_or(|provenance| {
            return provenance.source_kind != "ansi_stream"
                || provenance.byte_count != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
                || provenance.blake3 != hash;
        })
    {
        return Err(miette::miette!(
            "ANSI import session did not bind its source provenance"
        ));
    }
    return Ok(());
}

fn run_one_off_command(
    command: &[OsString],
    cols: u16,
    rows: u16,
    wait_predicate: Option<ScreenPredicate>,
    stable_for: Duration,
    timeout: Duration,
    detach: bool,
    requested_out: Option<&Path>,
) -> Result<OneOffCommandOutcome> {
    if cols == 0 || rows == 0 {
        return Err(miette::miette!(
            "one-off geometry must be greater than zero"
        ));
    }
    let paths = if let Some(session) = requested_out {
        if session.exists() {
            return Err(Error::ArtifactExists {
                path: session.to_path_buf(),
            }
            .into());
        }
        let parent = session
            .parent()
            .ok_or_else(|| return miette::miette!("one-off --out path has no parent"))?;
        fs_err::create_dir_all(parent).into_diagnostic()?;
        let name = session
            .file_name()
            .ok_or_else(|| return miette::miette!("one-off --out path has no basename"))?
            .to_string_lossy();
        let identity = uuid::Uuid::now_v7();
        DetachedSessionPaths {
            session: session.to_path_buf(),
            stdout_log: parent.join(format!(".{name}.{identity}.stdout.log")),
            stderr_log: parent.join(format!(".{name}.{identity}.stderr.log")),
        }
    } else {
        SessionRegistry::open_default()?.allocate_detached_paths()?
    };
    let stdout = open_detached_log(&paths.stdout_log)?;
    let stderr = open_detached_log(&paths.stderr_log)?;
    let executable = env::current_exe().into_diagnostic()?;
    let mut child = ProcessCommand::new(executable)
        .arg("record")
        .arg("--no-input")
        .arg("--out")
        .arg(&paths.session)
        .arg("--cols")
        .arg(cols.to_string())
        .arg("--rows")
        .arg(rows.to_string())
        .arg("--")
        .args(command)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .into_diagnostic()?;
    let started = Instant::now();
    let deadline = started + timeout;
    loop {
        if control_endpoint_is_reachable(&paths.session) {
            break;
        }
        if let Some(status) = child.try_wait().into_diagnostic()? {
            if paths.session.join("completion.json").is_file() {
                let mut observation =
                    observe_one_off_session(&paths.session, StatePoint::LatestPresented, false)?;
                if let Some(predicate) = &wait_predicate {
                    let matched = match predicate {
                        ScreenPredicate::Contains { text } => {
                            observation.frame.viewport_text.contains(text)
                        }
                        ScreenPredicate::MatchesRegex { pattern } => regex::Regex::new(pattern)
                            .into_diagnostic()?
                            .is_match(&observation.frame.viewport_text),
                        _ => false,
                    };
                    if !matched {
                        return Err(miette::miette!(
                            "one-off command completed before its screen predicate matched; completed evidence: {}",
                            paths.session.display(),
                        ));
                    }
                }
                observation.source_kind = ObservationSourceKind::OneOffCommand;
                observation.source_completed = true;
                return Ok(OneOffCommandOutcome {
                    session: paths.session,
                    observation: Some(observation),
                    detached: false,
                    diagnostic_logs: vec![paths.stdout_log, paths.stderr_log],
                });
            }
            return Err(miette::miette!(
                "one-off recorder exited with {status} before control readiness; recoverable session candidate: {}; stdout log: {}; stderr log: {}",
                paths.session.display(),
                paths.stdout_log.display(),
                paths.stderr_log.display(),
            ));
        }
        if Instant::now() >= deadline {
            return Err(miette::miette!(
                "one-off recorder did not become ready before the deadline; recoverable session candidate: {}",
                paths.session.display(),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
    if detach {
        return Ok(OneOffCommandOutcome {
            session: paths.session,
            observation: None,
            detached: true,
            diagnostic_logs: vec![paths.stdout_log, paths.stderr_log],
        });
    }
    let mut observation = if let Some(predicate) = wait_predicate {
        let waited = match wait_for_screen(
            &paths.session,
            predicate,
            stable_for,
            timeout.saturating_sub(started.elapsed()),
        ) {
            Ok(waited) => waited,
            Err(error) => {
                let stop_result = request(&paths.session, ControlCommand::Stop);
                if stop_result.is_ok() {
                    let _ = child.wait();
                }
                let stop_description = match &stop_result {
                    Ok(_) => "accepted".to_owned(),
                    Err(stop_error) => stop_error.to_string(),
                };
                return Err(miette::miette!(
                    "one-off wait failed: {error}; evidence preserved at {}; stop request: {}",
                    paths.session.display(),
                    stop_description,
                ));
            }
        };
        let observation = match observe_session_with_options(
            &paths.session,
            StatePoint::LatestPresented,
            false,
        ) {
            Ok(observation) => observation,
            Err(error) => {
                let stop_result = request(&paths.session, ControlCommand::Stop);
                if stop_result.is_ok() {
                    let _ = child.wait();
                }
                let stop_description = match &stop_result {
                    Ok(_) => "accepted".to_owned(),
                    Err(stop_error) => stop_error.to_string(),
                };
                return Err(miette::miette!(
                    "one-off observation failed after its wait proof: {error}; evidence preserved at {}; stop request: {}",
                    paths.session.display(),
                    stop_description,
                ));
            }
        };
        if observation.state_hash != waited.state_hash {
            let stop_result = request(&paths.session, ControlCommand::Stop);
            if stop_result.is_ok() {
                let _ = child.wait();
            }
            let stop_description = match &stop_result {
                Ok(_) => "accepted".to_owned(),
                Err(stop_error) => stop_error.to_string(),
            };
            return Err(miette::miette!(
                "one-off presented state changed after its wait proof; evidence preserved at {}; stop request: {}",
                paths.session.display(),
                stop_description,
            ));
        }
        request(&paths.session, ControlCommand::Stop)?;
        Some(observation)
    } else {
        None
    };
    let status = child.wait().into_diagnostic()?;
    if !paths.session.join("completion.json").is_file() {
        return Err(miette::miette!(
            "one-off recorder exited with {status} before durable completion; recoverable session candidate: {}; stdout log: {}; stderr log: {}",
            paths.session.display(),
            paths.stdout_log.display(),
            paths.stderr_log.display(),
        ));
    }
    if let Some(observation) = &mut observation {
        observation.source_kind = ObservationSourceKind::OneOffCommand;
        observation.source_completed = true;
    }
    return Ok(OneOffCommandOutcome {
        session: paths.session,
        observation,
        detached: false,
        diagnostic_logs: vec![paths.stdout_log, paths.stderr_log],
    });
}
