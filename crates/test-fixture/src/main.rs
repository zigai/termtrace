//! Deterministic canonical fixture generator for acceptance benchmarks.

use clap::{Args, Parser, Subcommand};
use miette::{IntoDiagnostic as _, Result};
use std::{
    env,
    ffi::OsString,
    fs::{self, DirBuilder, Permissions},
    os::unix::fs::{DirBuilderExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::ExitCode,
};
use termtrace_engine::recording::{control_socket_name, EffectiveEnvironment};
use termtrace_evidence::{
    artifact::write_secure,
    event_log::{DurabilityPolicy, EventLogWriter, SessionClock},
    provenance::{collect_execution_provenance, RecorderBuildIdentity},
    session::{canonical_manifest_bytes, manifest_hash, validate_completed_session},
};
use termtrace_model::{
    execution::environment::EnvironmentSpec,
    execution::event::{Event, EventLogHeader},
    execution::event_log::DurabilityMode,
    execution::provenance::ProvenancePolicy,
    execution::session::{
        GeometryProvenance, ResourceLimits, SessionCompletion, SessionGeometry, SessionManifest,
        COMPLETION_VALIDATION_VERSION, SESSION_COMPLETION_FORMAT, SESSION_COMPLETION_VERSION,
        SESSION_MANIFEST_FORMAT, SESSION_MANIFEST_VERSION,
    },
    terminal::replay::GHOSTTY_REVISION,
    value::ByteString,
};
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(name = "termtrace-fixture")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Generate one exact-size completed canonical session.
    Large(LargeArgs),
}

#[derive(Debug, Args)]
struct LargeArgs {
    /// Total canonical events, including the final exit event.
    #[arg(long)]
    events: u64,
    /// Exact aggregate payload bytes across output events.
    #[arg(long)]
    output_bytes: u64,
    /// New owner-only canonical session directory.
    #[arg(
        long,
        default_value = "benchmarks/fixtures/million-event-1gib.termtrace"
    )]
    out: PathBuf,
    /// Canonical fixture columns.
    #[arg(long, default_value_t = 120)]
    cols: u16,
    /// Canonical fixture rows.
    #[arg(long, default_value_t = 40)]
    rows: u16,
}

fn main() -> Result<ExitCode> {
    let cli = Cli::parse();
    match cli.command {
        Command::Large(arguments) => generate(arguments)?,
    }
    return Ok(ExitCode::SUCCESS);
}

fn generate(arguments: LargeArgs) -> Result<()> {
    if arguments.events < 2 {
        return Err(miette::miette!(
            "--events must include at least one output event and the terminal exit"
        ));
    }

    if arguments.output_bytes < arguments.events - 1 {
        return Err(miette::miette!(
            "--output-bytes must provide at least one byte per output event"
        ));
    }

    if arguments.output_bytes.div_ceil(arguments.events - 1) > 32 * 1024 * 1024 {
        return Err(miette::miette!(
            "fixture output chunks must not exceed 32 MiB; increase --events"
        ));
    }

    if arguments.cols == 0 || arguments.rows == 0 {
        return Err(miette::miette!("fixture geometry must be nonzero"));
    }

    create_session_directory(&arguments.out)?;

    let clock = SessionClock::now().into_diagnostic()?;
    let session_id = Uuid::now_v7();
    let durability = DurabilityPolicy {
        mode: DurabilityMode::Throughput,
        maximum_interval_ms: None,
        maximum_uncommitted_bytes: None,
    };

    let working_directory = env::current_dir().into_diagnostic()?;
    let command = [OsString::from("termtrace-fixture-generated")];
    let environment = EffectiveEnvironment::resolve(&EnvironmentSpec::default(), "xterm-ghostty")
        .into_diagnostic()?;

    let execution_provenance = collect_execution_provenance(
        &ProvenancePolicy::default(),
        environment.variables(),
        &[OsString::from("/bin/true")],
        &working_directory,
        RecorderBuildIdentity {
            version: env!("CARGO_PKG_VERSION"),
            source_revision: env!("TERMTRACE_BUILD_REVISION"),
            semantic_engine_revision: GHOSTTY_REVISION,
        },
    );

    let geometry = SessionGeometry {
        cols: arguments.cols,
        rows: arguments.rows,
        pixel_width: arguments.cols.saturating_mul(10),
        pixel_height: arguments.rows.saturating_mul(20),
    };
    let manifest = SessionManifest {
        failure_policy: Default::default(),
        format: SESSION_MANIFEST_FORMAT.to_owned(),
        version: SESSION_MANIFEST_VERSION,
        session_id,
        creation_alias: None,
        lineage: None,
        started_unix_ns: clock.started_unix_ns(),
        command: command
            .iter()
            .map(|argument| return ByteString::from(argument.as_encoded_bytes()))
            .collect(),
        working_directory: Some(ByteString::from(
            working_directory.as_os_str().as_encoded_bytes(),
        )),
        geometry: geometry.clone(),
        geometry_provenance: GeometryProvenance::default(),
        ghostty_visual: None,
        child_environment: environment.policy().clone(),
        execution_provenance,
        import_provenance: None,
        tmux_pane: None,
        termtrace_version: env!("CARGO_PKG_VERSION").to_owned(),
        operating_system: env::consts::OS.to_owned(),
        architecture: env::consts::ARCH.to_owned(),
        event_log: "events.jsonl".to_owned(),
        control_socket: control_socket_name(session_id),
        request_journal: "requests.jsonl".to_owned(),
        contains_sensitive_terminal_data: true,
        durability_policy: durability.clone(),
        resource_limits: ResourceLimits::default(),
    };
    let manifest_bytes = canonical_manifest_bytes(&manifest).into_diagnostic()?;

    write_secure(&arguments.out.join("session.json"), &manifest_bytes).into_diagnostic()?;

    let session_manifest_hash = manifest_hash(&manifest_bytes);
    let header = EventLogHeader::new(
        session_id,
        &session_manifest_hash,
        clock.started_unix_ns(),
        geometry.cols,
        geometry.rows,
        geometry.pixel_width,
        geometry.pixel_height,
        "xterm-ghostty",
    );
    let header_bytes = serde_json::to_vec(&header).into_diagnostic()?;

    let mut writer = EventLogWriter::create_with_policy(
        arguments.out.join("events.jsonl"),
        &header,
        clock,
        durability,
    )
    .into_diagnostic()?;

    write_output_events(&mut writer, arguments.events - 1, arguments.output_bytes)
        .into_diagnostic()?;
    let terminal_event_seq = writer
        .append(Event::Exit {
            code: Some(0),
            signal: None,
            signal_name: None,
        })
        .into_diagnostic()?;

    let completion = SessionCompletion {
        format: SESSION_COMPLETION_FORMAT.to_owned(),
        version: SESSION_COMPLETION_VERSION,
        validation_version: COMPLETION_VALIDATION_VERSION,
        session_id,
        session_manifest_hash,
        event_header_hash: blake3::hash(&header_bytes).to_hex().to_string(),
        exit_code: Some(0),
        signal_name: None,
        output_bytes: arguments.output_bytes,
        final_event_hash: writer.current_hash(),
        event_count: writer.next_seq(),
        terminal_event_seq,
        durability: writer.durability_record(),
        resource_limit: None,
        failure: None,
    };

    drop(writer);

    write_secure(
        &arguments.out.join("completion.json"),
        &serde_json::to_vec(&completion).into_diagnostic()?,
    )
    .into_diagnostic()?;

    validate_completed_session(&arguments.out).into_diagnostic()?;
    println!(
        "{} events, {} output bytes: {}",
        completion.event_count,
        completion.output_bytes,
        arguments.out.display()
    );
    return Ok(());
}

fn write_output_events(
    writer: &mut EventLogWriter,
    output_events: u64,
    output_bytes: u64,
) -> termtrace_evidence::Result<()> {
    let base = output_bytes / output_events;
    let larger_events = output_bytes % output_events;

    for index in 0..output_events {
        let length = base + u64::from(index < larger_events);
        let length = usize::try_from(length).map_err(|error| {
            return termtrace_evidence::Error::InvalidEvidence {
                reason: format!("fixture output chunk does not fit in memory: {error}"),
            };
        })?;

        let mut payload = vec![b'x'; length];
        if let Some(last) = payload.last_mut() {
            *last = b'\n';
        }

        writer.append_output(Event::Output {
            data: ByteString::new(payload),
        })?;
    }

    return Ok(());
}

fn create_session_directory(path: &Path) -> Result<()> {
    DirBuilder::new()
        .mode(0o700)
        .create(path)
        .into_diagnostic()?;

    fs::set_permissions(path, Permissions::from_mode(0o700)).into_diagnostic()?;
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_fixture_has_exact_counts_and_valid_completion() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let session = temporary.path().join("small.termtrace");

        generate(LargeArgs {
            events: 128,
            output_bytes: 65_536,
            out: session.clone(),
            cols: 80,
            rows: 24,
        })
        .expect("generate fixture");

        let completion = validate_completed_session(&session).expect("validate fixture");

        assert_eq!(completion.event_count, 128);
        assert_eq!(completion.output_bytes, 65_536);
        assert_eq!(completion.terminal_event_seq, 127);
    }
}
