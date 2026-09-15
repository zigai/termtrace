use crate::cli::output::ResponseWriter;

pub(super) mod args;

use crate::cli::command::completion_command;
use crate::cli::{
    agent::serve as serve_driver,
    output::{write_json_line, write_json_pretty, write_stdout},
    tooling::args::{CompletionsArgs, DoctorArgs, SchemaArgs, VersionArgs},
};
use clap_complete::generate;
use miette::{IntoDiagnostic as _, Result};
use std::{
    env,
    fmt::Write as _,
    io::{self, BufReader},
    os::unix::ffi::OsStrExt as _,
    process::ExitCode,
};
use termtrace_engine::{
    analysis::capability::{
        probe_capabilities as probe_engine_capabilities, require as require_engine_capability,
    },
    Error as EngineError,
};
use termtrace_evidence::receipt::receipt_signing_capability;
use termtrace_media::{
    capability::{presentation_capability, semantic_video_capability},
    native_capture::native_capabilities,
};
use termtrace_model::{
    execution::event::EVENT_LOG_VERSION,
    protocol::capability::{
        CapabilityReport, CapabilityStatus, NATIVE_SCREENSHOT_CAPABILITY, NATIVE_VIDEO_CAPABILITY,
        PRESENTATION_CAPABILITY, RECEIPT_SIGNING_CAPABILITY, SEMANTIC_VIDEO_CAPABILITY,
    },
    protocol::control::CONTROL_PROTOCOL_VERSION,
    terminal::replay::{GHOSTTY_REVISION, STATE_FORMAT_VERSION},
    value::ByteString,
};
use termtrace_service::system::SystemService;

#[must_use]
pub(super) fn probe_capabilities() -> CapabilityReport {
    return SystemService::probe_capabilities();
}

fn require_selected_capability(id: &str) -> termtrace_engine::Result<()> {
    let capability = match id {
        SEMANTIC_VIDEO_CAPABILITY => Some(semantic_video_capability()),
        PRESENTATION_CAPABILITY => Some(presentation_capability()),
        RECEIPT_SIGNING_CAPABILITY => Some(receipt_signing_capability()),
        NATIVE_SCREENSHOT_CAPABILITY | NATIVE_VIDEO_CAPABILITY => native_capabilities(
            id == NATIVE_SCREENSHOT_CAPABILITY,
            id == NATIVE_VIDEO_CAPABILITY,
        )
        .next(),
        _ => None,
    };
    let Some(capability) = capability else {
        return require_engine_capability(&probe_engine_capabilities([]), id);
    };

    if capability.status == CapabilityStatus::Ready {
        return Ok(());
    }
    return Err(EngineError::CapabilityBlocked {
        id: capability.id,
        code: capability.code,
        detail: capability.detail,
        remediation: capability
            .remediation
            .unwrap_or_else(|| return "no automatic remediation is available".to_owned()),
    });
}

pub(super) fn require_capability(id: &str) -> termtrace_sdk::Result<()> {
    return require_selected_capability(id).map_err(termtrace_sdk::Error::from);
}

pub(super) fn require_cli_capability(id: &str) -> Result<()> {
    match require_selected_capability(id) {
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

pub(super) fn run_doctor(response: &mut ResponseWriter, arguments: DoctorArgs) -> Result<ExitCode> {
    let report = probe_capabilities();

    if arguments.json {
        write_json_pretty(response, &report)?;
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
