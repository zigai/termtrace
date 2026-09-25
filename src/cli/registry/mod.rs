use crate::cli::output::ResponseWriter;

pub(super) mod args;

use crate::cli::{
    output::{write_json_line, write_json_pretty, write_stdout},
    registry::args::{GcArgs, ListArgs, RegisterArgs, RemoveArgs, ResolveArgs},
};
use args::RegistrySubcommand;
use miette::{IntoDiagnostic as _, Result};
use std::{
    ffi::OsString, fmt::Write as _, os::unix::ffi::OsStringExt as _, path::PathBuf,
    process::ExitCode,
};
use termtrace_engine::registry::SessionRegistry;
use termtrace_model::registry::RegistryEntry;

pub(super) fn run_register(
    response: &mut ResponseWriter,
    arguments: RegisterArgs,
) -> Result<ExitCode> {
    let registry = SessionRegistry::open_default().into_diagnostic()?;
    let entry = registry
        .register(&arguments.name, &arguments.session)
        .into_diagnostic()?;

    let display_path = entry.session_path();

    return write_json_line(
        response,
        &RegistryMutationResult {
            entry: &entry,
            canonical_session_display: display_path.display().to_string(),
        },
    );
}

#[derive(serde::Serialize)]
struct RegistryMutationResult<'entry> {
    #[serde(flatten)]
    entry: &'entry RegistryEntry,
    canonical_session_display: String,
}

pub(super) fn run_resolve(
    response: &mut ResponseWriter,
    arguments: ResolveArgs,
) -> Result<ExitCode> {
    let registry = SessionRegistry::open_default().into_diagnostic()?;
    let entry = registry.resolve(&arguments.name).into_diagnostic()?;

    if arguments.json {
        return write_json_pretty(response, &entry);
    }

    let mut path = entry.session_path().into_os_string().into_vec();
    path.push(b'\n');

    return write_stdout(response, path);
}

pub(super) fn run_list(response: &mut ResponseWriter, arguments: ListArgs) -> Result<ExitCode> {
    let registry = SessionRegistry::open_default().into_diagnostic()?;
    let entries = registry.list(arguments.all).into_diagnostic()?;

    if arguments.json {
        let mut encoded = serde_json::to_vec_pretty(&entries).into_diagnostic()?;
        encoded.push(b'\n');

        return write_stdout(response, encoded);
    }

    if entries.is_empty() {
        return write_stdout(response, "No sessions found.\n");
    }

    let rows = entries
        .into_iter()
        .map(|entry| {
            let path = PathBuf::from(OsString::from_vec(
                entry.canonical_session.as_bytes().to_vec(),
            ));

            return (
                entry.alias,
                entry.lifecycle.as_str(),
                entry.session_id.to_string(),
                path.display().to_string(),
            );
        })
        .collect::<Vec<_>>();

    let alias_width = rows
        .iter()
        .fold("ALIAS".len(), |width, row| return width.max(row.0.len()));
    let lifecycle_width = rows.iter().fold("LIFECYCLE".len(), |width, row| {
        return width.max(row.1.len());
    });
    let session_id_width = rows.iter().fold("SESSION ID".len(), |width, row| {
        return width.max(row.2.len());
    });

    let mut output = String::new();

    let header = format!(
        "{:<alias_width$}  {:<lifecycle_width$}  {:<session_id_width$}  CANONICAL SESSION",
        "ALIAS", "LIFECYCLE", "SESSION ID",
    );

    let rule = "─".repeat(header.chars().count());

    writeln!(output, "{header}").into_diagnostic()?;
    writeln!(output, "{rule}").into_diagnostic()?;
    for (alias, lifecycle, session_id, path) in rows {
        writeln!(
            output,
            "{alias:<alias_width$}  {lifecycle:<lifecycle_width$}  {session_id:<session_id_width$}  {path}",
        )
        .into_diagnostic()?;
    }

    return write_stdout(response, output);
}

pub(super) fn run_remove(response: &mut ResponseWriter, arguments: RemoveArgs) -> Result<ExitCode> {
    let registry = SessionRegistry::open_default().into_diagnostic()?;
    let entry = registry.remove(&arguments.name).into_diagnostic()?;
    let canonical_session_display = entry.session_path().display().to_string();

    return write_json_line(
        response,
        &serde_json::json!({
            "alias": entry.alias,
            "canonical_session": entry.canonical_session,
            "canonical_session_display": canonical_session_display,
            "session_id": entry.session_id,
            "evidence_preserved": true,
        }),
    );
}

pub(super) fn run_gc(response: &mut ResponseWriter, arguments: GcArgs) -> Result<ExitCode> {
    let registry = SessionRegistry::open_default().into_diagnostic()?;
    let report = registry
        .garbage_collect_with_quota(
            arguments.older_than_days,
            arguments.keep_latest,
            arguments.max_completed_bytes,
            arguments.apply,
        )
        .into_diagnostic()?;

    return write_json_line(response, &report);
}

pub(super) fn dispatch(
    response: &mut ResponseWriter,
    command: RegistrySubcommand,
) -> Result<ExitCode> {
    return match command {
        RegistrySubcommand::Register(arguments) => run_register(response, arguments),
        RegistrySubcommand::Resolve(arguments) => run_resolve(response, arguments),
        RegistrySubcommand::List(arguments) => run_list(response, arguments),
        RegistrySubcommand::Remove(arguments) => run_remove(response, arguments),
        RegistrySubcommand::Gc(arguments) => run_gc(response, arguments),
    };
}
