use crate::cli::output::ResponseWriter;

pub(super) mod args;
use args::IntegritySubcommand;

use std::{fmt::Write as _, mem::take, process::ExitCode};

use miette::{IntoDiagnostic as _, Result};
use termtrace_evidence::{
    event_log::EventLog, receipt::create_ssh_receipt, session::verify_session_with_trust,
};

use crate::cli::{
    integrity::args::{RecoverArgs, SignArgs, VerifyArgs, VerifyLevelArg},
    output::{paginate, stable_enum_label, write_json_line, write_stdout},
};

pub(super) fn run_verify(response: &mut ResponseWriter, arguments: VerifyArgs) -> Result<ExitCode> {
    if arguments.allowed_signers.is_some() && !matches!(arguments.level, VerifyLevelArg::Signature)
    {
        return Err(miette::miette!(
            "--allowed-signers requires --level signature"
        ));
    }
    let mut report = verify_session_with_trust(
        arguments.session,
        arguments.level.into(),
        arguments.allowed_signers.as_deref(),
    );
    let valid = report.valid;
    let (findings, page, partial) = paginate(take(&mut report.findings), &arguments.page);
    report.findings = findings;
    report.output_page = partial.then_some(page.clone());
    if arguments.json {
        write_json_line(response, &report)?;
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
    let report = EventLog::recover_prefix(arguments.event_log, arguments.out).into_diagnostic()?;
    return write_json_line(response, &report);
}

pub(super) fn dispatch(
    response: &mut ResponseWriter,
    command: IntegritySubcommand,
) -> Result<ExitCode> {
    return match command {
        IntegritySubcommand::Verify(arguments) => run_verify(response, arguments),
        IntegritySubcommand::Sign(arguments) => run_sign(response, arguments),
        IntegritySubcommand::Recover(arguments) => run_recover(response, arguments),
    };
}
