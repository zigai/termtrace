use crate::cli::output::ResponseWriter;

pub(super) mod args;

use crate::cli::{
    artifacts::run_bundle_verify,
    integrity::args::{RecoverArgs, SignArgs, VerifyArgs, VerifyLevelArg},
    output::{paginate, stable_enum_label, write_json_line, write_json_pretty, write_stdout},
    session_helpers::resolve_session,
};
use args::IntegritySubcommand;
use miette::{IntoDiagnostic as _, Result};
use std::{
    fmt::Write as _, fs::DirBuilder, mem::take, os::unix::fs::DirBuilderExt as _, process::ExitCode,
};
use termtrace_evidence::{
    event_log::EventLog,
    receipt::create_ssh_receipt,
    session::{recover_session, verify_session_with_trust},
};

pub(super) fn run_verify(response: &mut ResponseWriter, arguments: VerifyArgs) -> Result<ExitCode> {
    if arguments.allowed_signers.is_some() && !matches!(arguments.level, VerifyLevelArg::Signature)
    {
        return Err(miette::miette!(
            "--allowed-signers requires --level signature"
        ));
    }

    let mut report = verify_session_with_trust(
        resolve_session(arguments.session)?.path,
        arguments.level.into(),
        arguments.allowed_signers.as_deref(),
    );

    let valid = report.valid;
    let (findings, page, partial) = paginate(take(&mut report.findings), &arguments.page);
    report.findings = findings;
    report.output_page = partial.then_some(page.clone());

    if arguments.json {
        write_json_pretty(response, &report)?;
    } else {
        let status = if valid { "valid" } else { "invalid" };
        let level = stable_enum_label(&report.level)?;
        let mut output = format!("{status}: {} ({level})\n", report.session.display(),);

        for finding in &report.findings {
            writeln!(output, "{}: {}", finding.code, finding.message).into_diagnostic()?;
        }

        if partial {
            writeln!(
                output,
                "page: {}",
                serde_json::to_string(&page).into_diagnostic()?
            )
            .into_diagnostic()?;
        }

        write_stdout(response, output)?;
    }

    if valid {
        return Ok(ExitCode::SUCCESS);
    }
    return Ok(ExitCode::FAILURE);
}

pub(super) fn run_sign(response: &mut ResponseWriter, arguments: SignArgs) -> Result<ExitCode> {
    let receipt = create_ssh_receipt(&arguments.session, &arguments.key, &arguments.signer)
        .into_diagnostic()?;

    return write_json_line(
        response,
        &serde_json::json!({
            "receipt": receipt,
            "scheme": "openssh",
            "signer_key_id": arguments.signer,
        }),
    );
}

pub(super) fn run_recover(
    response: &mut ResponseWriter,
    arguments: RecoverArgs,
) -> Result<ExitCode> {
    if let Some(parent) = arguments
        .out
        .parent()
        .filter(|parent| return !parent.as_os_str().is_empty())
    {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)
            .into_diagnostic()?;
    }

    let report = if let Some(session) = arguments.session {
        recover_session(&session, &arguments.out).into_diagnostic()?
    } else if let Some(event_log) = arguments.event_log {
        EventLog::recover_prefix(event_log, arguments.out).into_diagnostic()?
    } else {
        return Err(miette::miette!(
            "recovery requires --session or --event-log"
        ));
    };

    return write_json_line(response, &report);
}

pub(super) fn dispatch(
    response: &mut ResponseWriter,
    command: IntegritySubcommand,
) -> Result<ExitCode> {
    return match command {
        IntegritySubcommand::Verify(arguments) => run_verify(response, arguments),
        IntegritySubcommand::VerifyBundle(arguments) => run_bundle_verify(response, arguments),
        IntegritySubcommand::Sign(arguments) => run_sign(response, arguments),
        IntegritySubcommand::Recover(arguments) => run_recover(response, arguments),
    };
}
