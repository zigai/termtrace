use crate::cli::output::ResponseWriter;

pub(super) mod args;
use args::ToolingSubcommand;

use std::{
    env,
    ffi::OsStr,
    fmt::Write as _,
    fs::Permissions,
    io::{self, BufReader, Read as _},
    os::unix::{ffi::OsStrExt as _, fs::PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode, ExitStatus},
};

use clap_complete::generate;
use miette::{IntoDiagnostic as _, Result};
use termtrace_engine::{
    Error as EngineError,
    analysis::capability::{
        probe_capabilities as probe_engine_capabilities, require as require_engine_capability,
    },
};
use termtrace_evidence::{artifact::write_secure, receipt::receipt_signing_capability};
use termtrace_media::{
    capability::derived_media_capabilities, native_capture::native_capabilities,
};
use termtrace_model::{
    execution::event::EVENT_LOG_VERSION,
    protocol::capability::{CapabilityReport, CapabilityStatus},
    protocol::control::CONTROL_PROTOCOL_VERSION,
    terminal::replay::{GHOSTTY_REVISION, STATE_FORMAT_VERSION},
    value::ByteString,
};

use crate::cli::command::completion_command;

use crate::cli::{
    agent::serve as serve_driver,
    output::{write_json_line, write_stdout},
    tooling::args::{
        ClientArgs, ClientCommand, ClientRunArgs, CompletionsArgs, DoctorArgs, SchemaArgs,
        VersionArgs,
    },
};

#[must_use]
pub(super) fn probe_capabilities() -> CapabilityReport {
    let mut external = derived_media_capabilities().to_vec();
    external.extend(native_capabilities());
    external.push(receipt_signing_capability());
    return probe_engine_capabilities(external);
}

pub(super) fn require_capability(id: &str) -> termtrace_sdk::Result<()> {
    return require_engine_capability(&probe_capabilities(), id)
        .map_err(termtrace_sdk::Error::from);
}
pub(super) fn require_cli_capability(id: &str) -> Result<()> {
    match require_engine_capability(&probe_capabilities(), id) {
        Ok(()) => return Ok(()),
        Err(EngineError::CapabilityBlocked {
            id,
            code,
            detail,
            remediation,
        }) => {
            return Err(miette::miette!(
                "required capability `{id}` is blocked ({code}): {detail}; fix: {remediation}; inspect with `termtrace doctor --require {id} --explain`"
            ));
        }
        Err(error) => return Err(error).into_diagnostic(),
    }
}

pub(super) fn run_version(
    response: &mut ResponseWriter,
    arguments: VersionArgs,
) -> Result<ExitCode> {
    if arguments.json {
        return write_json_line(
            response,
            &serde_json::json!({
                "termtrace_version": env!("CARGO_PKG_VERSION"),
                "build_revision": env!("TERMTRACE_BUILD_REVISION"),
                "ghostty_vt_revision": GHOSTTY_REVISION,
                "event_log_version": EVENT_LOG_VERSION,
                "state_format_version": STATE_FORMAT_VERSION,
                "control_protocol_version": CONTROL_PROTOCOL_VERSION,
                "target_os": env::consts::OS,
                "target_arch": env::consts::ARCH,
            }),
        );
    }
    return write_stdout(
        response,
        format!(
            "termtrace {}\nbuild {}\nlibghostty-vt {GHOSTTY_REVISION}\nevent-log v{EVENT_LOG_VERSION}\nstate-format v{STATE_FORMAT_VERSION}\ncontrol-protocol v{CONTROL_PROTOCOL_VERSION}\n",
            env!("CARGO_PKG_VERSION"),
            env!("TERMTRACE_BUILD_REVISION")
        ),
    );
}

pub(super) fn run_doctor(response: &mut ResponseWriter, arguments: DoctorArgs) -> Result<ExitCode> {
    let report = probe_capabilities();
    if arguments.json {
        write_json_line(response, &report)?;
    } else {
        write_stdout(response, capability_text(&report, arguments.explain)?)?;
    }
    if arguments
        .require
        .iter()
        .any(|required| return !report.is_ready(required))
    {
        return Ok(ExitCode::FAILURE);
    }
    return Ok(ExitCode::SUCCESS);
}

fn capability_text(report: &CapabilityReport, explain: bool) -> Result<String> {
    let mut output = String::new();
    for capability in &report.capabilities {
        writeln!(
            output,
            "{:<20} {:<7} {}",
            capability.id,
            match capability.status {
                CapabilityStatus::Ready => "ready",
                CapabilityStatus::Blocked => "blocked",
            },
            capability.detail
        )
        .into_diagnostic()?;
        if explain && let Some(remediation) = &capability.remediation {
            writeln!(output, "  fix: {remediation}").into_diagnostic()?;
        }
    }
    return Ok(output);
}

pub(super) fn run_client(response: &mut ResponseWriter, arguments: ClientArgs) -> Result<ExitCode> {
    let assets = client_assets_root()?;
    return match arguments.command {
        ClientCommand::Status => write_json_line(
            response,
            &serde_json::json!({
                "kind": "client_assets",
                "root": assets,
                "python": {
                    "package": assets.join("python"),
                    "runtime": if executable_on_path("uv") { "uv_isolated" } else { "python_path_fallback" },
                    "uv_available": executable_on_path("uv"),
                    "python_available": executable_on_path("python3"),
                },
                "typescript": {
                    "package": assets.join("typescript"),
                    "runtime": "bun_private_package_mapping",
                    "bun_available": executable_on_path("bun"),
                },
                "worktree_files_created": false,
            }),
        ),
        ClientCommand::Python(arguments) => run_python_client(&assets, arguments),
        ClientCommand::Typescript(arguments) => run_typescript_client(&assets, arguments),
    };
}

fn run_python_client(assets: &Path, arguments: ClientRunArgs) -> Result<ExitCode> {
    let (script, temporary) = prepare_client_script(&arguments.script, "py")?;
    let result = (|| {
        let binary = env::current_exe().into_diagnostic()?;
        if executable_on_path("uv") {
            let status = ProcessCommand::new("uv")
                .args(["run", "--quiet", "--isolated", "--no-project", "--with"])
                .arg(assets.join("python"))
                .args(["python"])
                .arg(&script)
                .args(&arguments.arguments)
                .env("TERMTRACE_BINARY", &binary)
                .status()
                .into_diagnostic()?;
            return process_exit_code(status);
        }
        let python_root = assets.join("python");
        let python_path = match env::var_os("PYTHONPATH") {
            Some(existing) => env::join_paths([python_root.as_os_str(), existing.as_os_str()])
                .map_err(|error| return miette::miette!("invalid PYTHONPATH: {error}"))?,
            None => python_root.into_os_string(),
        };
        let status = ProcessCommand::new("python3")
            .arg(&script)
            .args(&arguments.arguments)
            .env("PYTHONPATH", python_path)
            .env("TERMTRACE_BINARY", binary)
            .status()
            .map_err(|error| {
                return miette::miette!(
                    "Python client execution requires `uv` or Python 3 with `msgspec`: {error}"
                );
            })?;
        return process_exit_code(status);
    })();
    if let Some(temporary) = temporary {
        temporary.close().into_diagnostic()?;
    }
    return result;
}

fn run_typescript_client(assets: &Path, arguments: ClientRunArgs) -> Result<ExitCode> {
    if !executable_on_path("bun") {
        return Err(miette::miette!(
            "TypeScript client execution requires Bun on PATH"
        ));
    }
    let temporary = create_client_temp_directory()?;
    let result = (|| {
        let package = temporary.path().join("node_modules/@termtrace/test");
        fs_err::create_dir_all(&package).into_diagnostic()?;
        fs_err::set_permissions(&package, Permissions::from_mode(0o700)).into_diagnostic()?;
        write_secure(
            &package.join("package.json"),
            br#"{"type":"module","exports":"./index.js"}"#,
        )
        .into_diagnostic()?;
        let package_entry = assets
            .join("typescript/lib/index.js")
            .canonicalize()
            .into_diagnostic()?;
        let package_entry = package_entry.to_str().ok_or_else(|| {
            return miette::miette!("TypeScript client assets path is not valid UTF-8");
        })?;
        let package_entry = serde_json::to_string(package_entry).into_diagnostic()?;
        write_secure(
            &package.join("index.js"),
            format!("export * from {package_entry};\n").as_bytes(),
        )
        .into_diagnostic()?;
        let script = if arguments.script.as_os_str() == OsStr::new("-") {
            let source = temporary.path().join("source");
            fs_err::create_dir(&source).into_diagnostic()?;
            fs_err::set_permissions(&source, Permissions::from_mode(0o700)).into_diagnostic()?;
            let path = source.join("client.ts");
            write_secure(&path, &read_client_stdin()?).into_diagnostic()?;
            path
        } else {
            arguments.script.canonicalize().into_diagnostic()?
        };
        let node_modules = temporary.path().join("node_modules");
        let mut node_paths: Vec<PathBuf> = vec![node_modules];
        if let Some(existing) = env::var_os("NODE_PATH") {
            node_paths.extend(env::split_paths(&existing));
        }
        let node_path = env::join_paths(node_paths).into_diagnostic()?;
        let binary = env::current_exe().into_diagnostic()?;
        let status = ProcessCommand::new("bun")
            .arg("--no-install")
            .arg(&script)
            .args(&arguments.arguments)
            .env("NODE_PATH", node_path)
            .env("TERMTRACE_BINARY", binary)
            .status()
            .into_diagnostic()?;
        return process_exit_code(status);
    })();
    temporary.close().into_diagnostic()?;
    return result;
}

fn create_client_temp_directory() -> Result<tempfile::TempDir> {
    return tempfile::Builder::new()
        .prefix("termtrace-client-")
        .permissions(Permissions::from_mode(0o700))
        .tempdir()
        .into_diagnostic();
}

fn prepare_client_script(
    script: &Path,
    extension: &str,
) -> Result<(PathBuf, Option<tempfile::TempDir>)> {
    if script.as_os_str() != OsStr::new("-") {
        return Ok((script.canonicalize().into_diagnostic()?, None));
    }
    let temporary = create_client_temp_directory()?;
    let path = temporary.path().join(format!("client.{extension}"));
    let result = read_client_stdin().and_then(|bytes| {
        return write_secure(&path, &bytes).into_diagnostic();
    });
    if let Err(error) = result {
        temporary.close().into_diagnostic()?;
        return Err(error);
    }
    return Ok((path, Some(temporary)));
}

fn read_client_stdin() -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    io::stdin()
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .into_diagnostic()?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(miette::miette!(
            "standard-input client program exceeds the 4 MiB limit"
        ));
    }
    return Ok(bytes);
}

fn client_assets_root() -> Result<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os("TERMTRACE_CLIENT_ASSETS") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(binary) = env::current_exe()
        && let Some(directory) = binary.parent()
    {
        candidates.push(directory.join("clients"));
        if let Some(prefix) = directory.parent() {
            candidates.push(prefix.join("share/termtrace/clients"));
        }
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("packages"));
    for candidate in candidates {
        if candidate.join("python/pyproject.toml").is_file()
            && candidate.join("typescript/lib/index.js").is_file()
        {
            return candidate.canonicalize().map_err(|error| {
                return miette::miette!(
                    "could not resolve client asset root {}: {error}",
                    candidate.display()
                );
            });
        }
    }
    return Err(miette::miette!(
        "Termtrace client assets are unavailable; reinstall a release archive that includes `clients/` or set TERMTRACE_CLIENT_ASSETS"
    ));
}

fn executable_on_path(program: &str) -> bool {
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    return env::split_paths(&path).any(|directory| {
        let candidate = directory.join(program);
        return candidate.metadata().is_ok_and(|metadata| {
            return metadata.is_file() && metadata.permissions().mode() & 0o111 != 0;
        });
    });
}

fn process_exit_code(status: ExitStatus) -> Result<ExitCode> {
    if status.success() {
        return Ok(ExitCode::SUCCESS);
    }
    let code = status
        .code()
        .and_then(|code| return u8::try_from(code).ok())
        .unwrap_or(1);
    return Ok(ExitCode::from(code));
}

pub(super) fn run_completions(
    response: &mut ResponseWriter,
    arguments: CompletionsArgs,
) -> Result<ExitCode> {
    let mut command = completion_command();
    let mut output = Vec::new();
    generate(arguments.shell, &mut command, "termtrace", &mut output);
    return write_stdout(response, output);
}

pub(super) fn run_driver() -> Result<ExitCode> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    serve_driver(BufReader::new(stdin.lock()), stdout.lock()).into_diagnostic()?;
    return Ok(ExitCode::SUCCESS);
}

pub(super) fn run_schemas(
    response: &mut ResponseWriter,
    arguments: SchemaArgs,
) -> Result<ExitCode> {
    let directory = arguments.out;
    let replaced_existing_files = termtrace_schema::generate()
        .into_diagnostic()?
        .iter()
        .any(|schema| return directory.join(schema.filename).exists());
    let paths = termtrace_schema::write_all(&directory).into_diagnostic()?;
    return write_json_line(
        response,
        &serde_json::json!({
            "kind": "schema_generation",
            "directory": ByteString::from(directory.as_os_str().as_bytes()),
            "directory_display": directory.display().to_string(),
            "file_count": paths.len(),
            "replaced_existing_files": replaced_existing_files,
        }),
    );
}

pub(super) fn dispatch(
    response: &mut ResponseWriter,
    command: ToolingSubcommand,
) -> Result<ExitCode> {
    return match command {
        ToolingSubcommand::Client(arguments) => run_client(response, arguments),
        ToolingSubcommand::Schemas(arguments) => run_schemas(response, arguments),
        ToolingSubcommand::Completions(arguments) => run_completions(response, arguments),
    };
}
