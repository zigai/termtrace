use crate::cli::{
    command::should_disable_color,
    output::{write_json_pretty, write_progress, write_stdout, ResponseWriter},
    tooling::args::DoctorArgs,
};
use crossterm::terminal;
use miette::{IntoDiagnostic as _, Result};
use nix::unistd::getuid;
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    flag,
};
use std::{
    collections::BTreeSet,
    env,
    ffi::OsString,
    fmt::Write as _,
    fs::{self, Permissions},
    io::{self, ErrorKind, IsTerminal as _},
    mem,
    os::unix::{
        ffi::OsStringExt as _,
        fs::{MetadataExt as _, PermissionsExt as _},
    },
    path::{Path, PathBuf},
    process::ExitCode,
    result::Result as StdResult,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
};
use tempfile::Builder as TempBuilder;
use termtrace_engine::{
    recording::{record_with_output, RecordOptions},
    registry::{SessionRegistry, STATE_DIR_ENV},
};
use termtrace_evidence::{event_log::DurabilityPolicy, session::verify_session};
use termtrace_model::execution::{
    environment::EnvironmentSpec,
    event::TerminalColors,
    provenance::ProvenancePolicy,
    session::{GeometryProvenance, RecordingFailurePolicy, ResourceLimits, VerificationLevel},
};
use termtrace_model::protocol::{
    capability::{
        capability_tools, Capability, CapabilityReport, CapabilityStatus, CAPABILITY_IDS,
        CORE_CAPABILITY, NATIVE_SCREENSHOT_CAPABILITY, NATIVE_VIDEO_CAPABILITY,
        PRESENTATION_CAPABILITY, SEMANTIC_CAPABILITY, SEMANTIC_VIDEO_CAPABILITY,
    },
    doctor::{
        DoctorBuild, DoctorCheck, DoctorFix, DoctorFixOutcome, DoctorFixResult, DoctorInstall,
        DoctorPath, DoctorPaths, DoctorReport, DoctorStatus, DoctorSummary,
    },
};
use termtrace_process::{executables_on_path, find_executable, is_executable};
use termtrace_service::system::SystemService;
use termtrace_tmux::{
    artifact::TmuxSocket,
    client::TmuxClient,
    options::{self as tmux_options, INTEGRATION_VERSION, WRAPPER_BINARY_ENV},
    setup::launch_hook_supported,
};

const INSTALL: &str = "Install";
const DEPENDENCIES: &str = "Dependencies";
const STATE: &str = "State";
const CAPABILITIES: &str = "Capabilities";
const SMOKE: &str = "Smoke";
const STATE_ROOT_ID: &str = "state.root";
const ALIASES_ID: &str = "state.aliases";
const DOCTOR_FIX: &str = "termtrace doctor --fix";
const DEFAULT_PATH_CAPABILITIES: &[&str] = &[
    NATIVE_SCREENSHOT_CAPABILITY,
    NATIVE_VIDEO_CAPABILITY,
    PRESENTATION_CAPABILITY,
];
const ENVIRONMENT: &[&str] = &[
    STATE_DIR_ENV,
    "TERMTRACE_LOG",
    "TERMTRACE_SESSION",
    "TERMTRACE_DPI",
    "TERMTRACE_SCALE",
    "TERMTRACE_DISPLAY_SCALE",
    "TERMTRACE_INDEX_FPS",
    "TERMTRACE_INDEX_SAMPLING_STRIDE_NS",
    WRAPPER_BINARY_ENV,
];

pub(in crate::cli) fn run_doctor(
    response: &mut ResponseWriter,
    arguments: DoctorArgs,
) -> Result<ExitCode> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let received = Arc::new(AtomicUsize::new(0));

    for signal in [SIGINT, SIGTERM] {
        flag::register(signal, Arc::clone(&cancelled)).into_diagnostic()?;
        flag::register_usize(
            signal,
            Arc::clone(&received),
            usize::try_from(signal).into_diagnostic()?,
        )
        .into_diagnostic()?;
    }

    let home = env::var_os("HOME").map(PathBuf::from);
    let required: BTreeSet<&str> = arguments.require.iter().map(String::as_str).collect();
    let (capabilities, smoke) = thread::scope(|scope| {
        let smoke = scope.spawn(record_smoke);
        let capabilities = SystemService::probe_capabilities(&cancelled);

        return (
            capabilities,
            smoke
                .join()
                .unwrap_or_else(|_| return Err("the smoke recording panicked".to_owned())),
        );
    });

    let state = diagnose_state(home.as_deref());
    let tools = dependency_tools();
    let aliases = aliases_check(&state.check);

    let mut checks = vec![install_check(home.as_deref())];
    checks.extend(dependency_checks(&tools, &required, home.as_deref()));
    checks.push(launch_hook_check(&tools));
    checks.push(state.check);
    checks.push(aliases);
    checks.extend(tmux_integration_check(&tools, home.as_deref()));
    checks.extend(capability_checks(&capabilities, &tools, &required));
    checks.push(smoke_check(smoke, &capabilities));

    let signal = received.load(Ordering::SeqCst);
    if signal != 0 {
        return Ok(ExitCode::from(u8::try_from(128 + signal).unwrap_or(1)));
    }

    let mut fixes = Vec::new();
    if arguments.fix && !state.repairs.is_empty() {
        for path in &state.repairs {
            let action = format!("chmod 700 {}", shell_path(path, home.as_deref()));
            write_progress(response, &action);
            let error = fs::set_permissions(path, Permissions::from_mode(0o700)).err();
            fixes.push(DoctorFixOutcome {
                id: STATE_ROOT_ID.to_owned(),
                action,
                result: if error.is_some() {
                    DoctorFixResult::Failed
                } else {
                    DoctorFixResult::Applied
                },
                error: error.map(|error| return error.to_string()),
            });
        }

        replace_check(&mut checks, diagnose_state(home.as_deref()).check);
    }

    if arguments.fix
        && checks
            .iter()
            .any(|check| return check.id == ALIASES_ID && check.status == DoctorStatus::Warn)
    {
        let removed = SessionRegistry::default_state_root()
            .and_then(SessionRegistry::open)
            .and_then(|registry| return registry.reap_stale_aliases(true));

        let action = "remove stale alias records".to_owned();
        write_progress(response, &action);

        fixes.push(DoctorFixOutcome {
            id: ALIASES_ID.to_owned(),
            action: match &removed {
                Ok(aliases) => format!("{action}: {}", aliases.join(", ")),
                Err(_) => action,
            },
            result: if removed.is_ok() {
                DoctorFixResult::Applied
            } else {
                DoctorFixResult::Failed
            },
            error: removed.err().map(|error| return error.to_string()),
        });

        let state = checks
            .iter()
            .find(|check| return check.id == STATE_ROOT_ID)
            .cloned();
        if let Some(state) = state {
            replace_check(&mut checks, aliases_check(&state));
        }
    }

    let report = report(home.as_deref(), checks, fixes);

    if arguments.json {
        write_json_pretty(response, &report)?;
    } else {
        write_stdout(response, render(&report, &TextStyle::detect())?)?;
    }

    let automatic = report
        .checks
        .iter()
        .filter(|check| {
            return matches!(check.status, DoctorStatus::Warn | DoctorStatus::Fail)
                && check.fix.as_ref().is_some_and(|fix| return fix.automatic);
        })
        .count();

    if !arguments.fix && automatic > 0 {
        write_progress(
            response,
            format!(
                "Run 'termtrace doctor --fix' to apply {automatic} {}.",
                plural(automatic, "fix", "fixes")
            ),
        );
    }

    if report.summary.fail > 0 {
        return Ok(ExitCode::FAILURE);
    }
    return Ok(ExitCode::SUCCESS);
}

fn report(
    home: Option<&Path>,
    checks: Vec<DoctorCheck>,
    fixes: Vec<DoctorFixOutcome>,
) -> DoctorReport {
    let executable = env::current_exe().ok();
    let state = SessionRegistry::default_state_root().ok().map(|path| {
        return DoctorPath {
            path: display_path(&path, home),
            source: env::var_os(STATE_DIR_ENV).map(|_| return STATE_DIR_ENV.to_owned()),
        };
    });

    let environment = ENVIRONMENT
        .iter()
        .filter_map(|name| {
            return env::var_os(name).map(|value| {
                return ((*name).to_owned(), display_path(Path::new(&value), home));
            });
        })
        .collect();

    let mut summary = DoctorSummary::default();
    for check in &checks {
        let count = match check.status {
            DoctorStatus::Ok => &mut summary.ok,
            DoctorStatus::Info => &mut summary.info,
            DoctorStatus::Warn => &mut summary.warn,
            DoctorStatus::Fail => &mut summary.fail,
            DoctorStatus::Skip => &mut summary.skip,
        };
        *count += 1;
    }

    return DoctorReport {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        build: DoctorBuild {
            revision: env!("TERMTRACE_BUILD_REVISION").to_owned(),
            target: format!("{}/{}", env::consts::OS, env::consts::ARCH),
        },
        install: DoctorInstall {
            method: executable.as_deref().and_then(install_method),
            executable: executable
                .as_deref()
                .map(|path| return display_path(path, home)),
        },
        paths: DoctorPaths { state },
        env: environment,
        checks,
        summary,
        fixes,
    };
}

fn install_method(executable: &Path) -> Option<String> {
    let cargo_home = env::var_os("CARGO_HOME").map(PathBuf::from).or_else(|| {
        return env::var_os("HOME").map(|home| return Path::new(&home).join(".cargo"));
    })?;

    return (executable.parent() == Some(cargo_home.join("bin").as_path()))
        .then(|| return "cargo".to_owned());
}

fn install_check(home: Option<&Path>) -> DoctorCheck {
    let mut seen = BTreeSet::new();
    let copies: Vec<PathBuf> = executables_on_path("termtrace")
        .into_iter()
        .filter(|path| {
            return seen.insert(fs::canonicalize(path).unwrap_or_else(|_| return path.clone()));
        })
        .collect();

    return match copies.as_slice() {
        [] => check(
            INSTALL,
            "path",
            DoctorStatus::Info,
            "No copy of termtrace is on PATH",
        ),
        [copy] => check(
            INSTALL,
            "path",
            DoctorStatus::Ok,
            format!("{}, no other copies on PATH", display_path(copy, home)),
        ),
        _ => DoctorCheck {
            details: copies
                .iter()
                .map(|copy| return display_path(copy, home))
                .collect(),
            fix: Some(manual(
                "Remove the copies you do not use; the first one listed is the one that runs",
            )),
            ..check(
                INSTALL,
                "path",
                DoctorStatus::Warn,
                format!("{} copies on PATH", copies.len()),
            )
        },
    };
}

struct Tool {
    program: &'static str,
    found: Option<PathBuf>,
}

fn dependency_tools() -> Vec<Tool> {
    let mut programs: Vec<&'static str> = Vec::new();
    for id in CAPABILITY_IDS {
        for program in capability_tools(id) {
            if !programs.contains(program) {
                programs.push(program);
            }
        }
    }

    programs.push("tmux");

    return programs
        .into_iter()
        .map(|program| {
            return Tool {
                program,
                found: find_executable(program),
            };
        })
        .collect();
}

fn dependency_checks(
    tools: &[Tool],
    required: &BTreeSet<&str>,
    home: Option<&Path>,
) -> Vec<DoctorCheck> {
    return tools
        .iter()
        .map(|tool| {
            let name = tool.program.to_lowercase();
            if let Some(path) = &tool.found {
                return check(
                    DEPENDENCIES,
                    &name,
                    DoctorStatus::Ok,
                    display_path(path, home),
                );
            }

            let users: Vec<&str> = CAPABILITY_IDS
                .iter()
                .copied()
                .filter(|id| return capability_tools(id).contains(&tool.program))
                .collect();

            let status = if users.iter().any(|id| return required.contains(id)) {
                DoctorStatus::Fail
            } else if users
                .iter()
                .any(|id| return DEFAULT_PATH_CAPABILITIES.contains(id))
            {
                DoctorStatus::Warn
            } else {
                DoctorStatus::Info
            };

            let needed_for =
                if users.is_empty() { "termtrace tmux".to_owned() } else { users.join(", ") };

            return DoctorCheck {
                fix: Some(manual(install_hint(tool.program))),
                ..check(
                    DEPENDENCIES,
                    &name,
                    status,
                    format!("Not found; needed for {needed_for}"),
                )
            };
        })
        .collect();
}

fn install_hint(program: &str) -> String {
    return match program {
        "ffmpeg" | "ffprobe" => {
            "Install FFmpeg with libx264, libx264rgb, and libass support".to_owned()
        }
        "ghostty" => "Install Ghostty".to_owned(),
        "Xvfb" => "Install the Xvfb virtual X server".to_owned(),
        "dbus-run-session" => "Install D-Bus".to_owned(),
        "import" => "Install ImageMagick".to_owned(),
        "ssh-keygen" => "Install an OpenSSH client".to_owned(),
        _ => format!("Install {program}"),
    };
}

fn launch_hook_check(tools: &[Tool]) -> DoctorCheck {
    const NAME: &str = "tmux-launch-hook";

    let Some(tmux) = tools
        .iter()
        .find(|tool| return tool.program == "tmux")
        .and_then(|tool| return tool.found.as_deref())
    else {
        return skipped(DEPENDENCIES, NAME, vec![dependency_id("tmux")]);
    };

    let supported = TempBuilder::new()
        .prefix("termtrace-doctor-")
        .tempdir()
        .map_err(|error| return error.to_string())
        .and_then(|directory| {
            return launch_hook_supported(tmux, &directory.path().join("tmux"))
                .map_err(|error| return error.to_string());
        });

    return match supported {
        Ok(true) => check(
            DEPENDENCIES,
            NAME,
            DoctorStatus::Ok,
            "pane-launch-command is available",
        ),
        Ok(false) => check(
            DEPENDENCIES,
            NAME,
            DoctorStatus::Info,
            "Stock tmux; only tmux record --launch-hook needs the pane-launch-command patch",
        ),
        Err(error) => DoctorCheck {
            fix: Some(manual(
                "Check that tmux starts with `tmux -f /dev/null start-server`",
            )),
            ..check(
                DEPENDENCIES,
                NAME,
                DoctorStatus::Warn,
                format!("Could not probe the launch hook: {error}"),
            )
        },
    };
}

struct StateDiagnosis {
    check: DoctorCheck,
    repairs: Vec<PathBuf>,
}

fn diagnose_state(home: Option<&Path>) -> StateDiagnosis {
    let root = match SessionRegistry::default_state_root() {
        Ok(root) => root,
        Err(error) => {
            return StateDiagnosis {
                check: DoctorCheck {
                    fix: Some(manual(format!(
                        "Set {STATE_DIR_ENV} to a directory you own"
                    ))),
                    ..check(
                        STATE,
                        "root",
                        DoctorStatus::Fail,
                        format!("No state directory can be selected: {error}"),
                    )
                },
                repairs: Vec::new(),
            };
        }
    };

    let mut problems = Vec::new();
    let mut repairs = Vec::new();

    let mut repairable = true;
    for directory in SessionRegistry::owner_directories(&root) {
        let metadata = match fs::symlink_metadata(&directory) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => {
                problems.push(format!("{}: {error}", display_path(&directory, home)));
                repairable = false;
                continue;
            }
        };

        if SessionRegistry::validate_owner_directory(&directory).is_ok() {
            continue;
        }

        let shown = display_path(&directory, home);
        let mode = metadata.mode() & 0o777;

        if metadata.file_type().is_symlink() {
            problems.push(format!("{shown} is a symbolic link"));
            repairable = false;
        } else if !metadata.is_dir() {
            problems.push(format!("{shown} is not a directory"));
            repairable = false;
        } else if metadata.uid() != getuid().as_raw() {
            problems.push(format!("{shown} is owned by uid {}", metadata.uid()));
            repairable = false;
        } else {
            problems.push(format!("{shown} has mode {mode:04o}; expected 0700"));
            repairs.push(directory);
        }
    }

    let shown_root = display_path(&root, home);
    let check = match problems.as_slice() {
        [] if !root.exists() => check(
            STATE,
            "root",
            DoctorStatus::Info,
            format!("{shown_root} does not exist yet; it is created on first use"),
        ),
        [] => check(
            STATE,
            "root",
            DoctorStatus::Ok,
            format!("{shown_root} is owner-only"),
        ),
        [problem] => DoctorCheck {
            fix: Some(state_fix(repairable, &repairs, home)),
            ..check(STATE, "root", DoctorStatus::Fail, problem.clone())
        },
        _ => DoctorCheck {
            details: problems.clone(),
            fix: Some(state_fix(repairable, &repairs, home)),
            ..check(
                STATE,
                "root",
                DoctorStatus::Fail,
                format!("{} state directories are not owner-only", problems.len()),
            )
        },
    };

    if !repairable {
        repairs.clear();
    }

    return StateDiagnosis { check, repairs };
}

fn state_fix(repairable: bool, repairs: &[PathBuf], home: Option<&Path>) -> DoctorFix {
    if !repairable {
        return manual("Move the listed paths aside; Termtrace recreates them owner-only");
    }

    let paths: Vec<String> = repairs
        .iter()
        .map(|path| return shell_path(path, home))
        .collect();

    return DoctorFix {
        command: Some(format!("chmod 700 {}", paths.join(" "))),
        manual: None,
        automatic: true,
    };
}

fn aliases_check(state_root: &DoctorCheck) -> DoctorCheck {
    const NAME: &str = "aliases";

    if state_root.status == DoctorStatus::Fail {
        return skipped(STATE, NAME, vec![STATE_ROOT_ID.to_owned()]);
    }

    let Ok(root) = SessionRegistry::default_state_root() else {
        return skipped(STATE, NAME, vec![STATE_ROOT_ID.to_owned()]);
    };
    let [_, registry, ..] = SessionRegistry::owner_directories(&root);

    if !registry.exists() {
        return check(STATE, NAME, DoctorStatus::Ok, "No aliases are registered");
    }
    return match SessionRegistry::open(&root)
        .and_then(|registry| return registry.reap_stale_aliases(false))
    {
        Ok(stale) if stale.is_empty() => check(STATE, NAME, DoctorStatus::Ok, "No stale aliases"),
        Ok(stale) => DoctorCheck {
            details: stale.clone(),
            fix: Some(DoctorFix {
                command: Some(DOCTOR_FIX.to_owned()),
                manual: None,
                automatic: true,
            }),
            ..check(
                STATE,
                NAME,
                DoctorStatus::Warn,
                format!(
                    "{} stale {} still {} {}",
                    stale.len(),
                    plural(stale.len(), "alias", "aliases"),
                    plural(stale.len(), "holds its", "hold their"),
                    plural(stale.len(), "name", "names"),
                ),
            )
        },
        Err(error) => DoctorCheck {
            fix: Some(manual("Run `termtrace status` to see the registry error")),
            ..check(
                STATE,
                NAME,
                DoctorStatus::Warn,
                format!("Could not read the session registry: {error}"),
            )
        },
    };
}

fn tmux_integration_check(tools: &[Tool], home: Option<&Path>) -> Option<DoctorCheck> {
    const NAME: &str = "tmux-integration";

    let Some(tmux) = tools
        .iter()
        .find(|tool| return tool.program == "tmux")
        .and_then(|tool| return tool.found.clone())
    else {
        return Some(skipped(STATE, NAME, vec![dependency_id("tmux")]));
    };
    let client = TmuxClient::new(tmux, TmuxSocket::Default);
    let version = match client.option_text(tmux_options::VERSION) {
        Ok(version) => version,
        Err(error) if error.is_server_unavailable() => {
            return Some(check(
                STATE,
                NAME,
                DoctorStatus::Skip,
                "No tmux server is running",
            ));
        }
        Err(error) => {
            return Some(DoctorCheck {
                fix: Some(manual("Check that `tmux show-options -g` succeeds")),
                ..check(
                    STATE,
                    NAME,
                    DoctorStatus::Warn,
                    format!("Could not read the tmux server options: {error}"),
                )
            });
        }
    };

    if version.is_empty() {
        return None;
    }

    if let Err(error) = client.require_supported_version() {
        return Some(DoctorCheck {
            fix: Some(manual("Install tmux 3.6 or newer")),
            ..check(STATE, NAME, DoctorStatus::Fail, error.to_string())
        });
    }

    let reenable = || return manual("Run `termtrace tmux disable`, then `termtrace tmux enable`");
    if version != INTEGRATION_VERSION {
        return Some(DoctorCheck {
            fix: Some(reenable()),
            ..check(
                STATE,
                NAME,
                DoctorStatus::Warn,
                format!(
                    "Integration version {version} is enabled; this termtrace uses version {INTEGRATION_VERSION}"
                ),
            )
        });
    }

    let mut hooks = Vec::new();
    for option in [tmux_options::TERMTRACE, tmux_options::TMUX] {
        match client.option(option) {
            Ok(value) => hooks.push(PathBuf::from(OsString::from_vec(value))),
            Err(error) => {
                return Some(DoctorCheck {
                    fix: Some(reenable()),
                    ..check(
                        STATE,
                        NAME,
                        DoctorStatus::Warn,
                        format!("Could not read the integration options: {error}"),
                    )
                });
            }
        }
    }

    let broken: Vec<String> = hooks
        .iter()
        .filter(|path| return !is_executable(path))
        .map(|path| return display_path(path, home))
        .collect();

    return Some(match broken.as_slice() {
        [] => check(
            STATE,
            NAME,
            DoctorStatus::Ok,
            format!(
                "Enabled; hooks run {}",
                hooks
                    .first()
                    .map_or_else(String::new, |path| return display_path(path, home))
            ),
        ),
        [path] => DoctorCheck {
            fix: Some(reenable()),
            ..check(
                STATE,
                NAME,
                DoctorStatus::Fail,
                format!("Hooks run {path}, which is not an executable"),
            )
        },
        _ => DoctorCheck {
            details: broken.clone(),
            fix: Some(reenable()),
            ..check(
                STATE,
                NAME,
                DoctorStatus::Fail,
                "Hooks run executables that no longer exist",
            )
        },
    });
}

fn record_smoke() -> StdResult<(), String> {
    let directory = TempBuilder::new()
        .prefix("termtrace-doctor-")
        .tempdir()
        .map_err(|error| return error.to_string())?;

    let session = directory.path().join("smoke.termtrace");
    let options = RecordOptions {
        failure_policy: RecordingFailurePolicy::Stop,
        output_directory: session.clone(),
        creation_alias: None,
        lineage: None,
        import_provenance: None,
        tmux_pane: None,
        command: ["/bin/sh", "-c", "printf termtrace"]
            .into_iter()
            .map(OsString::from)
            .collect(),
        working_directory: Some(directory.path().to_path_buf()),
        cols: 80,
        rows: 24,
        cell_width_px: 9,
        cell_height_px: 18,
        geometry_provenance: GeometryProvenance::default(),
        ghostty_profile: None,
        visual_profile: None,
        term: "xterm-ghostty".to_owned(),
        environment: EnvironmentSpec::default(),
        colors: TerminalColors::default(),
        forward_stdin: false,
        continue_after_output_mirror_closes: false,
        durability_policy: DurabilityPolicy::default(),
        provenance_policy: ProvenancePolicy::default(),
        resource_limits: ResourceLimits::default(),
    };
    let outcome =
        record_with_output(&options, &mut io::sink()).map_err(|error| return error.to_string())?;

    if outcome.exit_code != Some(0) {
        return Err(format!(
            "the recorded shell ended with {}",
            outcome
                .signal_name
                .unwrap_or_else(|| return format!("exit code {:?}", outcome.exit_code))
        ));
    }

    let report = verify_session(&session, VerificationLevel::Content);
    if !report.valid {
        return Err(report.findings.first().map_or_else(
            || return "the recorded session failed verification".to_owned(),
            |finding| {
                return format!(
                    "the recorded session failed verification: {}",
                    finding.message
                );
            },
        ));
    }
    return Ok(());
}

fn smoke_check(result: StdResult<(), String>, capabilities: &CapabilityReport) -> DoctorCheck {
    const NAME: &str = "record";

    let needs: Vec<String> = [CORE_CAPABILITY, SEMANTIC_CAPABILITY]
        .into_iter()
        .filter(|id| return !capabilities.is_ready(id))
        .map(|id| return format!("capabilities.{id}"))
        .collect();

    if !needs.is_empty() {
        return skipped(SMOKE, NAME, needs);
    }
    return match result {
        Ok(()) => check(
            SMOKE,
            NAME,
            DoctorStatus::Ok,
            "Recorded a shell in a pseudo-terminal and verified the session",
        ),
        Err(error) => DoctorCheck {
            fix: Some(manual(
                "Run `termtrace record -- /bin/sh -c 'printf termtrace'` to reproduce",
            )),
            ..check(SMOKE, NAME, DoctorStatus::Fail, sentence(&error))
        },
    };
}

fn replace_check(checks: &mut [DoctorCheck], replacement: DoctorCheck) {
    if let Some(check) = checks
        .iter_mut()
        .find(|check| return check.id == replacement.id)
    {
        *check = replacement;
    }
}

fn capability_checks(
    report: &CapabilityReport,
    tools: &[Tool],
    required: &BTreeSet<&str>,
) -> Vec<DoctorCheck> {
    let semantic_ready = report.is_ready(SEMANTIC_CAPABILITY);

    return report
        .capabilities
        .iter()
        .map(|capability| {
            return DoctorCheck {
                code: Some(capability.code.clone()),
                ..capability_check(capability, tools, required, semantic_ready)
            };
        })
        .collect();
}

fn capability_check(
    capability: &Capability,
    tools: &[Tool],
    required: &BTreeSet<&str>,
    semantic_ready: bool,
) -> DoctorCheck {
    let id = capability.id.as_str();
    if capability.status == CapabilityStatus::Ready {
        return check(
            CAPABILITIES,
            id,
            DoctorStatus::Ok,
            sentence(&capability.detail),
        );
    }

    let missing: Vec<String> = tools
        .iter()
        .filter(|tool| {
            return tool.found.is_none() && capability_tools(id).contains(&tool.program);
        })
        .map(|tool| return dependency_id(tool.program))
        .collect();

    if !missing.is_empty() {
        return skipped(CAPABILITIES, id, missing);
    }

    if id == SEMANTIC_VIDEO_CAPABILITY && !semantic_ready {
        return skipped(
            CAPABILITIES,
            id,
            vec![format!("capabilities.{SEMANTIC_CAPABILITY}")],
        );
    }

    let status = if [CORE_CAPABILITY, SEMANTIC_CAPABILITY].contains(&id) || required.contains(id) {
        DoctorStatus::Fail
    } else {
        DoctorStatus::Warn
    };

    return DoctorCheck {
        fix: capability
            .remediation
            .as_deref()
            .map(|remediation| return manual(sentence(remediation))),
        ..check(CAPABILITIES, id, status, sentence(&capability.detail))
    };
}

fn check(group: &str, name: &str, status: DoctorStatus, message: impl Into<String>) -> DoctorCheck {
    return DoctorCheck {
        id: format!("{}.{name}", group.to_lowercase()),
        group: group.to_owned(),
        status,
        code: None,
        message: message.into(),
        details: Vec::new(),
        needs: Vec::new(),
        fix: None,
    };
}

fn skipped(group: &str, name: &str, needs: Vec<String>) -> DoctorCheck {
    let message = format!("needs {}", needs.join(", "));

    return DoctorCheck {
        needs,
        ..check(group, name, DoctorStatus::Skip, message)
    };
}

fn dependency_id(program: &str) -> String {
    return format!("{}.{}", DEPENDENCIES.to_lowercase(), program.to_lowercase());
}

fn manual(step: impl Into<String>) -> DoctorFix {
    return DoctorFix {
        command: None,
        manual: Some(step.into()),
        automatic: false,
    };
}

fn sentence(text: &str) -> String {
    let mut characters = text.chars();

    return characters.next().map_or_else(String::new, |first| {
        return first.to_uppercase().chain(characters).collect();
    });
}

fn plural<'word>(count: usize, one: &'word str, many: &'word str) -> &'word str {
    return if count == 1 { one } else { many };
}

fn display_path(path: &Path, home: Option<&Path>) -> String {
    return match home.and_then(|home| return path.strip_prefix(home).ok()) {
        Some(relative) if relative.as_os_str().is_empty() => "~".to_owned(),
        Some(relative) => format!("~/{}", relative.display()),
        None => path.display().to_string(),
    };
}

fn shell_path(path: &Path, home: Option<&Path>) -> String {
    let shown = display_path(path, home);
    let safe = |text: &str| {
        return text.chars().all(|character| {
            return character.is_ascii_alphanumeric() || "~/._-+:@%,".contains(character);
        });
    };
    if safe(&shown) {
        return shown;
    }
    return format!("'{}'", path.display().to_string().replace('\'', r"'\''"));
}

struct TextStyle {
    color: bool,
    width: Option<usize>,
}

impl TextStyle {
    fn detect() -> Self {
        let interactive = io::stdout().is_terminal();

        return Self {
            color: interactive && !should_disable_color(),
            width: interactive
                .then(terminal::size)
                .and_then(|size| return size.ok())
                .map(|(columns, _)| return usize::from(columns)),
        };
    }

    fn status(&self, status: DoctorStatus) -> String {
        let (word, code) = match status {
            DoctorStatus::Ok => ("ok", "32"),
            DoctorStatus::Info => ("info", "94"),
            DoctorStatus::Warn => ("warn", "33"),
            DoctorStatus::Fail => ("fail", "31"),
            DoctorStatus::Skip => ("skip", "90"),
        };
        let padding = " ".repeat(4 - word.len());
        if self.color {
            return format!("\x1b[{code}m{word}\x1b[0m{padding}");
        }
        return format!("{word}{padding}");
    }

    fn wrap(&self, text: &str, indent: usize) -> String {
        let Some(width) = self.width.filter(|width| return *width > indent + 20) else {
            return text.to_owned();
        };
        let available = width - indent;
        let mut lines: Vec<String> = Vec::new();
        let mut line = String::new();

        for word in text.split(' ') {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > available {
                lines.push(mem::take(&mut line));
            }

            if !line.is_empty() {
                line.push(' ');
            }

            line.push_str(word);
        }

        lines.push(line);

        return lines.join(&format!("\n{}", " ".repeat(indent)));
    }
}

fn render(report: &DoctorReport, style: &TextStyle) -> Result<String> {
    let mut output = String::new();
    writeln!(
        output,
        "termtrace {} ({}) {}",
        report.version, report.build.revision, report.build.target
    )
    .into_diagnostic()?;

    let executable = report.install.executable.as_deref().unwrap_or("unknown");
    let method = report
        .install
        .method
        .as_deref()
        .map(|method| return format!(" ({method})"))
        .unwrap_or_default();
    writeln!(output, "  executable  {executable}{method}").into_diagnostic()?;

    let state = report.paths.state.as_ref().map_or_else(
        || return "unavailable".to_owned(),
        |state| {
            return state.source.as_deref().map_or_else(
                || return state.path.clone(),
                |source| return format!("{} (via {source})", state.path),
            );
        },
    );
    writeln!(output, "  state       {state}").into_diagnostic()?;

    for (index, (name, value)) in report.env.iter().enumerate() {
        let label = if index == 0 { "env" } else { "" };
        writeln!(output, "  {label:<10}  {name}={value}").into_diagnostic()?;
    }

    let name_width = report
        .checks
        .iter()
        .map(|check| return check_name(check).len())
        .max()
        .unwrap_or(0);

    let message_column = 2 + 4 + 2 + name_width + 2;
    let fix_column = 2 + 4 + 2;
    let mut group: Option<&str> = None;

    for check in &report.checks {
        if group != Some(check.group.as_str()) {
            group = Some(check.group.as_str());
            write!(output, "\n{}\n", check.group).into_diagnostic()?;
        }

        writeln!(
            output,
            "  {}  {:<name_width$}  {}",
            style.status(check.status),
            check_name(check),
            style.wrap(&check.message, message_column)
        )
        .into_diagnostic()?;

        for detail in &check.details {
            writeln!(
                output,
                "{}{}",
                " ".repeat(message_column),
                style.wrap(detail, message_column)
            )
            .into_diagnostic()?;
        }

        if matches!(check.status, DoctorStatus::Warn | DoctorStatus::Fail)
            && let Some(fix) = &check.fix
        {
            let text = fix_text(fix);
            writeln!(
                output,
                "{}fix: {}",
                " ".repeat(fix_column),
                style.wrap(&text, fix_column + 5)
            )
            .into_diagnostic()?;
        }
    }

    write!(output, "\n{}\n", verdict(&report.summary)).into_diagnostic()?;
    let summary = report.summary;
    writeln!(
        output,
        "{} ok, {} info, {} warn, {} fail, {} skip",
        summary.ok, summary.info, summary.warn, summary.fail, summary.skip
    )
    .into_diagnostic()?;

    return Ok(output);
}

fn check_name(check: &DoctorCheck) -> &str {
    return check
        .id
        .split_once('.')
        .map_or(check.id.as_str(), |(_, name)| return name);
}

fn fix_text(fix: &DoctorFix) -> String {
    let text = fix
        .command
        .as_deref()
        .or(fix.manual.as_deref())
        .unwrap_or_default();

    if fix.automatic && text != DOCTOR_FIX {
        return format!("{text} (or: {DOCTOR_FIX})");
    }
    return text.to_owned();
}

fn verdict(summary: &DoctorSummary) -> String {
    let failures = usize::try_from(summary.fail).unwrap_or(usize::MAX);
    let warnings = usize::try_from(summary.warn).unwrap_or(usize::MAX);

    return match (failures, warnings) {
        (0, 0) => "No problems found.".to_owned(),
        (0, _) => format!(
            "No failures; {warnings} {}.",
            plural(warnings, "warning", "warnings")
        ),
        (_, 0) => format!(
            "{failures} {} attention.",
            plural(failures, "failure needs", "failures need")
        ),
        _ => format!(
            "{failures} {} and {warnings} {} need attention.",
            plural(failures, "failure", "failures"),
            plural(warnings, "warning", "warnings")
        ),
    };
}
