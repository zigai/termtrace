use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    slice,
    sync::atomic::AtomicBool,
};

use rusqlite::{params, Connection, OptionalExtension as _, Statement};
use tempfile::TempDir;
use termtrace_evidence::{
    event_log::{EventLogCursor, EventLogSummary},
    session::{validate_completed_session, SessionSnapshot},
};
use termtrace_model::{
    execution::event::Event,
    terminal::{
        analysis::{
            DiagnoseReport, DiagnosticCheck, DiagnosticFinding, FindingConfidence,
            NativeCorrelation, NativeEvidenceLevel,
        },
        replay::StateRecord,
        timeline::TimelineItem,
    },
};

use super::{
    analysis_error, detect_cursor_jumps, detect_dirty_discrepancies, detect_redundant_full_redraw,
    detect_transients, native_correlations_for_state, ValidatedVisualManifest,
};
use crate::{
    replay::{trust::SourceIdentity, visit_recording_states_range_while},
    timeline::stream::TokenCursor,
    Result,
};

/// Diagnose a completed source with a disk-backed intermediate index.
///
/// Replay history and VT tokens are not retained in memory. The returned findings
/// and their evidence references grow with the requested report itself.
///
/// # Errors
/// Rejects incomplete or changing sources and replay, framing, or temporary storage errors.
pub fn diagnose_session(
    session: &Path,
    checks: Vec<DiagnosticCheck>,
    broad_change_percent: u8,
    visual_evidence: &[ValidatedVisualManifest],
) -> Result<DiagnoseReport> {
    validate_completed_session(session)?;

    return diagnose_recording(session, true, checks, broad_change_percent, visual_evidence);
}

/// Diagnose a verified finite prefix; the report hash and count identify only that prefix.
///
/// # Errors
/// Rejects modified snapshots, malformed evidence, replay and temporary-index failures.
pub fn diagnose_snapshot(
    snapshot: &SessionSnapshot,
    checks: Vec<DiagnosticCheck>,
    broad_change_percent: u8,
) -> Result<DiagnoseReport> {
    return diagnose_recording(snapshot.path(), false, checks, broad_change_percent, &[]);
}

fn diagnose_recording(
    session: &Path,
    completed: bool,
    mut checks: Vec<DiagnosticCheck>,
    broad_change_percent: u8,
    visual_evidence: &[ValidatedVisualManifest],
) -> Result<DiagnoseReport> {
    let source = SourceIdentity::capture_candidate(session)?;
    let summary = EventLogSummary::read(session.join("events.jsonl"))?;

    checks.sort_unstable();
    checks.dedup();

    if checks.is_empty() {
        checks = DiagnosticCheck::all();
    }

    let store = DiagnosticStore::new()?;
    let mut findings = Vec::new();

    store.connection.execute_batch("BEGIN").map_err(sql_error)?;

    let mut presented = VecDeque::<StateRecord>::new();
    let mut previous_cursor = None;
    let mut hidden = Vec::new();
    let mut modes = Vec::new();
    let mut palette = Vec::new();
    let mut state_insert = store
        .connection
        .prepare("INSERT INTO states VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")
        .map_err(sql_error)?;

    visit_recording_states_range_while(
        session,
        completed,
        None,
        &AtomicBool::new(false),
        |record, _| {
            DiagnosticStore::insert_state(&mut state_insert, record)?;

            if checks.contains(&DiagnosticCheck::Dirty) {
                detect_dirty_discrepancies(slice::from_ref(record), &mut findings);
            }

            if checks.contains(&DiagnosticCheck::Cursor) && record.delta.cursor.is_some() {
                if let Some(previous) = previous_cursor.take() {
                    detect_cursor_jumps(&[previous, record.clone()], &mut findings);
                }

                previous_cursor = Some(record.clone());
            }

            if checks.contains(&DiagnosticCheck::Transient)
                && !record.presented
                && !record.delta.rows.is_empty()
            {
                hidden.push(record.state_id);
            }

            if checks.contains(&DiagnosticCheck::Modes) && record.delta.modes.is_some() {
                modes.push(record.state_id);
            }

            if checks.contains(&DiagnosticCheck::Palette) && record.delta.colors.is_some() {
                palette.push(record.state_id);
            }

            if record.presented {
                presented.push_back(record.clone());
                let recent = presented.make_contiguous();
                if checks.contains(&DiagnosticCheck::FullRedraw) {
                    let mut candidates = Vec::new();
                    detect_redundant_full_redraw(recent, &mut candidates);
                    findings.extend(candidates.into_iter().filter(|finding| {
                        return finding.state_ids.last() == Some(&record.state_id);
                    }));
                }

                if checks.contains(&DiagnosticCheck::Transient)
                    && recent.len() >= 3
                    && let Some(last_three) = recent.get(recent.len().saturating_sub(3)..)
                {
                    detect_transients(last_three, &mut findings);
                }

                if presented.len() > 8 {
                    presented.pop_front();
                }
            }

            return Ok(true);
        },
    )?;

    drop(state_insert);
    if !hidden.is_empty() {
        let count = hidden.len();
        findings.push(finding(
            "hidden_transient",
            FindingConfidence::High,
            hidden,
            Vec::new(),
            vec![format!(
                "{count} cell-changing states were not presentation eligible"
            )],
        ));
    }

    for (code, ids) in [("mode_churn", modes), ("palette_churn", palette)] {
        if ids.len() >= 3 {
            let count = ids.len();
            findings.push(finding(
                code,
                FindingConfidence::Medium,
                ids,
                Vec::new(),
                vec![format!("{count} related semantic mutations occurred")],
            ));
        }
    }

    let mut canonical = EventLogCursor::open(session.join("events.jsonl"))?;
    let initial_cols = canonical.header().cols;
    let initial_rows = canonical.header().rows;

    if checks.contains(&DiagnosticCheck::QueryLoop) {
        while let Some(record) = canonical.next_record()? {
            if let Event::QueryResponse {
                query, response, ..
            } = record.event
            {
                store.connection.execute("INSERT INTO queries VALUES (?1, ?2, 1) ON CONFLICT(query, response) DO UPDATE SET count=count+1", params![query.as_bytes(), response.as_bytes()]).map_err(sql_error)?;
            }
        }

        let mut statement = store
            .connection
            .prepare("SELECT count FROM queries WHERE count >= 3 ORDER BY query, response")
            .map_err(sql_error)?;
        let counts = statement
            .query_map([], |row| return row.get::<_, u64>(0))
            .map_err(sql_error)?;

        for count in counts {
            let count = count.map_err(sql_error)?;
            findings.push(finding(
                "repeated_query_response_loop",
                FindingConfidence::Medium,
                Vec::new(),
                Vec::new(),
                vec![format!(
                    "the same terminal query and host response occurred {count} times"
                )],
            ));
        }
    }

    if checks.contains(&DiagnosticCheck::FullRedraw) {
        let mut tokens = TokenCursor::new(session, None)?;
        while let Some(entry) = tokens.next()? {
            if let TimelineItem::VtToken { token } = entry.item {
                let kind = match token.description.as_str() {
                    "erase display (all)" => 1,
                    "printable bytes" => 2,
                    "erase saved lines" => 3,
                    _ => continue,
                };

                let event = token.source.first().map(|segment| return segment.event_seq);
                let offset = token
                    .source
                    .iter()
                    .map(|segment| return segment.start_byte_offset)
                    .min();

                store
                    .connection
                    .execute(
                        "INSERT INTO tokens VALUES (?1, ?2, ?3, ?4)",
                        params![token.token_id, kind, event, offset],
                    )
                    .map_err(sql_error)?;
            }
        }

        store.full_redraw(
            initial_cols,
            initial_rows,
            broad_change_percent,
            &mut findings,
        )?;
    }

    store
        .connection
        .execute_batch("COMMIT")
        .map_err(sql_error)?;

    for finding in &mut findings {
        for state_id in &finding.state_ids {
            let correlations = native_correlations_for_state(
                summary.header.session_id,
                *state_id,
                visual_evidence,
            );

            if !correlations.is_empty() {
                finding.native_correlations.extend(correlations);
            } else if store
                .connection
                .query_row(
                    "SELECT presented FROM states WHERE id=?1",
                    [state_id],
                    |row| return row.get::<_, bool>(0),
                )
                .optional()
                .map_err(sql_error)?
                .unwrap_or(false)
            {
                finding.native_correlations.push(NativeCorrelation {
                    level: NativeEvidenceLevel::PresentationEligible,
                    manifest: PathBuf::new(),
                    state_id: *state_id,
                    frame: None,
                    limitation: "semantic presentation eligibility is not native-frame observation"
                        .to_owned(),
                });
            }
        }
    }

    findings.sort_by(|left, right| {
        return left
            .state_ids
            .first()
            .cmp(&right.state_ids.first())
            .then_with(|| return left.code.cmp(&right.code));
    });

    if source != SourceIdentity::capture_candidate(session)? {
        return Err(analysis_error("canonical source changed during diagnosis"));
    }

    return Ok(DiagnoseReport {
        format: "termtrace-diagnose".to_owned(),
        version: 1,
        session_id: summary.header.session_id,
        source_final_event_hash: summary.terminal.map_or_else(
            || return summary.header.session_manifest_hash.clone(),
            |record| return record.hash,
        ),
        source_event_count: summary.event_count,
        checks,
        broad_change_fraction: format!("{broad_change_percent}/100"),
        findings,
        output_page: None,
    });
}

struct DiagnosticStore {
    connection: Connection,
    _directory: TempDir,
}

impl DiagnosticStore {
    fn new() -> Result<Self> {
        let directory = tempfile::tempdir().map_err(|error| {
            return analysis_error(format!("create diagnostic temporary directory: {error}"));
        })?;
        let connection =
            Connection::open(directory.path().join("diagnostics.sqlite3")).map_err(sql_error)?;
        connection.execute_batch("PRAGMA cache_size=-8192; PRAGMA temp_store=FILE;
            CREATE TABLE states (id INTEGER PRIMARY KEY, event INTEGER, offset INTEGER, presented INTEGER NOT NULL, changed INTEGER NOT NULL, cols INTEGER, rows INTEGER);
            CREATE INDEX states_event ON states(event, id);
            CREATE TABLE tokens (id INTEGER PRIMARY KEY, kind INTEGER NOT NULL, event INTEGER, offset INTEGER);
            CREATE INDEX tokens_kind ON tokens(kind, id);
            CREATE TABLE queries (query BLOB NOT NULL, response BLOB NOT NULL, count INTEGER NOT NULL, PRIMARY KEY(query,response)) WITHOUT ROWID;").map_err(sql_error)?;

        return Ok(Self {
            connection,
            _directory: directory,
        });
    }

    fn insert_state(statement: &mut Statement<'_>, state: &StateRecord) -> Result<()> {
        let changed = state.delta.rows.iter().fold(0_u64, |count, row| {
            return count.saturating_add(u64::try_from(row.cells.len()).unwrap_or(u64::MAX));
        });

        let cols = state.delta.geometry.map(|geometry| return geometry.0);
        let rows = state.delta.geometry.map(|geometry| return geometry.1);
        statement
            .execute(params![
                state.state_id,
                state.event_seq,
                state.event_byte_offset,
                state.presented,
                changed,
                cols,
                rows
            ])
            .map_err(sql_error)?;

        return Ok(());
    }

    fn full_redraw(
        &self,
        initial_cols: u16,
        initial_rows: u16,
        percent: u8,
        findings: &mut Vec<DiagnosticFinding>,
    ) -> Result<()> {
        let mut statement = self
            .connection
            .prepare("SELECT id, event, offset FROM tokens WHERE kind=1 ORDER BY id")
            .map_err(sql_error)?;
        let mut clears = statement.query([]).map_err(sql_error)?;

        while let Some(clear) = clears.next().map_err(sql_error)? {
            let token_id: u64 = clear.get(0).map_err(sql_error)?;
            let event: Option<u64> = clear.get(1).map_err(sql_error)?;
            let offset: Option<u64> = clear.get(2).map_err(sql_error)?;
            let state_id: Option<u64> = self
                .connection
                .query_row(
                    "SELECT id FROM states WHERE event=?1 AND offset>=?2 ORDER BY id LIMIT 1",
                    params![event, offset],
                    |row| return row.get(0),
                )
                .optional()
                .map_err(sql_error)?;
            findings.push(finding(
                "full_viewport_clear",
                FindingConfidence::High,
                state_id.into_iter().collect(),
                vec![token_id],
                vec![
                    "ED 2 occurred in canonical output".to_owned(),
                    "semantic evidence does not by itself prove a visible pixel clear".to_owned(),
                ],
            ));

            let Some(event) = event else {
                continue;
            };
            let mut cols = initial_cols;
            let mut rows = initial_rows;
            let mut changed = 0_u64;
            let mut ids = Vec::new();
            let mut repaint_statement = self.connection.prepare("SELECT id, changed, cols, rows FROM states WHERE event>=?1 AND event<=?2 ORDER BY id").map_err(sql_error)?;
            let mut repaint = repaint_statement
                .query(params![event, event.saturating_add(2)])
                .map_err(sql_error)?;

            while let Some(state) = repaint.next().map_err(sql_error)? {
                if let (Some(new_cols), Some(new_rows)) = (
                    state.get::<_, Option<u16>>(2).map_err(sql_error)?,
                    state.get::<_, Option<u16>>(3).map_err(sql_error)?,
                ) {
                    cols = new_cols;
                    rows = new_rows;
                }

                let candidate: u64 = state.get(1).map_err(sql_error)?;

                if candidate > 0 {
                    changed = changed.saturating_add(candidate);
                    ids.push(state.get(0).map_err(sql_error)?);
                }
            }

            let cells = u64::from(cols).saturating_mul(u64::from(rows)).max(1);

            if changed.saturating_mul(100) >= cells.saturating_mul(u64::from(percent)) {
                let mut printable = self
                    .connection
                    .prepare("SELECT id FROM tokens WHERE kind=2 AND id>=?1 ORDER BY id LIMIT 32")
                    .map_err(sql_error)?;
                let mut token_ids = printable
                    .query_map([token_id], |row| return row.get::<_, u64>(0))
                    .map_err(sql_error)?
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .map_err(sql_error)?;
                token_ids.push(token_id);
                findings.push(finding(
                    "clear_then_repaint",
                    FindingConfidence::High,
                    ids,
                    token_ids,
                    vec![
                        format!("at least {changed} cell updates followed ED 2 within two events"),
                        format!("broad-change threshold was {percent}% of {cells} cells"),
                        "ordered semantic repaint evidence is not a pixel-level flicker claim"
                            .to_owned(),
                    ],
                ));
            }
        }

        let mut scrollback = self
            .connection
            .prepare("SELECT id FROM tokens WHERE kind=3 ORDER BY id")
            .map_err(sql_error)?;
        let ids = scrollback
            .query_map([], |row| return row.get::<_, u64>(0))
            .map_err(sql_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sql_error)?;
        if !ids.is_empty() {
            findings.push(finding(
                "clear_scrollback",
                FindingConfidence::High,
                Vec::new(),
                ids,
                vec!["ED 3 occurred in canonical output".to_owned()],
            ));
        }

        return Ok(());
    }
}

fn finding(
    code: &str,
    confidence: FindingConfidence,
    state_ids: Vec<u64>,
    token_ids: Vec<u64>,
    facts: Vec<String>,
) -> DiagnosticFinding {
    return DiagnosticFinding {
        code: code.to_owned(),
        confidence,
        state_ids,
        token_ids,
        facts,
        native_correlations: Vec::new(),
    };
}

fn sql_error(error: rusqlite::Error) -> crate::Error {
    return analysis_error(format!("diagnostic temporary index: {error}"));
}
