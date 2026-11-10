//! `termtrace tmux`: argument parsing and rendering over `termtrace-tmux`.

use crate::cli::{
    artifacts,
    output::{optional_display, write_json_line, write_json_pretty, write_stdout, ResponseWriter},
};
use miette::{IntoDiagnostic as _, Result};
use std::{
    ffi::OsString, fmt::Write as _, os::unix::ffi::OsStringExt as _, path::PathBuf,
    process::ExitCode, time::Duration,
};
use termtrace_engine::registry::SessionRegistry;
use termtrace_model::value::ByteString;
use termtrace_tmux::recording::Tracks as _;
use termtrace_tmux::{
    artifact::{TmuxRecordingCoverage, TmuxRecordingManifest},
    client::TmuxClient,
    observer,
    pane::exec_pane_launch,
    recording::{self, CatalogPage, RecordRequest, StopRequest},
    scope::{ScopeTargets, TmuxScopePolicy},
    setup::{self, Disabled, PreparedTmuxSetup},
    status::{self, TmuxStatusReport},
    store::RecordingStore,
};

mod adapter;
mod args;
mod pane_recorder;

pub(super) use args::{TmuxArgs, TmuxSubcommand};

pub(super) fn dispatch(response: &mut ResponseWriter, command: TmuxSubcommand) -> Result<ExitCode> {
    return match command {
        TmuxSubcommand::Record(arguments) => {
            let tracks = adapter::open(arguments.setup.output_root.as_deref())?;
            let (store, manifest) = recording::record(
                &tracks,
                RecordRequest {
                    name: arguments.name,
                    new_server: arguments.new,
                    client: arguments.connection.client()?,
                    setup: arguments.setup.request(),
                    command: arguments.command,
                },
            )
            .into_diagnostic()?;

            write_recording(response, "tmux_recording", &store, &manifest)
        }
        TmuxSubcommand::Status(arguments) => match arguments.recording.as_deref() {
            Some(name) => {
                let tracks = adapter::open(arguments.output_root.as_deref())?;
                let manifest = recording::status(tracks.state_root(), name).into_diagnostic()?;

                write_json_line(response, &manifest)
            }
            None => {
                let report = status::observe(&arguments.connection.client()?).into_diagnostic()?;
                if arguments.json {
                    write_json_pretty(response, &report)
                } else {
                    write_stdout(response, status_text(report)?)
                }
            }
        },
        TmuxSubcommand::Events(arguments) => {
            let tracks = adapter::open(arguments.output_root.as_deref())?;
            let (manifest, page) = recording::events(
                tracks.state_root(),
                &arguments.name,
                arguments.from_sequence,
                arguments.limit,
            )
            .into_diagnostic()?;

            write_json_line(
                response,
                &serde_json::json!({
                    "kind": "tmux_events",
                    "recording_id": manifest.recording_id,
                    "events": page.events,
                    "next_sequence": page.next_sequence,
                }),
            )
        }
        TmuxSubcommand::Tracks(arguments) => run_catalog(response, arguments, false),
        TmuxSubcommand::Gaps(arguments) => run_catalog(response, arguments, true),
        TmuxSubcommand::Stop(arguments) => {
            let tracks = adapter::open(arguments.output_root.as_deref())?;
            let manifest = recording::stop(
                &tracks,
                StopRequest {
                    name: &arguments.name,
                    wait: arguments.wait,
                    timeout: Duration::from_millis(arguments.timeout_ms),
                },
            )
            .into_diagnostic()?;

            let store =
                RecordingStore::locate(tracks.state_root(), &arguments.name).into_diagnostic()?;

            write_recording(response, "tmux_recording_stopped", &store, &manifest)
        }
        TmuxSubcommand::Video(arguments) => {
            let tracks = adapter::open(arguments.output_root.as_deref())?;

            artifacts::render_video(response, &adapter::video_request(&tracks, arguments)?)
        }
        TmuxSubcommand::Bundle(arguments) => {
            let tracks = adapter::open(arguments.output_root.as_deref())?;
            let manifest = adapter::bundle(&tracks, &arguments)?;

            write_json_line(
                response,
                &serde_json::json!({
                    "kind": "tmux_bundle",
                    "directory": arguments.out,
                    "recording_id": manifest.recording_id,
                    "recording_name": manifest.recording_name,
                    "track_count": manifest.track_count,
                    "manifest": manifest,
                }),
            )
        }
        TmuxSubcommand::Config(arguments) => {
            let tracks = adapter::open(arguments.setup.output_root.as_deref())?;
            let setup = PreparedTmuxSetup::prepare(
                arguments.setup.request(),
                arguments.tmux,
                tracks.state_root(),
            )
            .into_diagnostic()?;

            write_stdout(response, setup.config_fragment().into_diagnostic()?)
        }
        TmuxSubcommand::Enable(arguments) => {
            let tracks = adapter::open(arguments.setup.output_root.as_deref())?;
            let client = arguments.connection.client()?;
            let mut setup = PreparedTmuxSetup::prepare(
                arguments.setup.request(),
                arguments.connection.tmux,
                tracks.state_root(),
            )
            .into_diagnostic()?;
            setup.scope = setup.scope.resolve_live(&client).into_diagnostic()?;
            let enabled = setup.enable(&client).into_diagnostic()?;
            let report = status::observe(&client).into_diagnostic()?;

            write_json_line(
                response,
                &serde_json::json!({
                    "enabled": true,
                    "already_enabled": enabled.already_enabled,
                    "integration_version": 1,
                    "coverage": setup.coverage,
                    "strict": setup.coverage == TmuxRecordingCoverage::Strict,
                    "output_root": ByteString::from(setup.output_root.into_os_string().into_vec()),
                    "scope": setup.scope,
                    "existing_panes": report.pane_count,
                    "existing_unrecorded_panes": report.unrecorded_panes,
                    "retroactive": false,
                }),
            )
        }
        TmuxSubcommand::Disable(connection) => {
            let disabled = setup::disable(&connection.client()?).into_diagnostic()?;

            write_json_line(
                response,
                &match disabled {
                    Disabled::AlreadyDisabled => serde_json::json!({
                        "enabled": false,
                        "already_disabled": true,
                    }),
                    Disabled::Restored => serde_json::json!({
                        "enabled": false,
                        "already_disabled": false,
                        "default_command_restored": true,
                    }),
                },
            )
        }
        TmuxSubcommand::Scope(arguments) => {
            let client = arguments.connection.client()?;
            if !client.is_integration_enabled() {
                return Err(miette::miette!(
                    "tmux recording is not enabled for this server"
                ));
            }

            let requested = TmuxScopePolicy::requested(
                &client,
                arguments.all,
                arguments.explicit_only,
                &ScopeTargets {
                    sessions: arguments.sessions,
                    windows: arguments.windows,
                    panes: arguments.panes,
                },
            )
            .into_diagnostic()?;

            let changed = requested.is_some();
            let policy = match requested {
                Some(policy) => {
                    setup::grandfather_current_panes(&client).into_diagnostic()?;
                    policy.store(&client).into_diagnostic()?;

                    policy
                }
                None => TmuxScopePolicy::from_options(&client).into_diagnostic()?,
            };

            write_json_line(
                response,
                &serde_json::json!({
                    "changed": changed,
                    "future_processes_only": changed,
                    "scope": policy,
                }),
            )
        }
        TmuxSubcommand::Run(arguments) => pane_recorder::run(arguments.into()),
        TmuxSubcommand::ExecPane(arguments) => {
            Err(exec_pane_launch(&arguments.command)).into_diagnostic()
        }
        TmuxSubcommand::Guard(arguments) => {
            observer::guard(&TmuxClient::inherited(), &arguments.pane).into_diagnostic()?;

            Ok(ExitCode::SUCCESS)
        }
        TmuxSubcommand::Supervise => {
            observer::supervise(
                &TmuxClient::inherited(),
                &mut |state_root, retention, linked| {
                    let report = SessionRegistry::open(state_root)?.retain_tmux_panes(
                        retention.older_than_days,
                        retention.max_bytes,
                        linked,
                    )?;

                    tracing::debug!(
                        removed = report.removed_sessions,
                        reclaimed_bytes = report.reclaimed_bytes,
                        "applied tmux pane retention"
                    );

                    return Ok(());
                },
            )
            .into_diagnostic()?;

            Ok(ExitCode::SUCCESS)
        }
        TmuxSubcommand::Watchdog => {
            observer::watchdog(&TmuxClient::inherited()).into_diagnostic()?;

            Ok(ExitCode::SUCCESS)
        }
    };
}

fn run_catalog(
    response: &mut ResponseWriter,
    arguments: args::TmuxCatalogArgs,
    gaps: bool,
) -> Result<ExitCode> {
    let tracks = adapter::open(arguments.page.output_root.as_deref())?;
    let limit = arguments.page.limit;
    let (manifest, summary, page) = recording::catalog_page(
        tracks.state_root(),
        &arguments.page.name,
        gaps,
        arguments.page.from_sequence,
        limit,
        arguments.active_only,
    )
    .into_diagnostic()?;

    let (entries, last) = match page {
        CatalogPage::Tracks(entries) => (
            serde_json::to_value(&entries).into_diagnostic()?,
            entries.last().map(|entry| return entry.sequence),
        ),
        CatalogPage::Gaps(entries) => (
            serde_json::to_value(&entries).into_diagnostic()?,
            entries.last().map(|entry| return entry.sequence),
        ),
    };

    let full = entries
        .as_array()
        .is_some_and(|entries| return entries.len() == usize::from(limit));

    let next_sequence = last
        .filter(|_| return full)
        .and_then(|sequence| return sequence.checked_add(1));

    return write_json_line(
        response,
        &serde_json::json!({
            "kind": if gaps { "tmux_coverage_gaps" } else { "tmux_track_inventory" },
            "recording_id": manifest.recording_id,
            "boundary": manifest.lifecycle_journal,
            "summary": summary,
            "entries": entries,
            "next_sequence": next_sequence,
        }),
    );
}

fn write_recording(
    response: &mut ResponseWriter,
    kind: &str,
    store: &RecordingStore,
    manifest: &TmuxRecordingManifest,
) -> Result<ExitCode> {
    return write_json_line(
        response,
        &serde_json::json!({
            "kind": kind,
            "recording_id": manifest.recording_id,
            "name": manifest.name,
            "lifecycle": manifest.state.lifecycle,
            "manifest": store.manifest_path(),
            "composite_track": manifest.state.composite_track,
            "pane_tracks": manifest.state.pane_tracks,
            "coverage": manifest.coverage,
            "coverage_gaps": manifest.state.coverage_gaps,
        }),
    );
}

fn status_text(report: TmuxStatusReport) -> Result<String> {
    let scope = report
        .scope
        .as_ref()
        .map_or("disabled", TmuxScopePolicy::mode_name);

    let mut output = format!(
        "enabled={} strict={} scope={} default-command={} panes={} selected={} excluded={} recorded={} unrecorded={}\n",
        report.enabled,
        report.strict,
        scope,
        if report.default_command_matches {
            "managed"
        } else {
            "unmanaged"
        },
        report.pane_count,
        report.selected_panes,
        report.excluded_panes,
        report.recorded_panes,
        report.unrecorded_panes,
    );

    for pane in report.panes {
        let recording = pane.recording_session.map_or_else(
            || return "-".to_owned(),
            |path| {
                return PathBuf::from(OsString::from_vec(path.into_bytes()))
                    .display()
                    .to_string();
            },
        );
        writeln!(
            output,
            "{} {} {} pid={} selected={} recorded={} {}",
            pane.session_id,
            pane.window_id,
            pane.pane_id,
            optional_display(pane.pane_pid),
            pane.selected,
            pane.recorded,
            recording,
        )
        .into_diagnostic()?;
    }

    return Ok(output);
}
