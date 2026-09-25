use crate::cli::output::ResponseWriter;
use crate::cli::{
    analysis::args::{EventsArgs, OutputArgs},
    output::{
        projection_warning, response_output, write_follow_json_line, write_follow_stdout,
        write_json_line, write_projection_warning,
    },
    session_helpers::resolve_session_path,
};
use miette::{IntoDiagnostic as _, Result};
use std::{
    io::{self, BufRead as _, BufReader, BufWriter, Seek as _, SeekFrom, Write as _},
    path::Path,
    process::ExitCode,
    thread,
    time::{Duration, Instant},
};
use termtrace_control::{request_or_completed, Reached};
use termtrace_engine::timeline::resolve_session_source_selection;
use termtrace_evidence::{
    artifact::write_secure, event_log::EventLogCursor, session::validate_completed_session,
};
use termtrace_model::{
    execution::{
        event::{event_type, Event, EventProjection, EventRecord},
        layout::SessionLayout,
    },
    protocol::control::{ControlCommand, ControlData},
    protocol::output_projection::{OutputProjectionQuality, OutputProjectionReport},
};
use termtrace_terminal::ControlStripper;

pub(in crate::cli) fn run_output(
    response: &mut ResponseWriter,
    arguments: OutputArgs,
) -> Result<ExitCode> {
    let session = resolve_session_path(arguments.session)?;

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

    let completion = validate_completed_session(&session).into_diagnostic()?;
    let time_range = resolve_session_source_selection(
        &session,
        arguments.range.start_monotonic_ns,
        arguments.range.end_monotonic_ns,
        arguments.range.from_marker.as_deref(),
        arguments.range.to_marker.as_deref(),
    )
    .into_diagnostic()?;

    let event_count = completion.event_count;
    let from_event = arguments.from_event_seq.unwrap_or(0);
    let to_event = arguments.to_event_seq.unwrap_or(event_count);
    if from_event > to_event || to_event > event_count {
        return Err(miette::miette!(
            "output event range must satisfy 0 <= FROM <= TO <= {event_count}"
        ));
    }

    let mut cursor =
        EventLogCursor::open(SessionLayout::new(&session).event_log()).into_diagnostic()?;
    let session_id = cursor.header().session_id;
    let mut staged = tempfile::tempfile().into_diagnostic()?;
    let mut source_hash = blake3::Hasher::new();
    let mut output_hash = blake3::Hasher::new();
    let mut source_bytes = 0_u64;
    let mut output_bytes = 0_u64;
    let mut stripper = ControlStripper::default();

    {
        let mut writer = BufWriter::new(&mut staged);
        while let Some(record) = cursor.next_record().into_diagnostic()? {
            if record.seq < from_event
                || record.seq >= to_event
                || time_range.is_some_and(|range| return !range.contains(record.monotonic_ns))
            {
                continue;
            }

            if let Event::Output { data } = record.event {
                source_hash.update(data.as_bytes());
                source_bytes = source_bytes
                    .saturating_add(u64::try_from(data.as_bytes().len()).unwrap_or(u64::MAX));

                let stripped;
                let bytes = if arguments.strip_controls {
                    stripped = stripper.feed(data.as_bytes());

                    stripped.as_slice()
                } else {
                    data.as_bytes()
                };
                output_hash.update(bytes);
                output_bytes =
                    output_bytes.saturating_add(u64::try_from(bytes.len()).unwrap_or(u64::MAX));

                writer.write_all(bytes).into_diagnostic()?;
            }
        }

        writer.flush().into_diagnostic()?;
    }

    cursor
        .validate_boundary(event_count, &completion.final_event_hash)
        .into_diagnostic()?;
    let _pending = cursor.finish().into_diagnostic()?;
    let alternate_screen = stripper.alternate_screen_observed();
    let quality = if !arguments.strip_controls {
        OutputProjectionQuality::ExactBytes
    } else if alternate_screen {
        OutputProjectionQuality::UnsuitableForFullScreen
    } else {
        OutputProjectionQuality::LossyLineOriented
    };

    let warning = projection_warning(quality);

    if let Some(path) = &arguments.manifest {
        let report = OutputProjectionReport {
            format: "termtrace-output-projection".to_owned(),
            version: 1,
            session_id,
            final_event_hash: completion.final_event_hash,
            from_event_seq: from_event,
            to_event_seq: to_event,
            mode: if arguments.raw { "raw" } else { "strip_controls" }.to_owned(),
            projection_quality: quality,
            source_byte_count: source_bytes,
            source_blake3: source_hash.finalize().to_hex().to_string(),
            output_byte_count: output_bytes,
            output_blake3: output_hash.finalize().to_hex().to_string(),
            dropped_byte_count: source_bytes.saturating_sub(output_bytes),
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

    staged.seek(SeekFrom::Start(0)).into_diagnostic()?;

    let mut output = response_output(response);

    match io::copy(&mut staged, &mut output).and_then(|_| return output.flush()) {
        Ok(()) => return Ok(ExitCode::SUCCESS),
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(ExitCode::SUCCESS),
        Err(error) => return Err(error).into_diagnostic(),
    }
}

fn follow_output(
    response: &mut ResponseWriter,
    session: &Path,
    next_event_seq: u64,
    end_event_seq: Option<u64>,
    strip_controls: bool,
) -> Result<ExitCode> {
    let mut stripper = ControlStripper::default();
    let result = follow_records(
        response,
        session,
        next_event_seq,
        end_event_seq,
        &["output".to_owned()],
        |response, record| {
            if let Event::Output { data } = record.event {
                let stripped;
                let bytes = if strip_controls {
                    stripped = stripper.feed(data.as_bytes());

                    stripped.as_slice()
                } else {
                    data.as_bytes()
                };

                return write_follow_stdout(response, bytes);
            }

            return Ok(true);
        },
    )?;

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

    return Ok(result);
}

pub(in crate::cli) fn run_events(
    response: &mut ResponseWriter,
    arguments: EventsArgs,
) -> Result<ExitCode> {
    let session = resolve_session_path(arguments.session)?;

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

    let completion = validate_completed_session(&session).into_diagnostic()?;
    let time_range = resolve_session_source_selection(
        &session,
        arguments.range.start_monotonic_ns,
        arguments.range.end_monotonic_ns,
        arguments.range.from_marker.as_deref(),
        arguments.range.to_marker.as_deref(),
    )
    .into_diagnostic()?;

    let event_count = completion.event_count;
    let from_event = arguments.from_event_seq.unwrap_or(0);
    let to_event = arguments.to_event_seq.unwrap_or(event_count);
    if from_event > to_event || to_event > event_count {
        return Err(miette::miette!(
            "event range must satisfy 0 <= FROM <= TO <= {event_count}"
        ));
    }

    let mut cursor =
        EventLogCursor::open(SessionLayout::new(&session).event_log()).into_diagnostic()?;
    let mut staged = tempfile::tempfile().into_diagnostic()?;
    let mut total = 0_usize;
    let mut returned = 0_usize;
    let mut next_event_seq = None;

    {
        let mut writer = BufWriter::new(&mut staged);
        while let Some(record) = cursor.next_record().into_diagnostic()? {
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

            if total >= arguments.page.offset {
                if arguments.page.all || returned < arguments.page.limit {
                    serde_json::to_writer(
                        &mut writer,
                        &EventProjection::from_record(&record, arguments.metadata_only),
                    )
                    .into_diagnostic()?;
                    writer.write_all(b"\n").into_diagnostic()?;
                    returned = returned.saturating_add(1);
                } else if next_event_seq.is_none() {
                    next_event_seq = Some(record.seq);
                }
            }

            total = total.saturating_add(1);
        }

        writer.flush().into_diagnostic()?;
    }

    cursor
        .validate_boundary(event_count, &completion.final_event_hash)
        .into_diagnostic()?;
    let _pending = cursor.finish().into_diagnostic()?;

    staged.seek(SeekFrom::Start(0)).into_diagnostic()?;

    for line in BufReader::new(&mut staged).lines() {
        let value: EventProjection =
            serde_json::from_str(&line.into_diagnostic()?).into_diagnostic()?;
        write_json_line(response, &value)?;
    }

    let start = arguments.page.offset.min(total);
    let end = start.saturating_add(returned);
    if start != 0 || end < total {
        write_json_line(
            response,
            &serde_json::json!({
                "kind": "output_page",
                "page": {
                    "offset": start, "limit": (!arguments.page.all).then_some(arguments.page.limit),
                    "total": total, "returned": returned, "truncated": end < total,
                    "next_offset": (end < total).then_some(end), "next_event_seq": next_event_seq,
                },
            }),
        )?;
    }

    return Ok(ExitCode::SUCCESS);
}

fn follow_events(
    response: &mut ResponseWriter,
    session: &Path,
    next_event_seq: u64,
    end_event_seq: Option<u64>,
    event_types: &[String],
    metadata_only: bool,
) -> Result<ExitCode> {
    return follow_records(
        response,
        session,
        next_event_seq,
        end_event_seq,
        event_types,
        |response, record| {
            return write_follow_json_line(
                response,
                &EventProjection::from_record(&record, metadata_only),
            );
        },
    );
}

fn follow_records(
    response: &mut ResponseWriter,
    session: &Path,
    mut next_event_seq: u64,
    end_event_seq: Option<u64>,
    event_types: &[String],
    mut emit: impl FnMut(&mut ResponseWriter, EventRecord) -> Result<bool>,
) -> Result<ExitCode> {
    let mut cursor =
        EventLogCursor::open(SessionLayout::new(session).event_log()).into_diagnostic()?;
    let mut initialized = false;
    let mut control_failure_since = None;
    let mut staged = tempfile::tempfile().into_diagnostic()?;
    let mut line = Vec::new();

    loop {
        let reached = match request_or_completed(
            session,
            ControlCommand::EventCursor { next_event_seq },
            Duration::ZERO,
        ) {
            Ok(reached) => {
                control_failure_since = None;
                reached
            }
            Err(error) => {
                let since = control_failure_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_secs(2) {
                    return Err(error).into_diagnostic();
                }

                thread::sleep(Duration::from_millis(20));

                continue;
            }
        };

        let (available, live_boundary, completed_hash) = if let Reached::Live(data) = reached {
            let ControlData::EventCursor {
                device,
                inode,
                header_hash,
                start,
                durable,
            } = *data
            else {
                return Err(miette::miette!(
                    "live cursor returned an unexpected response"
                ));
            };
            cursor
                .validate_live_source(device, inode, &header_hash)
                .into_diagnostic()?;

            if !initialized {
                if let Some(start) = start {
                    cursor
                        .resume_live_suffix(&start, device, inode, &header_hash)
                        .into_diagnostic()?;
                }

                // An absent retained start requires explicit historical streaming from
                // the header, never a cursor reconstructed from untrusted source bytes.
                initialized = true;
            }

            (durable.next_event_seq, Some(durable), None)
        } else {
            // A seeded suffix is not full-log operation verification. Validate the completed
            // source once, streaming, before allowing its final suffix to become observable.
            let completion = validate_completed_session(session).into_diagnostic()?;

            (
                completion.event_count,
                None,
                Some(completion.final_event_hash),
            )
        };
        let completed = completed_hash.is_some();

        if next_event_seq > available {
            return Err(miette::miette!(
                "event resume cursor {next_event_seq} exceeds durable event count {available}"
            ));
        }

        let emit_end = end_event_seq.unwrap_or(available).min(available);

        // Stage on an anonymous file, not in memory: nothing from a self-hashed suffix
        // is published until its endpoint matches the independent actor/completion.
        staged.set_len(0).into_diagnostic()?;
        staged.seek(SeekFrom::Start(0)).into_diagnostic()?;

        {
            let mut staged_writer = BufWriter::new(&mut staged);
            while cursor.next_event_seq() < available {
                let record = cursor.next_record().into_diagnostic()?.ok_or_else(|| {
                    return miette::miette!("durable event prefix is incomplete");
                })?;

                let kind = event_type(&record.event);

                if record.seq < next_event_seq
                    || record.seq >= emit_end
                    || (!event_types.is_empty()
                        && !event_types.iter().any(|value| return value == kind))
                {
                    continue;
                }

                serde_json::to_writer(&mut staged_writer, &record).into_diagnostic()?;
                staged_writer.write_all(b"\n").into_diagnostic()?;
            }

            staged_writer.flush().into_diagnostic()?;
        }

        if let Some(boundary) = live_boundary {
            cursor.validate_live_boundary(&boundary).into_diagnostic()?;
        } else if let Some(hash) = completed_hash {
            cursor
                .validate_boundary(available, &hash)
                .into_diagnostic()?;
        }

        staged.seek(SeekFrom::Start(0)).into_diagnostic()?;

        let mut staged_reader = BufReader::new(&mut staged);
        loop {
            line.clear();
            if staged_reader
                .read_until(b'\n', &mut line)
                .into_diagnostic()?
                == 0
            {
                break;
            }

            let record: EventRecord = serde_json::from_slice(&line).into_diagnostic()?;
            if !emit(response, record)? {
                return Ok(ExitCode::SUCCESS);
            }
        }

        next_event_seq = emit_end;

        if completed && end_event_seq.is_some_and(|end| return end > available) {
            return Err(miette::miette!(
                "event end cursor exceeds completed event count {available}"
            ));
        }

        if end_event_seq.is_some_and(|end| return next_event_seq >= end)
            || (completed && next_event_seq >= available)
        {
            return Ok(ExitCode::SUCCESS);
        }

        thread::sleep(Duration::from_millis(20));
    }
}
