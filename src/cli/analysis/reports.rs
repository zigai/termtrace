use super::*;

pub(in crate::cli) fn run_timeline(arguments: TimelineArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(&session, &log)?;
    let range = resolve_source_selection(
        &log,
        arguments.range.start_monotonic_ns,
        arguments.range.end_monotonic_ns,
        arguments.range.from_marker.as_deref(),
        arguments.range.to_marker.as_deref(),
    )?;
    let entries = build_timeline(&log, arguments.all_states, arguments.controls, range)?;
    let (entries, page, partial) = paginate(entries, &arguments.page);
    if arguments.jsonl {
        for entry in entries {
            write_json_line(&entry)?;
        }
        if partial {
            write_json_line(&serde_json::json!({
                "kind": "output_page",
                "page": page,
            }))?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    let mut output = String::new();
    for entry in entries {
        writeln!(output, "{}", timeline_text(&entry)?).into_diagnostic()?;
    }
    if partial {
        writeln!(
            output,
            "page: {}",
            serde_json::to_string(&page).into_diagnostic()?
        )
        .into_diagnostic()?;
    }
    return write_stdout(output);
}

pub(in crate::cli) fn run_diff(arguments: DiffArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(&session, &log)?;
    let from = diff_point(
        arguments.from_state,
        arguments.from_state_hash,
        arguments.from_event_seq,
        arguments.from_at,
        arguments.from_marker,
    )?;
    let to = diff_point(
        arguments.to_state,
        arguments.to_state_hash,
        arguments.to_event_seq,
        arguments.to_at,
        arguments.to_marker,
    )?;
    let (difference, before_state, after_state) = diff_states_with_semantic_states(&log, from, to)?;
    let image_states = matches!(arguments.format, DiffFormat::Svg | DiffFormat::Png)
        .then_some((before_state, after_state));
    let changed = difference.changed;
    let _output_status = match arguments.format {
        DiffFormat::Text => write_stdout(diff_text(&difference)?)?,
        DiffFormat::Json => write_stdout(format!(
            "{}\n",
            serde_json::to_string_pretty(&difference).into_diagnostic()?
        ))?,
        DiffFormat::JsonCompact => write_json_line(&difference)?,
        DiffFormat::Svg => {
            let (before, after) = image_states
                .as_ref()
                .ok_or_else(|| return miette::miette!("missing visual diff states"))?;
            write_stdout(render_diff_svg(before, after, &difference))?
        }
        DiffFormat::Png => {
            let (before, after) = image_states
                .as_ref()
                .ok_or_else(|| return miette::miette!("missing visual diff states"))?;
            write_stdout(render_diff_png(before, after, &difference)?)?
        }
    };
    if arguments.expect_no_change && changed {
        return Ok(ExitCode::FAILURE);
    }
    return Ok(ExitCode::SUCCESS);
}

pub(in crate::cli) fn run_explain(arguments: ExplainArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(&session, &log)?;
    let target = diff_point(
        arguments.state,
        arguments.state_hash,
        arguments.event_seq,
        arguments.at,
        arguments.marker,
    )?;
    let mut report = explain(&log, target, arguments.context)?;
    let (tokens, tokens_page, tokens_partial) = paginate(take(&mut report.tokens), &arguments.page);
    let (states, states_page, states_partial) = paginate(take(&mut report.states), &arguments.page);
    let (findings, findings_page, findings_partial) =
        paginate(take(&mut report.findings), &arguments.page);
    report.tokens = tokens;
    report.states = states;
    report.findings = findings;
    let partial = tokens_partial || states_partial || findings_partial;
    let page = ExplainOutputPage {
        tokens: tokens_page,
        states: states_page,
        findings: findings_page,
    };
    report.output_page = partial.then_some(page.clone());
    return match arguments.format {
        ExplainFormat::Text => {
            let mut output = explain_text(&report)?;
            if partial {
                writeln!(
                    output,
                    "page: {}",
                    serde_json::to_string(&page).into_diagnostic()?
                )
                .into_diagnostic()?;
            }
            write_stdout(output)
        }
        ExplainFormat::Json | ExplainFormat::JsonCompact => {
            if matches!(arguments.format, ExplainFormat::Json) {
                write_stdout(format!(
                    "{}\n",
                    serde_json::to_string_pretty(&report).into_diagnostic()?
                ))
            } else {
                write_json_line(&report)
            }
        }
    };
}

pub(in crate::cli) fn run_diagnose(arguments: DiagnoseArgs) -> Result<ExitCode> {
    let session = resolve_session_ref(arguments.session)?;
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(&session, &log)?;
    let mut report = diagnose(
        &session,
        &log,
        arguments.check,
        arguments.broad_change_percent,
    )?;
    let has_findings = !report.findings.is_empty();
    let (findings, page, partial) = paginate(take(&mut report.findings), &arguments.page);
    report.findings = findings;
    report.output_page = partial.then_some(page.clone());
    if arguments.json {
        write_json_line(&report)?;
    } else {
        let mut output = diagnose_text(&report)?;
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
    if arguments.fail_on_finding && has_findings {
        return Ok(ExitCode::FAILURE);
    }
    return Ok(ExitCode::SUCCESS);
}

fn explain_text(report: &ExplainReport) -> Result<String> {
    let mut output = format!(
        "state {} event={} context={} tokens={} states={}\n",
        report.target_state.state_id,
        optional_display(report.target_state.event_seq),
        report.context_events,
        report.tokens.len(),
        report.states.len(),
    );
    for token in &report.tokens {
        let source = token.source.first().map_or_else(
            || return "event - bytes -".to_owned(),
            |segment| {
                return format!(
                    "event {} bytes {}..{}",
                    segment.event_seq, segment.start_byte_offset, segment.end_byte_offset
                );
            },
        );
        writeln!(
            output,
            "  token {} {source} {}: {}",
            token.token_id,
            stable_enum_label(&token.kind)?,
            token.description
        )
        .into_diagnostic()?;
    }
    for finding in &report.findings {
        writeln!(
            output,
            "finding: {} [{}] {}",
            finding.code,
            stable_enum_label(&finding.confidence)?,
            finding.facts.join("; ")
        )
        .into_diagnostic()?;
    }
    return Ok(output);
}

fn diagnose_text(report: &DiagnoseReport) -> Result<String> {
    let mut output = format!(
        "session {} checks={} findings={} source={}\n",
        report.session_id,
        report.checks.len(),
        report.findings.len(),
        report.source_final_event_hash,
    );
    for finding in &report.findings {
        writeln!(
            output,
            "finding: {} [{}] states={} tokens={}",
            finding.code,
            stable_enum_label(&finding.confidence)?,
            id_list(&finding.state_ids),
            id_list(&finding.token_ids),
        )
        .into_diagnostic()?;
        for fact in &finding.facts {
            writeln!(output, "  fact: {fact}").into_diagnostic()?;
        }
        for correlation in &finding.native_correlations {
            writeln!(
                output,
                "  native: {} state={} frame={} {}",
                stable_enum_label(&correlation.level)?,
                correlation.state_id,
                optional_display(correlation.frame),
                correlation.limitation,
            )
            .into_diagnostic()?;
        }
    }
    return Ok(output);
}

fn diff_point(
    state: Option<u64>,
    state_hash: Option<String>,
    event_seq: Option<u64>,
    monotonic_ns: Option<u64>,
    marker: Option<String>,
) -> Result<StatePoint> {
    return match (state, state_hash, event_seq, monotonic_ns, marker) {
        (Some(state_id), None, None, None, None) => Ok(StatePoint::StateId { state_id }),
        (None, Some(state_hash), None, None, None) => Ok(StatePoint::StateHash { state_hash }),
        (None, None, Some(event_seq), None, None) => Ok(StatePoint::EventSeq { event_seq }),
        (None, None, None, Some(monotonic_ns), None) => {
            Ok(StatePoint::MonotonicNs { monotonic_ns })
        }
        (None, None, None, None, Some(label)) => Ok(StatePoint::Marker { label }),
        _ => Err(miette::miette!(
            "exactly one diff endpoint selector is required"
        )),
    };
}

fn diff_text(difference: &SemanticDiff) -> Result<String> {
    let changed_cells = difference.rows.iter().fold(0_usize, |count, row| {
        return count.saturating_add(row.cells.len());
    });
    let mut output = format!(
        "states {} -> {}  changed={}  rows={}  cells={}\n",
        difference.from.state_id,
        difference.to.state_id,
        difference.changed,
        difference.rows.len(),
        changed_cells,
    );
    for row in &difference.rows {
        for cell in &row.cells {
            let before = cell
                .before
                .as_ref()
                .map_or("∅", |value| return value.grapheme.as_str());
            let after = cell
                .after
                .as_ref()
                .map_or("∅", |value| return value.grapheme.as_str());
            writeln!(
                output,
                "  ({},{}) {} -> {}",
                cell.column,
                row.row,
                quoted_text(before)?,
                quoted_text(after)?,
            )
            .into_diagnostic()?;
        }
    }
    return Ok(output);
}

fn timeline_text(entry: &TimelineEntry) -> Result<String> {
    let seconds = entry.monotonic_ns / 1_000_000_000;
    let fractional = entry.monotonic_ns % 1_000_000_000;
    let event = entry
        .event_seq
        .map_or_else(|| return "-".to_owned(), |seq| return seq.to_string());
    let detail = match &entry.item {
        TimelineItem::SemanticState {
            state_id,
            event_byte_offset,
            presented,
            changed_rows,
            changed_cells,
            cursor_changed,
            modes_changed,
            metadata_changed,
            ..
        } => format!(
            "state {state_id} byte {} presented={} rows={} cells={} cursor={} modes={} metadata={}",
            event_byte_offset
                .map_or_else(|| return "-".to_owned(), |value| return value.to_string()),
            if *presented { "yes" } else { "no" },
            changed_rows.len(),
            changed_cells,
            cursor_changed,
            modes_changed,
            metadata_changed,
        ),
        TimelineItem::CanonicalEvent { event } => canonical_event_text(event)?,
        TimelineItem::VtToken { token } => {
            let location = token.source.first().map_or_else(
                || return "-".to_owned(),
                |segment| {
                    return format!("{}..{}", segment.start_byte_offset, segment.end_byte_offset);
                },
            );
            format!(
                "token {} {} bytes={} range={} {}",
                token.token_id,
                stable_enum_label(&token.kind)?,
                token.raw.as_bytes().len(),
                location,
                token.description,
            )
        }
    };
    return Ok(format!(
        "{seconds}.{fractional:09}s  event {event}  {detail}"
    ));
}

fn canonical_event_text(event: &Event) -> Result<String> {
    let text = match event {
        Event::InputIntent {
            source,
            input_kind,
            data,
            intent,
            ..
        } => {
            let intent = match intent {
                Some(value) => format!(" intent={}", quoted_text(value)?),
                None => String::new(),
            };
            format!(
                "input {}/{} bytes={}{}",
                stable_enum_label(source)?,
                stable_enum_label(input_kind)?,
                data.as_bytes().len(),
                intent,
            )
        }
        Event::ResizeIntent { cols, rows, .. } => {
            format!("resize {cols}x{rows}")
        }
        Event::SignalIntent { name, .. } => format!("signal {name}"),
        Event::Marker { label, .. } => format!("marker {}", quoted_text(label)?),
        Event::QueryResponse {
            query, responder, ..
        } => format!(
            "query bytes={} responder={responder}",
            query.as_bytes().len()
        ),
        Event::Lifecycle {
            lifecycle_kind,
            pid,
        } => format!(
            "lifecycle {} pid={}",
            stable_enum_label(lifecycle_kind)?,
            optional_display(*pid),
        ),
        Event::Exit {
            code, signal_name, ..
        } => format!(
            "exit code={} signal={}",
            optional_display(*code),
            optional_display(signal_name.as_deref()),
        ),
        Event::HostError { code, message } => {
            format!("host_error {code}: {message}")
        }
        _ => "canonical operation result".to_owned(),
    };
    return Ok(text);
}
