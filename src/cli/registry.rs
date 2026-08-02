use super::*;

pub(super) fn run_register(arguments: RegisterArgs) -> Result<ExitCode> {
    let registry = SessionRegistry::open_default()?;
    let entry = registry.register(&arguments.name, &arguments.session)?;
    let display_path = entry.session_path();
    return write_json_line(&RegistryMutationResult {
        entry: &entry,
        canonical_session_display: display_path.display().to_string(),
    });
}

#[derive(serde::Serialize)]
struct RegistryMutationResult<'entry> {
    #[serde(flatten)]
    entry: &'entry RegistryEntry,
    canonical_session_display: String,
}

pub(super) fn run_resolve(arguments: ResolveArgs) -> Result<ExitCode> {
    let registry = SessionRegistry::open_default()?;
    let entry = registry.resolve(&arguments.name)?;
    if arguments.json {
        return write_json_line(&entry);
    }
    let mut path = entry.session_path().into_os_string().into_vec();
    path.push(b'\n');
    return write_stdout(path);
}

pub(super) fn run_list(arguments: ListArgs) -> Result<ExitCode> {
    let registry = SessionRegistry::open_default()?;
    let entries = registry.list(arguments.all)?;
    if arguments.json {
        return write_json_line(&entries);
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
    writeln!(
        output,
        "{:<alias_width$}  {:<lifecycle_width$}  {:<session_id_width$}  CANONICAL SESSION",
        "ALIAS", "LIFECYCLE", "SESSION ID",
    )
    .into_diagnostic()?;
    for (alias, lifecycle, session_id, path) in rows {
        writeln!(
            output,
            "{alias:<alias_width$}  {lifecycle:<lifecycle_width$}  {session_id:<session_id_width$}  {path}",
        )
        .into_diagnostic()?;
    }
    return write_stdout(output);
}

pub(super) fn run_remove(arguments: RemoveArgs) -> Result<ExitCode> {
    let registry = SessionRegistry::open_default()?;
    let entry = registry.remove(&arguments.name)?;
    let canonical_session_display = entry.session_path().display().to_string();
    return write_json_line(&serde_json::json!({
        "alias": entry.alias,
        "canonical_session": entry.canonical_session,
        "canonical_session_display": canonical_session_display,
        "session_id": entry.session_id,
        "evidence_preserved": true,
    }));
}

pub(super) fn run_gc(arguments: GcArgs) -> Result<ExitCode> {
    let registry = SessionRegistry::open_default()?;
    let report = registry.garbage_collect(
        arguments.older_than_days,
        arguments.keep_latest,
        arguments.apply,
    )?;
    return write_json_line(&report);
}
