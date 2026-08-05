use super::*;

/// Verify that an event log is the exact immutable log named by completion.
///
/// # Errors
///
/// Returns an error when completion is absent or disagrees with canonical data.
pub fn validate_completed_log(
    session_directory: &Path,
    log: &EventLog,
) -> Result<SessionCompletion> {
    if !session_directory.join("completion.json").is_file() {
        return Err(Error::SessionIncomplete {
            session: session_directory.to_path_buf(),
        });
    }
    let manifest_path = session_directory.join("session.json");
    let manifest_bytes = fs_err::read(&manifest_path).map_err(|source| {
        return Error::Filesystem {
            path: manifest_path.clone(),
            source,
        };
    })?;
    let manifest = read_manifest(session_directory)?;
    validate_manifest(&manifest)?;
    if canonical_manifest_bytes(&manifest)? != manifest_bytes {
        return Err(Error::InvalidReplay {
            reason: "session manifest bytes are not in the canonical serialization".to_owned(),
        });
    }
    let completion = read_completion(session_directory)?;
    validate_completion_format(&completion)?;
    let final_event_hash = log.events.last().map(|record| return record.hash.as_str());
    let event_count = u64::try_from(log.events.len()).unwrap_or(u64::MAX);
    let actual_manifest_hash = manifest_hash(&manifest_bytes);
    let header_bytes = serde_json::to_vec(&log.header).map_err(|source| {
        return Error::InvalidJson {
            path: session_directory.join("events.jsonl"),
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
        return Err(Error::InvalidReplay {
            reason: "session manifest identity or geometry does not match the event-log header"
                .to_owned(),
        });
    }
    if log.header.session_manifest_hash != actual_manifest_hash {
        return Err(Error::InvalidReplay {
            reason: format!(
                "event-log session manifest hash `{}` does not match actual hash `{actual_manifest_hash}`",
                log.header.session_manifest_hash
            ),
        });
    }
    if completion.session_id != log.header.session_id {
        return Err(Error::InvalidReplay {
            reason: "completion session identifier does not match the event log".to_owned(),
        });
    }
    if completion.session_manifest_hash != actual_manifest_hash {
        return Err(Error::InvalidReplay {
            reason: "completion session manifest hash does not match session.json".to_owned(),
        });
    }
    if completion.event_header_hash != event_header_hash {
        return Err(Error::InvalidReplay {
            reason: "completion event header hash does not match events.jsonl".to_owned(),
        });
    }
    if final_event_hash != Some(completion.final_event_hash.as_str()) {
        return Err(Error::InvalidReplay {
            reason: format!(
                "completion final hash `{}` does not match event-log hash `{}`",
                completion.final_event_hash,
                final_event_hash.unwrap_or("<missing>")
            ),
        });
    }
    if completion.event_count != event_count {
        return Err(Error::InvalidReplay {
            reason: format!(
                "completion event count {} does not match event-log count {event_count}",
                completion.event_count
            ),
        });
    }
    let Some(terminal) = log.events.last() else {
        return Err(Error::InvalidReplay {
            reason: "completed event log has no terminal Exit event".to_owned(),
        });
    };
    let Event::Exit {
        code, signal_name, ..
    } = &terminal.event
    else {
        return Err(Error::InvalidReplay {
            reason: "completed event log does not end with an Exit event".to_owned(),
        });
    };
    if completion.terminal_event_seq != terminal.seq {
        return Err(Error::InvalidReplay {
            reason: "completion terminal event sequence does not match the final Exit event"
                .to_owned(),
        });
    }
    if completion.exit_code != *code || completion.signal_name != *signal_name {
        return Err(Error::InvalidReplay {
            reason: "completion exit outcome does not match the final Exit event".to_owned(),
        });
    }
    if completion.durability.policy != manifest.durability_policy {
        return Err(Error::InvalidReplay {
            reason: "completion durability policy does not match the chain-bound session policy"
                .to_owned(),
        });
    }
    if !completion.durability.final_sync_completed || completion.durability.sync_count == 0 {
        return Err(Error::InvalidReplay {
            reason: "completion does not record a successful final event-log synchronization"
                .to_owned(),
        });
    }
    if !log.pending_operations.is_empty() {
        return Err(Error::InvalidReplay {
            reason: "completed event log contains pending mutation intents".to_owned(),
        });
    }
    let output_bytes = log.events.iter().try_fold(0_u64, |total, record| {
        let bytes = match &record.event {
            Event::Output { data } => u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX),
            _ => 0,
        };
        return total.checked_add(bytes).ok_or_else(|| {
            return Error::InvalidReplay {
                reason: "canonical output byte count overflow".to_owned(),
            };
        });
    })?;
    if completion.output_bytes != output_bytes {
        return Err(Error::InvalidReplay {
            reason: format!(
                "completion output byte count {} does not match event-log count {output_bytes}",
                completion.output_bytes
            ),
        });
    }
    match (
        &manifest.resource_limits.output_bytes,
        &completion.resource_limit,
    ) {
        (Some(limit), Some(outcome))
            if outcome.kind == ResourceLimitKind::OutputBytes
                && outcome.limit == *limit
                && outcome.observed > *limit
                && outcome.observed <= output_bytes =>
        {
            if !log.events.iter().any(|record| {
                return matches!(
                    &record.event,
                    Event::HostError { code, .. } if code == "output_budget_exceeded"
                );
            }) {
                return Err(Error::InvalidReplay {
                    reason: "resource-limit completion has no canonical output-budget trigger"
                        .to_owned(),
                });
            }
        }
        (Some(limit), None) if output_bytes <= *limit => {}
        (None, None) => {}
        _ => {
            return Err(Error::InvalidReplay {
                reason: "completion resource-limit outcome does not match manifest policy and canonical output"
                    .to_owned(),
            });
        }
    }
    return Ok(completion);
}

pub(super) fn validate_manifest(manifest: &SessionManifest) -> Result<()> {
    if manifest.format != SESSION_MANIFEST_FORMAT {
        return Err(Error::InvalidReplay {
            reason: format!("unsupported session format `{}`", manifest.format),
        });
    }
    if manifest.version != SESSION_MANIFEST_VERSION {
        return Err(Error::InvalidReplay {
            reason: format!("unsupported session format version {}", manifest.version),
        });
    }
    if manifest.request_journal != MUTATION_JOURNAL_NAME {
        return Err(Error::InvalidReplay {
            reason: "session manifest names an unsupported mutation journal path".to_owned(),
        });
    }
    if let Some(profile) = &manifest.ghostty_visual {
        if profile.replay_profile != GHOSTTY_REPLAY_PROFILE_NAME {
            return Err(Error::InvalidReplay {
                reason: format!(
                    "session manifest Ghostty replay profile must be the canonical root artifact `{GHOSTTY_REPLAY_PROFILE_NAME}`"
                ),
            });
        }
        if !is_lowercase_blake3_hash(&profile.replay_profile_hash) {
            return Err(Error::InvalidReplay {
                reason: "session manifest Ghostty replay profile hash must be exactly 64 lowercase hexadecimal characters"
                    .to_owned(),
            });
        }
        if !is_lowercase_blake3_hash(&profile.effective_config_hash) {
            return Err(Error::InvalidReplay {
                reason: "session manifest Ghostty effective configuration hash must be exactly 64 lowercase hexadecimal characters"
                    .to_owned(),
            });
        }
    }
    if manifest.geometry_provenance.source == GeometrySource::GhosttyFit {
        if manifest.geometry_provenance.capture_target.is_none() {
            return Err(Error::InvalidReplay {
                reason: "Ghostty-fitted geometry requires a canonical capture target".to_owned(),
            });
        }
        if manifest.ghostty_visual.is_none() {
            return Err(Error::InvalidReplay {
                reason: "Ghostty-fitted geometry requires recorded Ghostty profile provenance"
                    .to_owned(),
            });
        }
    }
    if let Some(tmux) = &manifest.tmux_pane {
        if tmux.version != TMUX_PANE_PROVENANCE_VERSION {
            return Err(Error::InvalidReplay {
                reason: format!("unsupported tmux pane provenance version {}", tmux.version),
            });
        }
        if tmux.tmux_version.is_empty()
            || tmux.socket_path.as_bytes().is_empty()
            || tmux.server_pid == 0
            || tmux.pane_pid == 0
            || tmux.pane_width == 0
            || tmux.pane_height == 0
            || tmux.pane_width != manifest.geometry.cols
            || tmux.pane_height != manifest.geometry.rows
            || !valid_tmux_id(&tmux.session_id, b'$')
            || !valid_tmux_id(&tmux.window_id, b'@')
            || !valid_tmux_id(&tmux.pane_id, b'%')
        {
            return Err(Error::InvalidReplay {
                reason: "session manifest contains invalid tmux pane provenance".to_owned(),
            });
        }
    }
    manifest.durability_policy.validate()?;
    if manifest.resource_limits.output_bytes == Some(0) {
        return Err(Error::InvalidReplay {
            reason: "session resource budgets must be greater than zero".to_owned(),
        });
    }
    manifest.child_environment.validate().map_err(|error| {
        return Error::InvalidReplay {
            reason: format!("invalid manifest child environment policy: {error}"),
        };
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

fn is_lowercase_blake3_hash(value: &str) -> bool {
    return value.len() == 64
        && value
            .bytes()
            .all(|byte| return byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
}

pub(super) fn validate_completion_format(completion: &SessionCompletion) -> Result<()> {
    if completion.format != SESSION_COMPLETION_FORMAT {
        return Err(Error::InvalidReplay {
            reason: format!("unsupported completion format `{}`", completion.format),
        });
    }
    if completion.version != SESSION_COMPLETION_VERSION {
        return Err(Error::InvalidReplay {
            reason: format!(
                "unsupported completion format version {}",
                completion.version
            ),
        });
    }
    if completion.validation_version != COMPLETION_VALIDATION_VERSION {
        return Err(Error::InvalidReplay {
            reason: format!(
                "unsupported completion validation version {}",
                completion.validation_version
            ),
        });
    }
    return Ok(());
}
