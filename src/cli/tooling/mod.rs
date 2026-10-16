use crate::cli::output::ResponseWriter;

pub(super) mod args;

use crate::cli::command::completion;
use crate::cli::{
    agent::serve as serve_driver,
    output::{write_json_line, write_stdout},
    tooling::args::{CompletionsArgs, SchemaArgs, VersionArgs},
};
use clap_complete::generate;
use miette::{IntoDiagnostic as _, Report, Result};
use std::{
    env,
    io::{self, BufReader},
    os::unix::ffi::OsStrExt as _,
    process::ExitCode,
    sync::atomic::{AtomicBool, Ordering},
};
use termtrace_engine::Error as EngineError;
use termtrace_model::{
    execution::event::EVENT_LOG_VERSION,
    protocol::control::CONTROL_PROTOCOL_VERSION,
    terminal::replay::{GHOSTTY_REVISION, STATE_FORMAT_VERSION},
    value::ByteString,
};
use termtrace_service::{system::SystemService, Error as ServiceError};

mod doctor;

pub(super) use doctor::run_doctor;

pub(super) fn require_capability(id: &str) -> termtrace_sdk::Result<()> {
    return SystemService::require(id, &AtomicBool::new(false));
}

pub(super) fn require_cli_capability(id: &str, cancelled: &AtomicBool) -> Result<()> {
    let result = SystemService::require(id, cancelled);
    if cancelled.load(Ordering::SeqCst) {
        return Err(miette::miette!("the `{id}` capability probe was cancelled"));
    }
    return result.map_err(capability_report);
}

/// Render a service failure, naming the doctor command when a capability is blocked.
pub(super) fn capability_report(error: ServiceError) -> Report {
    if let ServiceError::Engine(EngineError::CapabilityBlocked {
        id,
        code,
        detail,
        remediation,
    }) = &error
    {
        return miette::miette!(
            "required capability `{id}` is blocked ({code}): {detail}; fix: {remediation}; inspect with `termtrace doctor --require {id}`"
        );
    }
    return Report::from_err(error);
}

pub(super) fn run_version(
    response: &mut ResponseWriter,
    arguments: VersionArgs,
) -> Result<ExitCode> {
    if arguments.json {
        let value = serde_json::json!({
            "termtrace_version": env!("CARGO_PKG_VERSION"),
            "build_revision": env!("TERMTRACE_BUILD_REVISION"),
            "ghostty_vt_revision": GHOSTTY_REVISION,
            "event_log_version": EVENT_LOG_VERSION,
            "state_format_version": STATE_FORMAT_VERSION,
            "control_protocol_version": CONTROL_PROTOCOL_VERSION,
            "target_os": env::consts::OS,
            "target_arch": env::consts::ARCH,
        });

        let mut encoded = serde_json::to_vec_pretty(&value).into_diagnostic()?;
        encoded.push(b'\n');

        return write_stdout(response, encoded);
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

pub(super) fn run_completions(
    response: &mut ResponseWriter,
    arguments: CompletionsArgs,
) -> Result<ExitCode> {
    let mut command = completion();
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
