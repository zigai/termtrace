use crate::cli::output::ResponseWriter;

use std::{
    path::Path,
    process::ExitCode,
    thread,
    time::{Duration, Instant},
};

use miette::{IntoDiagnostic as _, Result};
use termtrace_engine::timeline::resolve_source_selection;
use termtrace_evidence::{
    artifact::write_secure, event_log::EventLog, session::validate_completed_log,
};
use termtrace_model::{
    execution::event::{Event, EventProjection, event_type},
    protocol::control::{ControlCommand, ControlData},
    protocol::output_projection::{OutputProjectionQuality, OutputProjectionReport},
};
use termtrace_sdk::request;
use termtrace_terminal::ControlStripper;

use crate::cli::{
    analysis::args::{EventsArgs, OutputArgs},
    output::{
        paginate, projection_warning, write_follow_json_line, write_follow_stdout, write_json_line,
        write_projection_warning, write_stdout,
    },
    session_helpers::resolve_session_ref,
};

pub(in crate::cli) fn run_output(
    response: &mut ResponseWriter,
    arguments: OutputArgs,
) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    if arguments.follow {
        if arguments.manifest.is_some() {
            return Err(miette::miette!(
                "--manifest is available only for completed non-follow output"
            ));
        }
        if arguments.range.from_marker.is_some()
            || arguments.range.to_marker.is_some()
            || arguments.range.start_monotonic_ns.is_some()
            || arguments.range.end_monotonic_ns.is_some()
        {
            return Err(miette::miette!(
                "live output following currently supports event-sequence bounds only"
            ));
        }
        return follow_output(
            response,
            &session,
            arguments.from_event_seq.unwrap_or(0),
            arguments.to_event_seq,
            arguments.strip_controls,
        );
    }
    let log = EventLog::read(session.join("events.jsonl")).into_diagnostic()?;
    let completion = validate_completed_log(&session, &log).into_diagnostic()?;
    let time_range = resolve_source_selection(
        &log,
        arguments.range.start_monotonic_ns,
        arguments.range.end_monotonic_ns,
        arguments.range.from_marker.as_deref(),
        arguments.range.to_marker.as_deref(),
    )
    .into_diagnostic()?;
    let event_count = u64::try_from(log.events.len()).unwrap_or(u64::MAX);
    let from_event = arguments.from_event_seq.unwrap_or(0);
    let to_event = arguments.to_event_seq.unwrap_or(event_count);
    if from_event > to_event || to_event > event_count {
        return Err(miette::miette!(
            "output event range must satisfy 0 <= FROM <= TO <= {event_count}"
        ));
    }
    let mut output = Vec::new();
    for record in &log.events {
        if record.seq < from_event
            || record.seq >= to_event
            || time_range.is_some_and(|range| return !range.contains(record.monotonic_ns))
        {
            continue;
        }
        if let Event::Output { data } = &record.event {
            output.extend_from_slice(data.as_bytes());
        }
    }
    let source = output;
    let (output, quality, dropped, alternate_screen) = if arguments.strip_controls {
        let mut stripper = ControlStripper::default();
        let output = stripper.feed(&source);
        let alternate_screen = stripper.alternate_screen_observed();
        let quality = if alternate_screen {
            OutputProjectionQuality::UnsuitableForFullScreen
        } else {
            OutputProjectionQuality::LossyLineOriented
        };
        (
            output,
            quality,
            stripper
                .source_byte_count()
                .saturating_sub(stripper.output_byte_count()),
            alternate_screen,
        )
    } else {
        (
            source.clone(),
            OutputProjectionQuality::ExactBytes,
            0,
            false,
        )
    };
    let warning = projection_warning(quality);
    if let Some(path) = &arguments.manifest {
        let report = OutputProjectionReport {
            format: "termtrace-output-projection".to_owned(),
            version: 1,
            session_id: log.header.session_id,
            final_event_hash: completion.final_event_hash,
            from_event_seq: from_event,
            to_event_seq: to_event,
            mode: if arguments.raw {
                "raw"
            } else {
                "strip_controls"
            }
            .to_owned(),
            projection_quality: quality,
            source_byte_count: u64::try_from(source.len()).unwrap_or(u64::MAX),
            source_blake3: blake3::hash(&source).to_hex().to_string(),
            output_byte_count: u64::try_from(output.len()).unwrap_or(u64::MAX),
            output_blake3: blake3::hash(&output).to_hex().to_string(),
            dropped_byte_count: dropped,
            alternate_screen_observed: alternate_screen,
            warning: warning.map(str::to_owned),
        };
        let mut bytes = serde_json::to_vec_pretty(&report).into_diagnostic()?;
        bytes.push(b'\n');
        write_secure(path, &bytes).into_diagnostic()?;
    }
    if arguments.strip_controls {
        write_projection_warning(
            response,
            warning.unwrap_or("control-stripped output is lossy"),
        )?;
    }
    return write_stdout(response, output);
}

fn follow_output(
    response: &mut ResponseWriter,
    session: &Path,
    mut next_event_seq: u64,
    end_event_seq: Option<u64>,
    strip_controls: bool,
) -> Result<ExitCode> {
    let mut control_failure_since = None;
    let mut stripper = ControlStripper::default();
    loop {
        let completion_exists = session.join("completion.json").is_file();
        let log = if completion_exists {
            let log = EventLog::read(session.join("events.jsonl")).into_diagnostic()?;
            validate_completed_log(session, &log).into_diagnostic()?;
            log
        } else {
            let response = match request(session, ControlCommand::Status) {
                Ok(response) => {
                    control_failure_since = None;
                    response
                }
                Err(_) if session.join("completion.json").is_file() => continue,
                Err(error) => {
                    let since = control_failure_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= Duration::from_secs(2) {
                        return Err(error).into_diagnostic();
                    }
                    thread::sleep(Duration::from_millis(20));
                    continue;
                }
            };
            let ControlData::Status { boundaries, .. } = response else {
                return Err(miette::miette!(
                    "live status returned an unexpected response"
                ));
            };
            EventLog::read_prefix(
                session.join("events.jsonl"),
                boundaries.durable.next_event_seq,
            )
            .into_diagnostic()?
        };
        let available = u64::try_from(log.events.len()).unwrap_or(u64::MAX);
        if next_event_seq > available {
            return Err(miette::miette!(
                "output resume cursor {next_event_seq} exceeds durable event count {available}"
            ));
        }
        let emit_end = end_event_seq.unwrap_or(available).min(available);
        let mut bytes = Vec::new();
        for record in &log.events {
            if record.seq < next_event_seq || record.seq >= emit_end {
                continue;
            }
            if let Event::Output { data } = &record.event {
                bytes.extend_from_slice(data.as_bytes());
            }
        }
        let emitted = if strip_controls {
            stripper.feed(&bytes)
        } else {
            bytes
        };
        if !emitted.is_empty() && !write_follow_stdout(response, &emitted)? {
            return Ok(ExitCode::SUCCESS);
        }
        next_event_seq = emit_end;
        if completion_exists && end_event_seq.is_some_and(|end| return end > available) {
            return Err(miette::miette!(
                "output end cursor exceeds completed event count {available}"
            ));
        }
        if end_event_seq.is_some_and(|end| return next_event_seq >= end)
            || (completion_exists && next_event_seq >= available)
        {
            if strip_controls {
                let quality = if stripper.alternate_screen_observed() {
                    OutputProjectionQuality::UnsuitableForFullScreen
                } else {
                    OutputProjectionQuality::LossyLineOriented
                };
                write_projection_warning(
                    response,
                    projection_warning(quality).unwrap_or("control-stripped output is lossy"),
                )?;
            }
            return Ok(ExitCode::SUCCESS);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

pub(in crate::cli) fn run_events(
    response: &mut ResponseWriter,
    arguments: EventsArgs,
) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    if arguments.follow {
        if arguments.range.from_marker.is_some()
            || arguments.range.to_marker.is_some()
            || arguments.range.start_monotonic_ns.is_some()
            || arguments.range.end_monotonic_ns.is_some()
        {
            return Err(miette::miette!(
                "live event following currently supports event-sequence bounds only"
            ));
        }
        return follow_events(
            response,
            &session,
            arguments.from_event_seq.unwrap_or(0),
            arguments.to_event_seq,
            &arguments.event_types,
            arguments.metadata_only,
        );
    }
    let log = EventLog::read(session.join("events.jsonl")).into_diagnostic()?;
    validate_completed_log(&session, &log).into_diagnostic()?;
    let time_range = resolve_source_selection(
        &log,
        arguments.range.start_monotonic_ns,
        arguments.range.end_monotonic_ns,
        arguments.range.from_marker.as_deref(),
        arguments.range.to_marker.as_deref(),
    )
    .into_diagnostic()?;
    let event_count = u64::try_from(log.events.len()).unwrap_or(u64::MAX);
    let from_event = arguments.from_event_seq.unwrap_or(0);
    let to_event = arguments.to_event_seq.unwrap_or(event_count);
    if from_event > to_event || to_event > event_count {
        return Err(miette::miette!(
            "event range must satisfy 0 <= FROM <= TO <= {event_count}"
        ));
    }
    let mut records = Vec::new();
    for record in &log.events {
        let kind = event_type(&record.event);
        if record.seq < from_event
            || record.seq >= to_event
            || time_range.is_some_and(|range| return !range.contains(record.monotonic_ns))
            || (!arguments.event_types.is_empty()
                && !arguments
                    .event_types
                    .iter()
                    .any(|value| return value == kind))
        {
            continue;
        }
        records.push(record);
    }
    let next_sequences = records
        .iter()
        .map(|record| return record.seq)
        .collect::<Vec<_>>();
    let projections = records
        .into_iter()
        .map(|record| return EventProjection::from_record(record, arguments.metadata_only))
        .collect::<Vec<_>>();
    let (projections, page, partial) = paginate(projections, &arguments.page);
    for projection in projections {
        write_json_line(response, &projection)?;
    }
    if partial {
        let next_event_seq = page
            .next_offset
            .and_then(|offset| return next_sequences.get(offset).copied());
        let mut page = serde_json::to_value(page).into_diagnostic()?;
        if let Some(object) = page.as_object_mut() {
            object.insert(
                "next_event_seq".to_owned(),
                serde_json::json!(next_event_seq),
            );
        }
        write_json_line(
            response,
            &serde_json::json!({
                "kind": "output_page",
                "page": page,
            }),
        )?;
    }
    return Ok(ExitCode::SUCCESS);
}

fn follow_events(
    response: &mut ResponseWriter,
    session: &Path,
    mut next_event_seq: u64,
    end_event_seq: Option<u64>,
    event_types: &[String],
    metadata_only: bool,
) -> Result<ExitCode> {
    let mut control_failure_since = None;
    loop {
        let completion_exists = session.join("completion.json").is_file();
        let log = if completion_exists {
            let log = EventLog::read(session.join("events.jsonl")).into_diagnostic()?;
            validate_completed_log(session, &log).into_diagnostic()?;
            log
        } else {
            let response = match request(session, ControlCommand::Status) {
                Ok(response) => {
                    control_failure_since = None;
                    response
                }
                Err(_) if session.join("completion.json").is_file() => continue,
                Err(error) => {
                    let since = control_failure_since.get_or_insert_with(Instant::now);
                    if since.elapsed() >= Duration::from_secs(2) {
                        return Err(error).into_diagnostic();
                    }
                    thread::sleep(Duration::from_millis(20));
                    continue;
                }
            };
            let ControlData::Status { boundaries, .. } = response else {
                return Err(miette::miette!(
                    "live status returned an unexpected response"
                ));
            };
            EventLog::read_prefix(
                session.join("events.jsonl"),
                boundaries.durable.next_event_seq,
            )
            .into_diagnostic()?
        };
        let available = u64::try_from(log.events.len()).unwrap_or(u64::MAX);
        if next_event_seq > available {
            return Err(miette::miette!(
                "event resume cursor {next_event_seq} exceeds durable event count {available}"
            ));
        }
        let emit_end = end_event_seq.unwrap_or(available).min(available);
        for record in &log.events {
            let kind = event_type(&record.event);
            if record.seq < next_event_seq
                || record.seq >= emit_end
                || (!event_types.is_empty()
                    && !event_types.iter().any(|value| return value == kind))
            {
                continue;
            }
            if !write_follow_json_line(
                response,
                &EventProjection::from_record(record, metadata_only),
            )? {
                return Ok(ExitCode::SUCCESS);
            }
        }
        next_event_seq = emit_end;
        if completion_exists && end_event_seq.is_some_and(|end| return end > available) {
            return Err(miette::miette!(
                "event end cursor exceeds completed event count {available}"
            ));
        }
        if end_event_seq.is_some_and(|end| return next_event_seq >= end)
            || (completion_exists && next_event_seq >= available)
        {
            return Ok(ExitCode::SUCCESS);
        }
        thread::sleep(Duration::from_millis(20));
    }
}
