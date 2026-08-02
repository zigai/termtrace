use super::*;

pub(super) fn run_verify(arguments: VerifyArgs) -> Result<ExitCode> {
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
        write_json_line(&report)?;
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
        write_stdout(output)?;
    }
    if valid {
        return Ok(ExitCode::SUCCESS);
    }
    return Ok(ExitCode::FAILURE);
}

pub(super) fn run_sign(arguments: SignArgs) -> Result<ExitCode> {
    let receipt = create_ssh_receipt(&arguments.session, &arguments.key, &arguments.signer)?;
    return write_json_line(&serde_json::json!({
        "receipt": receipt,
        "scheme": "openssh",
        "signer_key_id": arguments.signer,
    }));
}

pub(super) fn run_recover(arguments: RecoverArgs) -> Result<ExitCode> {
    let report = EventLog::recover_prefix(arguments.event_log, arguments.out)?;
    return write_json_line(&report);
}
