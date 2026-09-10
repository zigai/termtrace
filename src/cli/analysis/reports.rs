use crate::cli::output::ResponseWriter;
use crate::cli::{
    analysis::args::{
        DiagnoseArgs, DiagnoseFacadeArgs, DiffArgs, DiffFormat, ExplainArgs, ExplainFormat,
        InspectFacadeArgs, TimelineArgs,
    },
    output::{
        id_list, optional_display, paginate, quoted_text, stable_enum_label, write_json_line,
        write_stdout,
    },
    session_helpers::{
        resolve_session, resolve_session_path, resolve_state_point, write_session_lifecycle,
    },
};
use miette::{IntoDiagnostic as _, Result};
use std::{
    fmt::Write as _,
    io,
    mem::take,
    path::{Path, PathBuf},
    process::ExitCode,
};
use termtrace_engine::{
    analysis::{
        diagnostics::{diagnose_session, explain_session},
        last_action::analyze_last_action_session,
    },
    diff::diff_session_states_with_semantic_states,
    timeline::{
        build_session_timeline_page, resolve_session_source_selection,
        visit_session_timeline_while, SourceRange,
    },
    Error as EngineError,
};
use termtrace_media::native_capture::validated_visual_manifests;
use termtrace_model::{
    execution::event::Event,
    registry::RegistryLifecycle,
    terminal::analysis::{DiagnoseReport, ExplainOutputPage, ExplainReport, LastActionAnalysis},
    terminal::diff::SemanticDiff,
    terminal::observation::StatePoint,
    terminal::timeline::{TimelineEntry, TimelineItem},
    value::pagination::OutputPage,
};
use termtrace_render::{render_diff_png, render_diff_svg};

pub(in crate::cli) fn run_timeline(
    response: &mut ResponseWriter,
    arguments: TimelineArgs,
) -> Result<ExitCode> {
    let session = resolve_session_path(arguments.session.clone())?;
    let range = resolve_session_source_selection(
        &session,
        arguments.range.start_monotonic_ns,
        arguments.range.end_monotonic_ns,
        arguments.range.from_marker.as_deref(),
        arguments.range.to_marker.as_deref(),
    )
    .into_diagnostic()?;

    if arguments.page.all {
        return run_all_timeline(response, &session, &arguments, range);
    }

    let (entries, page, partial) = {
        let limit = u16::try_from(arguments.page.limit)
            .map_err(|_| return miette::miette!("timeline page limit exceeds 65535"))?;

        let timeline_page = build_session_timeline_page(
            &session,
            arguments.all_states,
            arguments.controls,
            range,
            u64::try_from(arguments.page.offset).unwrap_or(u64::MAX),
            limit,
        )
        .into_diagnostic()?;

        let next_offset = timeline_page
            .next_offset
            .and_then(|offset| return usize::try_from(offset).ok());
        let truncated = next_offset.is_some();
        let returned = timeline_page.entries.len();
        let total = (!truncated && (arguments.page.offset == 0 || returned > 0))
            .then(|| return arguments.page.offset.saturating_add(returned));

        let page = OutputPage {
            offset: arguments.page.offset,
            limit: Some(arguments.page.limit),
            total,
            returned,
            truncated,
            next_offset,
        };

        (
            timeline_page.entries,
            page,
            arguments.page.offset != 0 || truncated,
        )
    };

    if arguments.jsonl {
        for entry in entries {
            write_json_line(response, &entry)?;
        }

        if partial {
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

    return write_stdout(response, output);
}

fn run_all_timeline(
    response: &mut ResponseWriter,
    session: &Path,
    arguments: &TimelineArgs,
    range: Option<SourceRange>,
) -> Result<ExitCode> {
    let mut total = 0_usize;

    visit_session_timeline_while(
        session,
        arguments.all_states,
        arguments.controls,
        range,
        |entry| {
            if total >= arguments.page.offset {
                if arguments.jsonl {
                    write_json_line(response, &entry).map_err(|source| {
                        return EngineError::Filesystem {
                            path: PathBuf::from("<timeline-output>"),
                            source: io::Error::other(source.to_string()),
                        };
                    })?;
                } else {
                    write_stdout(
                        response,
                        format!(
                            "{}\n",
                            timeline_text(&entry).map_err(|source| {
                                return EngineError::Filesystem {
                                    path: PathBuf::from("<timeline-output>"),
                                    source: io::Error::other(source.to_string()),
                                };
                            })?
                        ),
                    )
                    .map_err(|source| {
                        return EngineError::Filesystem {
                            path: PathBuf::from("<timeline-output>"),
                            source: io::Error::other(source.to_string()),
                        };
                    })?;
                }
            }

            total = total.saturating_add(1);
            return Ok(true);
        },
    )
    .into_diagnostic()?;

    let offset = arguments.page.offset.min(total);
    if offset != 0 {
        let page = OutputPage {
            offset,
            limit: None,
            total: Some(total),
            returned: total.saturating_sub(offset),
            truncated: false,
            next_offset: None,
        };

        if arguments.jsonl {
            write_json_line(
                response,
                &serde_json::json!({ "kind": "output_page", "page": page }),
            )?;
        } else {
            write_stdout(
                response,
                format!(
                    "page: {}\n",
                    serde_json::to_string(&page).into_diagnostic()?
                ),
            )?;
        }
    }

    return Ok(ExitCode::SUCCESS);
}

pub(in crate::cli) fn run_diff(
    response: &mut ResponseWriter,
    arguments: DiffArgs,
) -> Result<ExitCode> {
    let session = resolve_session_path(arguments.session)?;
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

    let (difference, before_state, after_state) =
        diff_session_states_with_semantic_states(&session, from, to).into_diagnostic()?;
    let changed = difference.changed;

    let _output_status = match arguments.format {
        DiffFormat::Text => write_stdout(response, diff_text(&difference)?)?,
        DiffFormat::Json => write_stdout(
            response,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&difference).into_diagnostic()?
            ),
        )?,
        DiffFormat::JsonCompact => write_json_line(response, &difference)?,
        DiffFormat::Svg => write_stdout(
            response,
            render_diff_svg(&before_state, &after_state, &difference).into_diagnostic()?,
        )?,
        DiffFormat::Png => write_stdout(
            response,
            render_diff_png(&before_state, &after_state, &difference).into_diagnostic()?,
        )?,
    };

    if arguments.expect_no_change && changed {
        return Ok(ExitCode::FAILURE);
    }
    return Ok(ExitCode::SUCCESS);
}

pub(in crate::cli) fn run_explain(
    response: &mut ResponseWriter,
    arguments: ExplainArgs,
) -> Result<ExitCode> {
    let session = resolve_session_path(arguments.session)?;
    let target = diff_point(
        arguments.state,
        arguments.state_hash,
        arguments.event_seq,
        arguments.at,
        arguments.marker,
    )?;

    let mut report = explain_session(&session, target, arguments.context).into_diagnostic()?;
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

            write_stdout(response, output)
        }
        ExplainFormat::Json | ExplainFormat::JsonCompact => {
            if matches!(arguments.format, ExplainFormat::Json) {
                write_stdout(
                    response,
                    format!(
                        "{}\n",
                        serde_json::to_string_pretty(&report).into_diagnostic()?
                    ),
                )
            } else {
                write_json_line(response, &report)
            }
        }
    };
}

pub(in crate::cli) fn run_diagnose(
    response: &mut ResponseWriter,
    arguments: DiagnoseArgs,
) -> Result<ExitCode> {
    let session = resolve_session_path(arguments.session)?;
    let visual_evidence = validated_visual_manifests(&session).into_diagnostic()?;
    let mut report = diagnose_session(
        &session,
        arguments.check,
        arguments.broad_change_percent,
        &visual_evidence,
    )
    .into_diagnostic()?;

    let has_findings = !report.findings.is_empty();
    let (findings, page, partial) = paginate(take(&mut report.findings), &arguments.page);
    report.findings = findings;
    report.output_page = partial.then_some(page.clone());

    if arguments.json {
        write_json_line(response, &report)?;
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

        write_stdout(response, output)?;
    }

    if arguments.fail_on_finding && has_findings {
        return Ok(ExitCode::FAILURE);
    }
    return Ok(ExitCode::SUCCESS);
}

fn run_last_action_facade<T: serde::Serialize>(
    response: &mut ResponseWriter,
    session: PathBuf,
    broad_change_percent: u8,
    operation: &'static str,
    select: impl FnOnce(&LastActionAnalysis) -> &T,
) -> Result<ExitCode> {
    let resolved = resolve_session(session)?;
    if resolved.lifecycle != RegistryLifecycle::Completed {
        return write_session_lifecycle(response, &resolved, operation, ExitCode::FAILURE);
    }

    let analysis =
        analyze_last_action_session(&resolved.path, broad_change_percent).into_diagnostic()?;

    return write_json_line(response, select(&analysis));
}

pub(in crate::cli) fn run_inspect_facade(
    response: &mut ResponseWriter,
    arguments: InspectFacadeArgs,
) -> Result<ExitCode> {
    return run_last_action_facade(
        response,
        arguments.session,
        arguments.broad_change_percent,
        "inspect_last_action",
        |analysis| return &analysis.inspection,
    );
}

pub(in crate::cli) fn run_diagnose_facade(
    response: &mut ResponseWriter,
    arguments: DiagnoseFacadeArgs,
) -> Result<ExitCode> {
    return run_last_action_facade(
        response,
        arguments.session,
        arguments.broad_change_percent,
        "diagnose_since_last_action",
        |analysis| return &analysis.diagnostics,
    );
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
    return resolve_state_point(
        state,
        state_hash,
        event_seq,
        monotonic_ns,
        marker,
        None,
        "diff endpoint",
    );
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
            semantic_input,
            caller_intent,
            ..
        } => {
            let semantic = match semantic_input {
                Some(value) => format!(" semantic_input={}", quoted_text(value)?),
                None => String::new(),
            };
            let intent = match caller_intent {
                Some(value) => format!(" caller_intent={}", quoted_text(value)?),
                None => String::new(),
            };

            format!(
                "input {}/{} bytes={}{}{}",
                stable_enum_label(source)?,
                stable_enum_label(input_kind)?,
                data.as_bytes().len(),
                semantic,
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
