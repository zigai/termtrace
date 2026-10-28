use super::*;
use nix::{
    fcntl::{fcntl, FcntlArg, SealFlag},
    unistd::geteuid,
};
use std::{
    fs::{self, File},
    os::{fd::AsRawFd as _, unix::fs::MetadataExt as _},
    path::Component,
    sync::atomic::AtomicBool,
};
use termtrace_model::execution::session::{recorder_exit_status, RecordingFailurePolicy};

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
        original_grid: _,
        original_canvas: _,
        explicit_geometry: _,
        resolved_ghostty_profile_provenance,
        outer_resize_socket,
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
    let resize_authority = outer_resize_socket.map_or(ResizeAuthority::ChildPty, |socket| {
        return ResizeAuthority::OuterTerminal { socket };
    });

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

    let stdin = if arguments.environment_spec_stdin {
        if !no_input {
            return Err(miette::miette!(
                "--environment-spec-stdin requires --no-input so stdin cannot be consumed twice"
            ));
        }

        Some(read_environment_stdin()?)
    } else {
        None
    };

    let supplied_environment = supplied_environment(&arguments, stdin.as_deref())?;

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

        return report_record_outcome(run_with_response(response, &options, &resize_authority)?);
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

    let visual_profile = arguments
        .visual_profile
        .as_ref()
        .map(VisualProfile::record)
        .transpose()
        .into_diagnostic()?;
    if let Some(recorded) = &visual_profile {
        check_visual_profile_arguments(
            &arguments,
            &overrides,
            resolved_ghostty_profile_provenance.is_some(),
            recorded,
        )?;
    }

    let profile_dpi = visual_profile
        .as_ref()
        .and_then(|recorded| return recorded.profile.native.as_ref())
        .map(NativeVisualProfile::resolved_dpi);

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
    } else if let Some(recorded) = &visual_profile {
        resolve_visual_profile(&recorded.profile, &AtomicBool::new(false))
            .into_diagnostic()?
            .map(|resolution| return resolution.profile)
    } else if needs_profile {
        Some(
            resolve_ghostty_profile(arguments.ghostty_config.as_deref(), overrides)
                .into_diagnostic()?,
        )
    } else {
        None
    };

    let (geometry, geometry_provenance) =
        resolve_record(&arguments, ghostty_profile.as_ref(), profile_dpi)?;

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
            visual_profile,
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
        &resize_authority,
    )?;

    return report_record_outcome(outcome);
}

pub(super) fn check_visual_profile_arguments(
    arguments: &RecordingArgs,
    overrides: &GhosttyVisualOverrides,
    resolved: bool,
    recorded: &RecordedVisualProfile,
) -> Result<()> {
    if !overrides.is_empty() || (arguments.ghostty_config.is_some() && !resolved) {
        return Err(miette::miette!(
            "--visual-profile replaces --ghostty-config, --font-family, --font-size, --padding-x, --padding-y, --foreground and --background"
        ));
    }

    if recorded.profile.native.is_none() && matches!(arguments.geometry, GeometryMode::Fit) {
        return Err(miette::miette!(
            "fitted geometry measures Ghostty and requires a native visual profile section"
        ));
    }
    return Ok(());
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
    resize_authority: &ResizeAuthority,
) -> Result<RecordOutcome> {
    let mut output = response_output(response);

    return record_with_resize_authority(options, &mut output, resize_authority).into_diagnostic();
}

pub(super) fn report_record_outcome(outcome: RecordOutcome) -> Result<ExitCode> {
    writeln!(io::stderr(), "recording: {}", outcome.event_log.display()).into_diagnostic()?;

    return recorder_exit_status(
        outcome.exit_code,
        outcome.signal_name.as_deref(),
        outcome.resource_limit.is_some(),
    )
    .map(ExitCode::from)
    .ok_or_else(|| return miette::miette!("child exit code does not fit a process exit status"));
}

pub(super) fn read_environment_stdin() -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    io::stdin()
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .into_diagnostic()?;

    return Ok(bytes);
}

fn is_descriptor_path(path: &Path) -> bool {
    let components: Vec<_> = path.components().collect();
    let number = |component: &Component<'_>| {
        return component.as_os_str().to_str().is_some_and(|text| {
            return !text.is_empty() && text.bytes().all(|b| return b.is_ascii_digit());
        });
    };

    return matches!(
        components.as_slice(),
        [Component::RootDir, Component::Normal(proc), pid, Component::Normal(fd), descriptor]
            if *proc == "proc" && *fd == "fd" && number(pid) && number(descriptor)
    );
}

// An original-capture supervisor passes its sealed owner-only memfd as `/proc/<pid>/fd/<n>`.
// The magic link is followed, and the opened descriptor itself must be that memfd.
fn read_environment_memfd(path: &Path) -> Result<EnvironmentSpec> {
    let mut file = File::open(path).into_diagnostic()?;
    let target = fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).into_diagnostic()?;
    let metadata = file.metadata().into_diagnostic()?;
    let required = SealFlag::F_SEAL_SHRINK | SealFlag::F_SEAL_GROW | SealFlag::F_SEAL_WRITE;
    let seals =
        SealFlag::from_bits_truncate(fcntl(&file, FcntlArg::F_GET_SEALS).into_diagnostic()?);

    if !target.as_os_str().as_bytes().starts_with(b"/memfd:")
        || !metadata.is_file()
        || metadata.uid() != geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || !seals.contains(required)
    {
        return Err(miette::miette!(
            "{}: a descriptor environment specification must be a sealed owner-only memfd",
            path.display()
        ));
    }

    let mut bytes = Vec::new();
    (&mut file)
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .into_diagnostic()?;

    return parse_environment_spec(&bytes, path).into_diagnostic();
}

pub(super) fn supplied_environment(
    arguments: &RecordingArgs,
    stdin: Option<&[u8]>,
) -> Result<Option<EnvironmentSpec>> {
    if let Some(bytes) = stdin {
        return parse_environment_spec(bytes, Path::new("<environment-spec-stdin>"))
            .map(Some)
            .into_diagnostic();
    }

    if let Some(path) = arguments.environment_spec.as_deref() {
        if is_descriptor_path(path) {
            return read_environment_memfd(path).map(Some);
        }
        return read_environment_spec(path).map(Some).into_diagnostic();
    }

    if arguments.clear_env
        || !arguments.copy_env.is_empty()
        || !arguments.unset_env.is_empty()
        || arguments.color_policy.is_some()
    {
        return Ok(Some(EnvironmentSpec {
            base: if arguments.clear_env {
                EnvironmentBase::Clear
            } else {
                EnvironmentBase::Inherit
            },
            copy: arguments.copy_env.iter().cloned().collect(),
            unset: arguments.unset_env.iter().cloned().collect(),
            color: arguments.color_policy.map_or(ColorPolicy::Auto, Into::into),
            ..EnvironmentSpec::default()
        }));
    }
    return Ok(None);
}
