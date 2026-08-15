use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs::Permissions,
    io::Read as _,
    iter,
    os::unix::{
        fs::PermissionsExt as _,
        net::{SocketAddr, UnixStream},
    },
    path::PathBuf,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use tempfile::TempDir;

use super::super::{
    control::{MutationHistory, spawn_control_server},
    input::encode_paste,
};
use super::*;
use termtrace_evidence::{
    Error as EvidenceError,
    event_log::{CanonicalBoundary, DurabilityPolicy, EventLog},
    request_journal::{MUTATION_JOURNAL_NAME, guarded_command_hash},
    session::{read_completion, validate_completed_log},
};
use termtrace_model::{
    execution::environment::{EnvironmentBase, EnvironmentSpec},
    execution::event::{Event, InputSource},
    execution::provenance::ProvenancePolicy,
    execution::session::{
        GeometryProvenance, ResourceLimitKind, ResourceLimits, SessionCompletion, SessionManifest,
    },
    protocol::control::{
        CONTROL_PROTOCOL_VERSION, ControlCommand, ControlData, ControlRequest, ControlResponse,
        ControlResult,
    },
    terminal::observation::ActorBoundaries,
};
use termtrace_terminal::PresentationAction;

fn control_socket_address(name: &str) -> SocketAddr {
    use std::os::linux::net::SocketAddrExt as _;

    return SocketAddr::from_abstract_name(name.as_bytes()).expect("control socket address");
}

fn cached_test_response(request_id: Uuid, next_event_seq: u64) -> ControlResponse {
    let boundary = CanonicalBoundary {
        next_event_seq,
        event_hash: format!("{next_event_seq:064x}"),
        output_bytes: 0,
    };
    return ControlResponse {
        version: CONTROL_PROTOCOL_VERSION,
        request_id,
        result: ControlResult::Ok {
            data: ControlData::Acknowledged {
                boundaries: ActorBoundaries {
                    durable: boundary.clone(),
                    observed: boundary,
                },
            },
        },
    };
}

fn isolated_options(output_directory: PathBuf, command: Vec<OsString>) -> RecordOptions {
    return RecordOptions {
        output_directory,
        creation_alias: None,
        lineage: None,
        import_provenance: None,
        tmux_pane: None,
        command,
        working_directory: None,
        cols: 80,
        rows: 24,
        cell_width_px: 9,
        cell_height_px: 18,
        geometry_provenance: GeometryProvenance::default(),
        ghostty_profile: None,
        term: "xterm-ghostty".to_owned(),
        environment: EnvironmentSpec::default(),
        colors: TerminalColors::default(),
        forward_stdin: false,
        continue_after_output_mirror_closes: false,
        durability_policy: DurabilityPolicy::default(),
        provenance_policy: ProvenancePolicy::default(),
        resource_limits: ResourceLimits::default(),
    };
}

struct FailingOutput;

impl Write for FailingOutput {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        return Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            format!("rejected {} output bytes", buf.len()),
        ));
    }

    fn flush(&mut self) -> io::Result<()> {
        return Ok(());
    }
}

#[test]
fn fatal_actor_exit_writes_durable_failure_completion() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("actor-failure");
    let options = isolated_options(
        session.clone(),
        vec![
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from("printf fatal-output"),
        ],
    );

    let result = record_to(&options, &mut FailingOutput);
    assert!(result.is_err(), "failing output sink must stop the actor");
    let completion = read_completion(&session).expect("failure completion");
    let failure = completion.failure.as_ref().expect("recorder failure");
    assert_eq!(failure.code, "recorder_actor_failed");
    assert!(completion.durability.final_sync_completed);
    assert!(!session.join(SESSION_FAILURE_NAME).exists());
    let log = EventLog::read(session.join("events.jsonl")).expect("failure event log");
    assert!(matches!(
        log.events.last().map(|record| return &record.event),
        Some(Event::Exit {
            code: None,
            signal_name: None,
            ..
        })
    ));
    assert!(log.events.iter().any(|record| matches!(
        &record.event,
        Event::HostError { code, .. } if code == "recorder_actor_failed"
    )));
    validate_completed_log(&session, &log).expect("valid durable failure completion");
}
#[test]
fn poisoned_terminal_exit_append_preserves_incomplete_failure_without_completion() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("poisoned-fatal-append");
    let options = isolated_options(
        session.clone(),
        vec![
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from("printf fatal-output"),
        ],
    );
    POISON_FATAL_APPEND.with(|poison| poison.set(true));

    let result = record_to(&options, &mut FailingOutput);

    assert!(result.is_err(), "poisoned append must fail recording");
    assert!(
        !session.join("completion.json").exists(),
        "an unclosed canonical log must not have completion"
    );
    let diagnostic: serde_json::Value = serde_json::from_slice(
        &fs_err::read(session.join(SESSION_FAILURE_NAME)).expect("failure diagnostic"),
    )
    .expect("failure diagnostic JSON");
    assert_eq!(diagnostic["lifecycle"], "incomplete");
    assert_eq!(diagnostic["failure"]["code"], "recorder_actor_failed");
    assert_eq!(
        diagnostic["closure_failure"]["code"],
        "canonical_log_failed"
    );
    assert!(diagnostic["durable_boundary"]["event_hash"].is_string());
    let log = EventLog::read(session.join("events.jsonl")).expect("recoverable event prefix");
    assert!(matches!(
        log.events.last().map(|record| return &record.event),
        Some(Event::HostError { code, .. }) if code == "recorder_actor_failed"
    ));
    assert_eq!(
        diagnostic["durable_boundary"]["next_event_seq"].as_u64(),
        Some(u64::try_from(log.events.len()).expect("event count"))
    );
    assert!(matches!(
        validate_completed_log(&session, &log),
        Err(EvidenceError::SessionIncomplete { .. })
    ));
}

#[test]
fn spawn_failure_after_manifest_creation_writes_valid_fatal_closure() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("spawn-failure");
    let options = isolated_options(
        session.clone(),
        vec![OsString::from("/definitely/missing/termtrace-command")],
    );

    let result = record_to(&options, &mut Vec::new());

    assert!(result.is_err(), "missing command must fail to spawn");
    assert!(session.join("session.json").is_file());
    let completion = read_completion(&session).expect("spawn failure completion");
    assert_eq!(
        completion
            .failure
            .as_ref()
            .map(|failure| return failure.code.as_str()),
        Some("spawn_failed")
    );
    let log = EventLog::read(session.join("events.jsonl")).expect("spawn failure event log");
    assert!(matches!(
        log.events.last().map(|record| return &record.event),
        Some(Event::Exit {
            code: None,
            signal_name: None,
            ..
        })
    ));
    assert!(log.events.iter().any(|record| matches!(
        &record.event,
        Event::HostError { code, .. } if code == "spawn_failed"
    )));
    validate_completed_log(&session, &log).expect("valid spawn failure completion");
    assert!(!session.join(SESSION_FAILURE_NAME).exists());
}

#[test]
fn clear_environment_is_exact_and_secret_values_do_not_enter_evidence() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("isolated");
    let secret = b"not-for-any-artifact";
    let mut options = isolated_options(
        session.clone(),
        vec![
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from("test \"${#TERMTRACE_PRIVATE_TOKEN}\" -eq 20; test -z \"${HOME+x}\""),
        ],
    );
    options.environment = EnvironmentSpec {
        base: EnvironmentBase::Clear,
        set: BTreeMap::from([(
            "TERMTRACE_PRIVATE_TOKEN".to_owned(),
            ByteString::from(secret.as_slice()),
        )]),
        ..EnvironmentSpec::default()
    };

    let outcome = record(&options).expect("isolated recording");
    assert_eq!(outcome.exit_code, Some(0));
    let manifest: SessionManifest =
        serde_json::from_slice(&fs_err::read(session.join("session.json")).expect("manifest"))
            .expect("manifest JSON");
    assert_eq!(
        manifest.child_environment.explicit_names,
        BTreeSet::from(["TERMTRACE_PRIVATE_TOKEN".to_owned()])
    );
    for entry in fs_err::read_dir(&session).expect("session entries") {
        let path = entry.expect("entry").path();
        if path.is_file() {
            let bytes = fs_err::read(&path).expect("artifact bytes");
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|window| return window == secret),
                "secret leaked into {}",
                path.display()
            );
        }
    }
}

#[test]
fn output_budget_retains_observed_bytes_and_records_termination() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("budget");
    let mut options = isolated_options(
        session.clone(),
        vec![
            OsString::from("/usr/bin/python3"),
            OsString::from("-c"),
            OsString::from("import os,time; os.write(1,b'x'*131072); time.sleep(30)"),
        ],
    );
    options.resource_limits.output_bytes = Some(1024);

    let mut mirrored_output = Vec::new();
    let outcome = record_to(&options, &mut mirrored_output).expect("budgeted recording");
    assert!(outcome.output_bytes > 1024);
    assert_eq!(mirrored_output.len() as u64, outcome.output_bytes);
    let completion: SessionCompletion =
        serde_json::from_slice(&fs_err::read(session.join("completion.json")).expect("completion"))
            .expect("completion JSON");
    let limit = completion.resource_limit.expect("resource outcome");
    assert_eq!(limit.kind, ResourceLimitKind::OutputBytes);
    assert_eq!(limit.limit, 1024);
    assert!(limit.observed > limit.limit);
    assert_eq!(completion.output_bytes, outcome.output_bytes);
    let log = EventLog::read(session.join("events.jsonl")).expect("event log");
    let retained = log.events.iter().fold(0_u64, |total, record| {
        return total
            + match &record.event {
                Event::Output { data } => data.as_bytes().len() as u64,
                _ => 0,
            };
    });
    assert_eq!(retained, completion.output_bytes);
    let signal_operation = log.events.iter().find_map(|record| {
        return match &record.event {
            Event::SignalIntent {
                operation_id, name, ..
            } if name == "SIGTERM" => Some(*operation_id),
            _ => None,
        };
    });
    assert!(signal_operation.is_some());
    assert!(log.events.iter().any(|record| {
        return matches!(
            &record.event,
            Event::SignalResult { operation_id, .. }
                if Some(*operation_id) == signal_operation
        );
    }));
    assert!(log.events.iter().any(|record| {
        return matches!(
            &record.event,
            Event::HostError { code, .. } if code == "output_budget_exceeded"
        );
    }));
    validate_completed_log(&session, &log).expect("valid completed budget evidence");
}

#[test]
fn completed_output_burst_refreshes_the_pty_drain_deadline() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("completed-burst");
    let options = isolated_options(
        session.clone(),
        vec![
            OsString::from("/usr/bin/python3"),
            OsString::from("-c"),
            OsString::from(
                "import os; data=b''.join(b'\\x1b[2J\\x1b[HSTATE-%04d\\x1b[K\\x1b[?2026h\\x1b[?2026l'%i for i in range(1500)); os.write(1,data)",
            ),
        ],
    );

    let mut mirrored_output: Vec<u8> = Vec::new();
    let outcome = record_to(&options, &mut mirrored_output).expect("completed burst recording");
    assert_eq!(outcome.exit_code, Some(0));
    assert!(mirrored_output.len() > 24_000);
    let completion = read_completion(&session).expect("completed burst completion");
    assert!(completion.failure.is_none());
    assert_eq!(
        completion.output_bytes,
        u64::try_from(mirrored_output.len()).expect("output length")
    );
}

#[test]
fn spawn_and_provenance_share_the_effective_child_path() {
    let temporary = TempDir::new().expect("temporary directory");
    let executable = temporary.path().join("child-path-command");
    fs_err::write(&executable, b"#!/bin/sh\nprintf CHILD_PATH\n").expect("fixture");
    fs_err::set_permissions(&executable, Permissions::from_mode(0o700)).expect("mode");
    let session = temporary.path().join("child-path");
    let mut options = isolated_options(session.clone(), vec![OsString::from("child-path-command")]);
    options.environment = EnvironmentSpec {
        base: EnvironmentBase::Clear,
        set: BTreeMap::from([(
            "PATH".to_owned(),
            ByteString::from(temporary.path().as_os_str().as_bytes()),
        )]),
        ..EnvironmentSpec::default()
    };
    let mut output = Vec::new();
    let outcome = record_to(&options, &mut output).expect("record child PATH command");
    assert_eq!(outcome.exit_code, Some(0));
    assert_eq!(output, b"CHILD_PATH");
    let manifest: SessionManifest =
        serde_json::from_slice(&fs_err::read(session.join("session.json")).expect("manifest"))
            .expect("manifest JSON");
    assert_eq!(
        manifest
            .execution_provenance
            .executable
            .resolved
            .as_ref()
            .map(ByteString::as_bytes),
        Some(executable.as_os_str().as_bytes())
    );
}

#[test]
fn evicted_mutation_outcomes_are_loaded_from_the_persistent_journal() {
    let temporary = TempDir::new().expect("temporary directory");
    let journal = MutationJournal::create(temporary.path()).expect("mutation journal");
    let mut history = MutationHistory::new(journal, 1);
    let first_id = Uuid::now_v7();
    let first_command = ControlCommand::Mark {
        label: "first".to_owned(),
    };
    let first_response = cached_test_response(first_id, 1);
    history
        .remember(
            first_id,
            &first_command,
            guarded_command_hash(&first_command, None).expect("command hash"),
            0,
            1,
            "1".repeat(64),
            first_response.clone(),
        )
        .expect("remember first mutation");
    let second_id = Uuid::now_v7();
    let second_command = ControlCommand::Mark {
        label: "second".to_owned(),
    };
    history
        .remember(
            second_id,
            &second_command,
            guarded_command_hash(&second_command, None).expect("command hash"),
            1,
            2,
            "2".repeat(64),
            cached_test_response(second_id, 2),
        )
        .expect("remember second mutation");
    let journal_path = temporary.path().join(MUTATION_JOURNAL_NAME);
    let moved_journal_path = temporary.path().join("temporarily-moved-requests.jsonl");
    fs_err::rename(&journal_path, &moved_journal_path).expect("move mutation journal");
    let (_, cached_retry) = history
        .lookup(second_id, &second_command, None)
        .expect("load cached mutation");
    assert!(cached_retry.is_some());
    assert!(
        history.lookup(first_id, &first_command, None).is_err(),
        "evicted mutation must require its persistent journal"
    );
    fs_err::rename(&moved_journal_path, &journal_path).expect("restore mutation journal");

    let (_, retry) = history
        .lookup(first_id, &first_command, None)
        .expect("load evicted mutation");
    assert_eq!(retry, Some(first_response));
}

#[test]
fn records_exact_output_and_exit_after_output_close() {
    let temporary = TempDir::new().expect("temporary directory");
    let session = temporary.path().join("recording");
    let options = RecordOptions {
        output_directory: session.clone(),
        creation_alias: None,
        lineage: None,
        import_provenance: None,
        tmux_pane: None,
        command: vec![
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from(r"printf '\033[31mhello\033[0m\377'"),
        ],
        working_directory: None,
        cols: 80,
        rows: 24,
        cell_width_px: 9,
        cell_height_px: 18,
        geometry_provenance: GeometryProvenance::default(),
        ghostty_profile: None,
        term: "xterm-ghostty".to_owned(),
        environment: EnvironmentSpec::default(),
        colors: TerminalColors::default(),
        forward_stdin: false,
        continue_after_output_mirror_closes: false,
        durability_policy: DurabilityPolicy::default(),
        provenance_policy: ProvenancePolicy::default(),
        resource_limits: ResourceLimits::default(),
    };

    let mut mirrored_output = Vec::new();
    let outcome = record_to(&options, &mut mirrored_output).expect("record command");
    assert_eq!(outcome.exit_code, Some(0));
    assert_eq!(
        fs_err::metadata(&session)
            .expect("session metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    let recording = EventLog::read(&outcome.event_log).expect("valid event log");
    let output = recording
        .events
        .iter()
        .filter_map(|record| match &record.event {
            Event::Output { data } => return Some(data.as_bytes()),
            _ => return None,
        })
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(output, b"\x1b[31mhello\x1b[0m\xff");
    assert_eq!(mirrored_output, output);
    assert!(matches!(
        recording.events.last().map(|record| return &record.event),
        Some(Event::Exit { code: Some(0), .. })
    ));
    let output_closed_index = recording
        .events
        .iter()
        .position(|record| {
            return matches!(
                record.event,
                Event::Lifecycle {
                    lifecycle_kind: LifecycleKind::OutputClosed,
                    ..
                }
            );
        })
        .expect("output closed event");
    assert!(output_closed_index < recording.events.len() - 1);
    let manifest: SessionManifest = serde_json::from_slice(
        &fs_err::read(session.join("session.json")).expect("session manifest"),
    )
    .expect("valid session manifest");
    let completion: SessionCompletion = serde_json::from_slice(
        &fs_err::read(session.join("completion.json")).expect("completion manifest"),
    )
    .expect("valid completion manifest");
    assert_eq!(manifest.session_id, recording.header.session_id);
    assert_eq!(completion.session_id, recording.header.session_id);
    assert_eq!(
        completion.final_event_hash,
        recording.events.last().unwrap().hash
    );
    assert_eq!(completion.event_count, recording.events.len() as u64);
    assert_eq!(manifest.command[0].as_bytes(), b"/bin/sh");
}

#[test]
fn paste_encoding_is_explicit_and_rejects_delimiter_injection() {
    assert_eq!(
        encode_paste(b"one\ntwo", true).expect("bracketed paste"),
        b"\x1b[200~one\ntwo\x1b[201~"
    );
    assert_eq!(
        encode_paste(b"one\ntwo", false).expect("plain paste"),
        b"one\rtwo"
    );
    assert!(encode_paste(b"unsafe\x1b[201~command", true).is_err());
}

#[test]
fn refuses_to_overwrite_existing_session_directory() {
    let temporary = TempDir::new().expect("temporary directory");
    let options = RecordOptions {
        output_directory: temporary.path().to_path_buf(),
        creation_alias: None,
        lineage: None,
        import_provenance: None,
        tmux_pane: None,
        command: vec![OsString::from("true")],
        working_directory: None,
        cols: 80,
        rows: 24,
        cell_width_px: 9,
        cell_height_px: 18,
        geometry_provenance: GeometryProvenance::default(),
        ghostty_profile: None,
        term: "xterm-ghostty".to_owned(),
        environment: EnvironmentSpec::default(),
        colors: TerminalColors::default(),
        forward_stdin: false,
        continue_after_output_mirror_closes: false,
        durability_policy: DurabilityPolicy::default(),
        provenance_policy: ProvenancePolicy::default(),
        resource_limits: ResourceLimits::default(),
    };

    let error = record(&options).expect_err("existing directory must fail");
    assert!(matches!(error, Error::SessionExists { .. }));
}

#[test]
fn terminates_descendants_that_retain_the_slave_pty() {
    let temporary = TempDir::new().expect("temporary directory");
    let options = RecordOptions {
        output_directory: temporary.path().join("recording"),
        creation_alias: None,
        lineage: None,
        import_provenance: None,
        tmux_pane: None,
        command: vec![
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from("(trap '' HUP; exec sleep 30) & printf done"),
        ],
        working_directory: None,
        cols: 80,
        rows: 24,
        cell_width_px: 9,
        cell_height_px: 18,
        geometry_provenance: GeometryProvenance::default(),
        ghostty_profile: None,
        term: "xterm-ghostty".to_owned(),
        environment: EnvironmentSpec::default(),
        colors: TerminalColors::default(),
        forward_stdin: false,
        continue_after_output_mirror_closes: false,
        durability_policy: DurabilityPolicy::default(),
        provenance_policy: ProvenancePolicy::default(),
        resource_limits: ResourceLimits::default(),
    };
    let started = Instant::now();
    let mut output = Vec::new();

    let outcome = record_to(&options, &mut output).expect("record command");

    assert_eq!(outcome.exit_code, Some(0));
    assert_eq!(output, b"done");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "retained PTY was not terminated promptly"
    );
}

#[test]
fn answers_and_audits_terminal_queries() {
    let temporary = TempDir::new().expect("temporary directory");
    let options = RecordOptions {
        output_directory: temporary.path().join("query"),
        creation_alias: None,
        lineage: None,
        import_provenance: None,
        tmux_pane: None,
        command: vec![
            OsString::from("/usr/bin/python3"),
            OsString::from("-c"),
            OsString::from(
                "import os,tty; tty.setraw(0); os.write(1,b'\\x1b[6n'); r=os.read(0,32); os.write(1,b'GOT:'+r.hex().encode())",
            ),
        ],
        working_directory: None,
        cols: 80,
        rows: 24,
        cell_width_px: 9,
        cell_height_px: 18,
        geometry_provenance: GeometryProvenance::default(),
        ghostty_profile: None,
        term: "xterm-ghostty".to_owned(),
        environment: EnvironmentSpec::default(),
        colors: TerminalColors::default(),
        forward_stdin: false,
        continue_after_output_mirror_closes: false,
        durability_policy: DurabilityPolicy::default(),
        provenance_policy: ProvenancePolicy::default(),
        resource_limits: ResourceLimits::default(),
    };
    let mut output = Vec::new();

    let outcome = record_to(&options, &mut output).expect("record query command");
    let log = EventLog::read(outcome.event_log).expect("valid query log");

    assert!(output.windows(4).any(|window| return window == b"GOT:"));
    assert!(log.events.iter().any(|record| matches!(
        &record.event,
        Event::QueryResponse { query, response, .. }
            if query.as_bytes() == b"\x1b[6n" && response.as_bytes() == b"\x1b[1;1R"
    )));
    assert!(log.events.iter().any(|record| matches!(
        record.event,
        Event::InputIntent {
            source: InputSource::Host,
            ..
        }
    )));
}

#[test]
fn preserves_complete_osc_query_when_response_precedes_st() {
    let temporary = TempDir::new().expect("temporary directory");
    let options = RecordOptions {
        output_directory: temporary.path().join("osc-query"),
        creation_alias: None,
        lineage: None,
        import_provenance: None,
        tmux_pane: None,
        command: vec![
            OsString::from("/usr/bin/python3"),
            OsString::from("-c"),
            OsString::from(
                "import os,tty; tty.setraw(0); q=b'\\x1b]11;?\\x1b\\\\'; os.write(1,q); r=b'';\nwhile not r.endswith(b'\\x1b\\\\'): r+=os.read(0,99)\nos.write(1,b'GOT:'+r.hex().encode())",
            ),
        ],
        working_directory: None,
        cols: 80,
        rows: 24,
        cell_width_px: 9,
        cell_height_px: 18,
        geometry_provenance: GeometryProvenance::default(),
        ghostty_profile: None,
        term: "xterm-ghostty".to_owned(),
        environment: EnvironmentSpec::default(),
        colors: TerminalColors::default(),
        forward_stdin: false,
        continue_after_output_mirror_closes: false,
        durability_policy: DurabilityPolicy::default(),
        provenance_policy: ProvenancePolicy::default(),
        resource_limits: ResourceLimits::default(),
    };
    let mut output = Vec::new();

    let outcome = record_to(&options, &mut output).expect("record OSC query command");
    let log = EventLog::read(outcome.event_log).expect("valid OSC query log");

    assert!(log.events.iter().any(|record| matches!(
        &record.event,
        Event::QueryResponse { query, response, .. }
            if query.as_bytes() == b"\x1b]11;?\x1b\\"
                && response.as_bytes() == b"\x1b]11;rgb:0000/0000/0000\x1b\\"
    )));
}

#[test]
fn presentation_tracker_recognizes_only_synchronized_output_mode_commands() {
    for (input, expected) in [
        (
            b"visible\x1b[?2026hhidden\x1b[?2026l".as_slice(),
            vec![true, false],
        ),
        (b"\x1b[?25;2026h".as_slice(), vec![true]),
        (b"\x9b?2026l".as_slice(), vec![false]),
        (b"\x1b[2026h".as_slice(), Vec::new()),
        (b"\x1b[?20260h".as_slice(), Vec::new()),
        (b"\x1b[?2026:1h".as_slice(), Vec::new()),
        (b"\x1b[?2026\x18h".as_slice(), Vec::new()),
    ] {
        let mut tracker = PresentationTracker::new(false);
        let mut commands = Vec::new();
        let mut framed = Vec::new();
        for byte in input {
            match tracker.push(*byte) {
                PresentationAction::Immediate => framed.push(*byte),
                PresentationAction::Buffered => {}
                PresentationAction::Sequence { bytes, command } => {
                    framed.extend(bytes);
                    if let Some(command) = command {
                        commands.push(command);
                    }
                }
            }
        }
        framed.extend(tracker.finish());
        assert_eq!(commands, expected, "input: {input:?}");
        assert_eq!(framed, input, "framing must be lossless");
    }

    const TRACKED_SEQUENCE_BOUND: usize = 64 * 1024;
    let mut tracker = PresentationTracker::new(false);
    let mut longest_sequence = 0;
    for byte in b"\x1b[?"
        .iter()
        .copied()
        .chain(iter::repeat_n(b'0', TRACKED_SEQUENCE_BOUND * 2))
    {
        if let PresentationAction::Sequence { bytes, .. } = tracker.push(byte) {
            longest_sequence = longest_sequence.max(bytes.len());
        }
    }
    longest_sequence = longest_sequence.max(tracker.finish().len());
    assert!(longest_sequence <= TRACKED_SEQUENCE_BOUND + 1);
}

#[test]
fn control_shutdown_cancels_an_inflight_request() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let socket_name = control_socket_name(Uuid::now_v7());
    let worker = spawn_control_server(&socket_name, sender).expect("control server");
    let socket = control_socket_address(&socket_name);
    let client = thread::spawn(move || {
        let mut stream = UnixStream::connect_addr(&socket).expect("connect control client");
        let request = ControlRequest::new(ControlCommand::Ping);
        serde_json::to_writer(&mut stream, &request).expect("encode request");
        stream.write_all(b"\n").expect("terminate request");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("read shutdown");
    });
    let pending = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("inflight request");

    let started = Instant::now();
    worker.stop().expect("cancel control server");
    assert!(started.elapsed() < Duration::from_secs(1));
    drop(pending);
    client.join().expect("join control client");
}

#[test]
fn control_server_accepts_a_second_request_while_the_first_is_pending() {
    let (sender, receiver) = mpsc::sync_channel(1);
    let socket_name = control_socket_name(Uuid::now_v7());
    let worker = spawn_control_server(&socket_name, sender).expect("control server");
    let socket = control_socket_address(&socket_name);
    let first_socket = socket.clone();
    let first_client = thread::spawn(move || {
        let mut stream = UnixStream::connect_addr(&first_socket).expect("connect first client");
        let request = ControlRequest::new(ControlCommand::Ping);
        serde_json::to_writer(&mut stream, &request).expect("encode first request");
        stream.write_all(b"\n").expect("terminate first request");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .expect("read first shutdown");
    });
    let first = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("first inflight request");

    let second_client = thread::spawn(move || {
        let mut stream = UnixStream::connect_addr(&socket).expect("connect second client");
        let request = ControlRequest::new(ControlCommand::Ping);
        serde_json::to_writer(&mut stream, &request).expect("encode second request");
        stream.write_all(b"\n").expect("terminate second request");
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .expect("read second shutdown");
    });
    let second = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("second concurrent request");

    worker.stop().expect("cancel control server");
    drop(first);
    drop(second);
    first_client.join().expect("join first client");
    second_client.join().expect("join second client");
}
