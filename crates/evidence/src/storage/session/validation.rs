use super::io::{canonical_manifest_bytes, manifest_hash, read_completion, read_manifest};
use crate::storage::{
    artifact::hash_regular_file, event_log::EventLogSummary, is_lowercase_blake3,
    CompletionIntegrityError, CompletionIntegrityKind, Error, Result,
};
use std::{fs, path::Path};
use termtrace_model::{
    execution::event::Event,
    execution::layout::{self, SessionLayout},
    execution::session::{
        GeometrySource, ResourceLimitKind, SessionCompletion, SessionManifest,
        COMPLETION_VALIDATION_VERSION, SESSION_COMPLETION_FORMAT, SESSION_COMPLETION_VERSION,
        SESSION_MANIFEST_FORMAT, TMUX_PANE_PROVENANCE_VERSION,
    },
    execution::visual_profile::NativeVisualProfile,
    protocol::control::control_socket_name,
};

pub(super) fn validate_replay_profile_file(
    session_directory: &Path,
    manifest: &SessionManifest,
) -> Result<()> {
    let Some(profile) = &manifest.ghostty_visual else {
        return Ok(());
    };

    let profile_path = SessionLayout::new(session_directory).ghostty_replay_profile();
    let (_, actual_hash) = hash_regular_file(&profile_path)?;
    if actual_hash != profile.replay_profile_hash {
        return Err(Error::InvalidEvidence {
            reason: format!(
                "recorded Ghostty replay profile `{}` hash does not match session provenance",
                profile_path.display()
            ),
            source: None,
        });
    }
    return Ok(());
}

/// Stream and validate a completed canonical session without retaining its output.
///
/// # Errors
///
/// Returns an error for any malformed source or inconsistent completion.
pub fn validate_completed_session(session_directory: &Path) -> Result<SessionCompletion> {
    return validate_completed_session_with_check(session_directory, || return Ok(()));
}

/// Validate a completed canonical session, running `check` before every streamed record.
///
/// # Errors
///
/// Returns any check failure, malformed source, or inconsistent completion.
pub fn validate_completed_session_with_check(
    session_directory: &Path,
    check: impl FnMut() -> Result<()>,
) -> Result<SessionCompletion> {
    let layout = SessionLayout::new(session_directory);
    if !layout.completion().is_file() {
        return Err(Error::SessionIncomplete {
            session: session_directory.to_path_buf(),
        });
    }

    let completion = read_completion(session_directory)?;
    let summary = EventLogSummary::read_with_check(layout.event_log(), check)?;
    validate_completion_summary_candidate(session_directory, &summary, &completion)?;

    return Ok(completion);
}

/// Validate a completion candidate against a streamed, fully validated log summary.
///
/// # Errors
///
/// Returns an error when any canonical identity, count, hash, or format contract differs.
pub fn validate_completion_summary_candidate(
    session_directory: &Path,
    log: &EventLogSummary,
    completion: &SessionCompletion,
) -> Result<()> {
    let manifest_path = SessionLayout::new(session_directory).manifest();
    let manifest_bytes = fs::read(&manifest_path).map_err(|source| {
        return Error::Filesystem {
            path: manifest_path.clone(),
            source,
        };
    })?;

    let manifest = read_manifest(session_directory)?;
    validate_manifest(&manifest)?;

    validate_completion_format(completion)?;

    return validate_completion_with_loaded(
        session_directory,
        log,
        &manifest,
        &manifest_bytes,
        completion,
    )
    .map_err(|failure| return failure.error);
}

pub(super) fn validate_completion_with_loaded(
    session_directory: &Path,
    log: &EventLogSummary,
    manifest: &SessionManifest,
    manifest_bytes: &[u8],
    completion: &SessionCompletion,
) -> Result<(), CompletionIntegrityError> {
    if canonical_manifest_bytes(manifest)? != manifest_bytes {
        return Err(Error::invalid(
            "session manifest bytes are not in the canonical serialization",
        )
        .into());
    }

    validate_replay_profile_file(session_directory, manifest)?;

    let final_event_hash = log
        .terminal
        .as_ref()
        .map(|record| return record.hash.as_str());

    let event_count = log.event_count;
    let actual_manifest_hash = manifest_hash(manifest_bytes);
    let header_bytes = serde_json::to_vec(&log.header).map_err(|source| {
        return Error::InvalidJson {
            path: SessionLayout::new(session_directory).event_log(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    let event_header_hash = blake3::hash(&header_bytes).to_hex().to_string();

    if manifest.session_id != log.header.session_id
        || manifest.started_unix_ns != log.header.started_unix_ns
        || manifest.geometry.cols != log.header.cols
        || manifest.geometry.rows != log.header.rows
        || manifest.geometry.pixel_width != log.header.pixel_width
        || manifest.geometry.pixel_height != log.header.pixel_height
    {
        return Err(Error::invalid(
            "session manifest identity or geometry does not match the event-log header",
        )
        .into());
    }

    if log.header.session_manifest_hash != actual_manifest_hash {
        return Err(CompletionIntegrityError::new(
            CompletionIntegrityKind::SessionManifestHash,
            format!(
                "event-log session manifest hash `{}` does not match actual hash `{actual_manifest_hash}`",
                log.header.session_manifest_hash
            ),
        ));
    }

    if completion.session_id != log.header.session_id {
        return Err(
            Error::invalid("completion session identifier does not match the event log").into(),
        );
    }

    if completion.session_manifest_hash != actual_manifest_hash {
        return Err(CompletionIntegrityError::new(
            CompletionIntegrityKind::SessionManifestHash,
            "completion session manifest hash does not match session.json",
        ));
    }

    if completion.event_header_hash != event_header_hash {
        return Err(CompletionIntegrityError::new(
            CompletionIntegrityKind::EventHeaderHash,
            "completion event header hash does not match events.jsonl",
        ));
    }

    if final_event_hash != Some(completion.final_event_hash.as_str()) {
        return Err(CompletionIntegrityError::new(
            CompletionIntegrityKind::FinalEventHash,
            format!(
                "completion final hash `{}` does not match event-log hash `{}`",
                completion.final_event_hash,
                final_event_hash.unwrap_or("<missing>")
            ),
        ));
    }

    if completion.event_count != event_count {
        return Err(CompletionIntegrityError::new(
            CompletionIntegrityKind::EventCount,
            format!(
                "completion event count {} does not match event-log count {event_count}",
                completion.event_count
            ),
        ));
    }

    let Some(terminal) = log.terminal.as_ref() else {
        return Err(CompletionIntegrityError::new(
            CompletionIntegrityKind::TerminalExitMissing,
            "completed event log has no terminal Exit event",
        ));
    };
    let Event::Exit {
        code, signal_name, ..
    } = &terminal.event
    else {
        return Err(CompletionIntegrityError::new(
            CompletionIntegrityKind::TerminalExitMissing,
            "completed event log does not end with an Exit event",
        ));
    };

    if completion.terminal_event_seq != terminal.seq {
        return Err(Error::invalid(
            "completion terminal event sequence does not match the final Exit event",
        )
        .into());
    }

    if completion.exit_code != *code || completion.signal_name != *signal_name {
        return Err(CompletionIntegrityError::new(
            CompletionIntegrityKind::ExitOutcome,
            "completion exit outcome does not match the final Exit event",
        ));
    }

    if completion.durability.policy != manifest.durability_policy {
        return Err(CompletionIntegrityError::new(
            CompletionIntegrityKind::DurabilityRecord,
            "completion durability policy does not match the chain-bound session policy",
        ));
    }

    if !completion.durability.final_sync_completed || completion.durability.sync_count == 0 {
        return Err(CompletionIntegrityError::new(
            CompletionIntegrityKind::DurabilityRecord,
            "completion does not record a successful final event-log synchronization",
        ));
    }

    if !log.pending_operations.is_empty() {
        return Err(CompletionIntegrityError::new(
            CompletionIntegrityKind::PendingMutationIntents,
            "completed event log contains pending mutation intents",
        ));
    }

    let output_bytes = log.output_bytes;
    if completion.output_bytes != output_bytes {
        return Err(CompletionIntegrityError::new(
            CompletionIntegrityKind::OutputCount,
            format!(
                "completion output byte count {} does not match event-log count {output_bytes}",
                completion.output_bytes
            ),
        ));
    }

    if let Some(limit) = manifest.resource_limits.output_bytes
        && output_bytes > limit
        && completion.resource_limit.is_none()
    {
        return Err(Error::invalid(
            "output bytes exceeded manifest limit without completion outcome",
        )
        .into());
    }

    if let Some(outcome) = &completion.resource_limit {
        let [output_trigger, duration_trigger, idle_trigger] = log.resource_triggers;
        let (configured_limit, triggered, policy_label, trigger_label) = match outcome.kind {
            ResourceLimitKind::OutputBytes => (
                manifest.resource_limits.output_bytes,
                output_trigger,
                "canonical output",
                "output-budget",
            ),
            ResourceLimitKind::MaxDuration => (
                manifest.resource_limits.max_duration_ms,
                duration_trigger,
                "duration",
                "max-duration",
            ),
            ResourceLimitKind::IdleTimeout => (
                manifest.resource_limits.idle_timeout_ms,
                idle_trigger,
                "idle duration",
                "idle-timeout",
            ),
        };
        let Some(limit) = configured_limit else {
            return Err(Error::invalid(
                "completion resource-limit outcome does not match manifest policy",
            )
            .into());
        };
        let observed_valid = match outcome.kind {
            ResourceLimitKind::OutputBytes => {
                outcome.observed > limit && outcome.observed <= output_bytes
            }
            ResourceLimitKind::MaxDuration | ResourceLimitKind::IdleTimeout => {
                outcome.observed >= limit
            }
        };

        if outcome.limit != limit || !observed_valid {
            return Err(Error::invalid(format!(
                "completion resource-limit outcome does not match manifest policy and {policy_label}"
            )).into());
        }

        if !triggered {
            return Err(Error::invalid(format!(
                "resource-limit completion has no canonical {trigger_label} trigger"
            ))
            .into());
        }
    }

    return Ok(());
}

pub(super) fn validate_manifest(manifest: &SessionManifest) -> Result<()> {
    if manifest.format != SESSION_MANIFEST_FORMAT {
        return Err(Error::invalid(format!(
            "unsupported session format `{}`",
            manifest.format
        )));
    }

    if manifest.version != manifest.expected_version() {
        return Err(Error::invalid(format!(
            "unsupported session format version {}",
            manifest.version
        )));
    }

    if let Some(recorded) = &manifest.visual_profile {
        recorded
            .validate()
            .map_err(|error| return Error::invalid(error.to_string()))?;
        let expected = recorded
            .profile
            .native
            .as_ref()
            .map(NativeVisualProfile::overrides);

        if expected.as_ref()
            != manifest
                .ghostty_visual
                .as_ref()
                .map(|provenance| return &provenance.overrides)
        {
            return Err(Error::invalid(
                "session visual profile disagrees with its recorded Ghostty provenance",
            ));
        }
    }

    if manifest.control_socket != control_socket_name(manifest.session_id) {
        return Err(Error::invalid(
            "session manifest control endpoint does not match its session ID",
        ));
    }

    if manifest.event_log != layout::EVENT_LOG {
        return Err(Error::invalid(
            "session manifest names an unsupported event log path",
        ));
    }

    if manifest.request_journal != layout::MUTATION_JOURNAL {
        return Err(Error::invalid(
            "session manifest names an unsupported mutation journal path",
        ));
    }

    if let Some(profile) = &manifest.ghostty_visual {
        if profile.replay_profile != layout::GHOSTTY_REPLAY_PROFILE {
            return Err(Error::invalid(format!(
                "session manifest Ghostty replay profile must be the canonical root artifact `{}`",
                layout::GHOSTTY_REPLAY_PROFILE
            )));
        }

        if !is_lowercase_blake3(&profile.replay_profile_hash) {
            return Err(Error::invalid(
                "session manifest Ghostty replay profile hash must be exactly 64 lowercase hexadecimal characters",
            ));
        }

        if !is_lowercase_blake3(&profile.effective_config_hash) {
            return Err(Error::invalid(
                "session manifest Ghostty effective configuration hash must be exactly 64 lowercase hexadecimal characters",
            ));
        }
    }

    if manifest.geometry_provenance.source == GeometrySource::GhosttyFit {
        if manifest.geometry_provenance.capture_target.is_none() {
            return Err(Error::invalid(
                "Ghostty-fitted geometry requires a canonical capture target",
            ));
        }

        if manifest.ghostty_visual.is_none() {
            return Err(Error::invalid(
                "Ghostty-fitted geometry requires recorded Ghostty profile provenance",
            ));
        }
    }

    if let Some(tmux) = &manifest.tmux_pane
        && (tmux.version != TMUX_PANE_PROVENANCE_VERSION
            || tmux.tmux_version.is_empty()
            || tmux.socket_path.as_bytes().is_empty()
            || tmux.server_pid == 0
            || tmux.pane_pid == 0
            || tmux.pane_width == 0
            || tmux.pane_height == 0
            || tmux.pane_width != manifest.geometry.cols
            || tmux.pane_height != manifest.geometry.rows
            || !valid_tmux_id(&tmux.session_id, b'$')
            || !valid_tmux_id(&tmux.window_id, b'@')
            || !valid_tmux_id(&tmux.pane_id, b'%'))
    {
        return Err(Error::invalid(
            "session manifest contains invalid tmux pane provenance",
        ));
    }

    manifest.durability_policy.validate()?;

    if manifest.resource_limits.output_bytes == Some(0) {
        return Err(Error::invalid(
            "session resource budgets must be greater than zero",
        ));
    }

    manifest.child_environment.validate().map_err(|error| {
        return Error::invalid(format!(
            "invalid manifest child environment policy: {error}"
        ));
    })?;

    return Ok(());
}

fn valid_tmux_id(value: &str, prefix: u8) -> bool {
    let bytes = value.as_bytes();

    return bytes.first() == Some(&prefix)
        && bytes.len() > 1
        && bytes
            .get(1..)
            .is_some_and(|suffix| return suffix.iter().all(u8::is_ascii_digit));
}

pub(super) fn validate_completion_format(completion: &SessionCompletion) -> Result<()> {
    if completion.format != SESSION_COMPLETION_FORMAT {
        return Err(Error::invalid(format!(
            "unsupported completion format `{}`",
            completion.format
        )));
    }

    if completion.version != SESSION_COMPLETION_VERSION {
        return Err(Error::invalid(format!(
            "unsupported completion format version {}",
            completion.version
        )));
    }

    if completion.validation_version != COMPLETION_VALIDATION_VERSION {
        return Err(Error::invalid(format!(
            "unsupported completion validation version {}",
            completion.validation_version
        )));
    }
    return Ok(());
}
