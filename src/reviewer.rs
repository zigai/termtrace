//! Read-only semantic forensic reviewer with safe terminal rendering.

use std::{
    collections::BTreeMap,
    fs::Permissions,
    io::{self, Stdout},
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event as TerminalEvent, KeyCode,
        KeyEventKind, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    flag::register as register_signal,
};
use uuid::Uuid;

use crate::{
    Error, Result,
    analysis::{NativeCorrelation, native_correlations},
    artifact::write_secure,
    event::Event,
    event_log::EventLog,
    inspection::state_text,
    replay::{CollectedReplay, StateInspection, StateRecord, StateSelector, inspect, replay},
    session::validate_completed_log,
};

/// Stable scriptable reviewer command inventory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewerCapabilities {
    /// Reviewer contract version.
    pub version: u32,
    /// Interactive command identifiers.
    pub commands: Vec<String>,
    /// Safety boundary for recorded terminal content.
    pub rendering_policy: String,
}

impl Default for ReviewerCapabilities {
    fn default() -> Self {
        return Self {
            version: 1,
            commands: [
                "next_state",
                "previous_state",
                "toggle_presented_only",
                "next_marker",
                "previous_marker",
                "next_input",
                "previous_input",
                "next_search_match",
                "previous_search_match",
                "toggle_help_palette",
                "inspect_native_correlation",
                "mouse_scroll_navigation",
                "export_selected_state",
                "quit",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            rendering_policy: "semantic_cells_only_no_raw_replay".to_owned(),
        };
    }
}

/// Noninteractive reviewer source summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewSummary {
    /// Immutable completed session directory.
    pub canonical_session: PathBuf,
    /// Number of distinct semantic states.
    pub state_count: u64,
    /// Number of presentation-eligible states.
    pub presented_state_count: u64,
    /// Number of canonical markers.
    pub marker_count: u64,
    /// Number of honest direct or scheduled native correlations.
    pub native_correlation_count: u64,
    /// Final semantic state hash.
    pub final_state_hash: String,
    /// Safety boundary used for viewport rendering.
    pub rendering_policy: String,
}

/// One semantic viewport text match with a copyable canonical selector.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewSearchMatch {
    /// Exact semantic state identifier.
    pub state_id: u64,
    /// Canonical event sequence associated with the state.
    pub event_seq: Option<u64>,
    /// Whether the state was presentation-eligible.
    pub presented: bool,
    /// Exact semantic state hash.
    pub state_hash: String,
    /// Ready-to-run selector arguments.
    pub selector: String,
}

/// Stable noninteractive reviewer text-search result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewSearchResult {
    /// Immutable completed session directory.
    pub canonical_session: PathBuf,
    /// Exact viewport substring query.
    pub query: String,
    /// Ordered semantic state matches.
    pub matches: Vec<ReviewSearchMatch>,
}

/// Manifest for one explicit reviewer state export.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReviewExportManifest {
    /// Reviewer export contract version.
    pub version: u32,
    /// Immutable source session.
    pub canonical_session: PathBuf,
    /// Exact exported semantic state identifier.
    pub state_id: u64,
    /// Exact semantic state hash.
    pub state_hash: String,
    /// Canonical source head anchoring the export.
    pub source_final_event_hash: String,
    /// BLAKE3 of `state.json`.
    pub state_json_blake3: String,
    /// BLAKE3 of `screen.txt`.
    pub screen_text_blake3: String,
}

struct ReviewModel {
    session: PathBuf,
    log: EventLog,
    replay: CollectedReplay,
    selected: usize,
    presented_only: bool,
    search_query: Option<String>,
    search_matches: Vec<usize>,
    show_help: bool,
    native_correlations: BTreeMap<u64, Vec<NativeCorrelation>>,
}

impl ReviewModel {
    fn open(
        session: impl AsRef<Path>,
        around_marker: Option<&str>,
        search_query: Option<&str>,
    ) -> Result<Self> {
        let session = session.as_ref().to_path_buf();
        let log = EventLog::read(session.join("events.jsonl"))?;
        validate_completed_log(&session, &log)?;
        let replay = replay(&log)?;
        if replay.states.is_empty() {
            return Err(review_error(
                "completed session contains no semantic states",
            ));
        }
        let selected = match around_marker {
            Some(label) => marker_state_index(&log, &replay.states, label)?,
            None => replay.states.len().saturating_sub(1),
        };
        let search_matches = match search_query {
            Some(query) => search_state_indexes(&log, &replay.states, query)?,
            None => Vec::new(),
        };
        let native_correlations = native_correlations(&session, log.header.session_id)?;
        return Ok(Self {
            session,
            log,
            replay,
            selected,
            presented_only: true,
            search_query: search_query.map(str::to_owned),
            search_matches,
            show_help: false,
            native_correlations,
        });
    }

    fn summary(&self) -> ReviewSummary {
        let final_state_hash = self
            .replay
            .states
            .last()
            .map_or_else(String::new, |state| return state.state_hash.clone());
        return ReviewSummary {
            canonical_session: self.session.clone(),
            state_count: u64::try_from(self.replay.states.len()).unwrap_or(u64::MAX),
            presented_state_count: u64::try_from(
                self.replay
                    .states
                    .iter()
                    .filter(|state| return state.presented)
                    .count(),
            )
            .unwrap_or(u64::MAX),
            marker_count: u64::try_from(
                self.log
                    .events
                    .iter()
                    .filter(|record| return matches!(record.event, Event::Marker { .. }))
                    .count(),
            )
            .unwrap_or(u64::MAX),
            native_correlation_count: u64::try_from(
                self.native_correlations
                    .values()
                    .map(Vec::len)
                    .sum::<usize>(),
            )
            .unwrap_or(u64::MAX),
            final_state_hash,
            rendering_policy: "semantic_cells_only_no_raw_replay".to_owned(),
        };
    }

    fn selected_record(&self) -> Result<&StateRecord> {
        return self
            .replay
            .states
            .get(self.selected)
            .ok_or_else(|| return review_error("selected reviewer state is out of bounds"));
    }

    fn selected_inspection(&self) -> Result<StateInspection> {
        return inspect(
            &self.log,
            StateSelector::StateId(self.selected_record()?.state_id),
        );
    }

    fn move_selection(&mut self, direction: isize) {
        let mut candidate = self.selected;
        loop {
            candidate = if direction < 0 {
                candidate.saturating_sub(1)
            } else {
                candidate
                    .saturating_add(1)
                    .min(self.replay.states.len().saturating_sub(1))
            };
            let Some(state) = self.replay.states.get(candidate) else {
                return;
            };
            if !self.presented_only || state.presented {
                self.selected = candidate;
                return;
            }
            if candidate == 0 || candidate + 1 == self.replay.states.len() {
                return;
            }
        }
    }

    fn jump_event(&mut self, direction: isize, marker: bool) {
        let selected_time = self
            .replay
            .states
            .get(self.selected)
            .map_or(0, |state| return state.monotonic_ns);
        let candidates = self
            .log
            .events
            .iter()
            .filter(|record| {
                return if marker {
                    matches!(record.event, Event::Marker { .. })
                } else {
                    matches!(record.event, Event::InputIntent { .. })
                };
            })
            .map(|record| return record.monotonic_ns)
            .collect::<Vec<_>>();
        let target = if direction < 0 {
            candidates
                .iter()
                .rev()
                .find(|time| return **time < selected_time)
        } else {
            candidates.iter().find(|time| return **time > selected_time)
        };
        if let Some(time) = target
            && let Some(index) = self
                .replay
                .states
                .iter()
                .position(|state| return state.monotonic_ns >= *time)
        {
            self.selected = index;
        }
    }

    fn jump_search(&mut self, direction: isize) {
        let target = if direction < 0 {
            self.search_matches
                .iter()
                .rev()
                .find(|index| return **index < self.selected)
                .or_else(|| return self.search_matches.last())
        } else {
            self.search_matches
                .iter()
                .find(|index| return **index > self.selected)
                .or_else(|| return self.search_matches.first())
        };
        if let Some(index) = target {
            self.selected = *index;
        }
    }
}

struct TerminalModeGuard;

impl TerminalModeGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().map_err(terminal_error)?;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture).map_err(terminal_error)?;
        return Ok(Self);
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        let _leave_result = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
        let _raw_result = disable_raw_mode();
    }
}

/// Load and summarize a validated completed session without changing it.
pub fn summarize(session: impl AsRef<Path>, around_marker: Option<&str>) -> Result<ReviewSummary> {
    return Ok(ReviewModel::open(session, around_marker, None)?.summary());
}

/// Search every semantic viewport in a validated completed session.
pub fn search(session: impl AsRef<Path>, query: &str) -> Result<ReviewSearchResult> {
    if query.is_empty() || query.len() > 4096 {
        return Err(review_error(
            "review search must contain 1 through 4096 bytes",
        ));
    }
    let model = ReviewModel::open(session, None, Some(query))?;
    let matches = model
        .search_matches
        .iter()
        .filter_map(|index| return model.replay.states.get(*index))
        .map(|state| {
            return ReviewSearchMatch {
                state_id: state.state_id,
                event_seq: state.event_seq,
                presented: state.presented,
                state_hash: state.state_hash.clone(),
                selector: format!("--state {}", state.state_id),
            };
        })
        .collect();
    return Ok(ReviewSearchResult {
        canonical_session: model.session,
        query: query.to_owned(),
        matches,
    });
}

/// Atomically export one semantic state and viewport text without replacement.
pub fn export_state(
    session: impl AsRef<Path>,
    state_id: u64,
    destination: impl AsRef<Path>,
) -> Result<ReviewExportManifest> {
    let model = ReviewModel::open(session, None, None)?;
    let state = model
        .replay
        .states
        .iter()
        .find(|state| return state.state_id == state_id)
        .ok_or_else(|| return review_error(format!("review state {state_id} does not exist")))?;
    let inspection = inspect(&model.log, StateSelector::StateId(state_id))?;
    let state_json = serde_json::to_vec_pretty(&inspection).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<review-state>"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    let screen = state_text(&inspection.state);
    let destination = destination.as_ref();
    if destination.exists() {
        return Err(review_error("review export destination already exists"));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| return review_error("review export destination has no parent"))?;
    fs_err::create_dir_all(parent).map_err(|source| {
        return Error::Filesystem {
            path: parent.to_path_buf(),
            source,
        };
    })?;
    let temporary = parent.join(format!(".review-export-{}.tmp", Uuid::now_v7()));
    fs_err::create_dir(&temporary).map_err(|source| {
        return Error::Filesystem {
            path: temporary.clone(),
            source,
        };
    })?;
    fs_err::set_permissions(&temporary, Permissions::from_mode(0o700)).map_err(|source| {
        return Error::Filesystem {
            path: temporary.clone(),
            source,
        };
    })?;
    write_secure(&temporary.join("state.json"), &state_json)?;
    write_secure(&temporary.join("screen.txt"), screen.as_bytes())?;
    let manifest = ReviewExportManifest {
        version: 1,
        canonical_session: model.session,
        state_id,
        state_hash: state.state_hash.clone(),
        source_final_event_hash: model.log.events.last().map_or_else(
            || return model.log.header.session_manifest_hash.clone(),
            |record| return record.hash.clone(),
        ),
        state_json_blake3: blake3::hash(&state_json).to_hex().to_string(),
        screen_text_blake3: blake3::hash(screen.as_bytes()).to_hex().to_string(),
    };
    let mut manifest_json = serde_json::to_vec_pretty(&manifest).map_err(|source| {
        return Error::InvalidJson {
            path: temporary.join("manifest.json"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    manifest_json.push(b'\n');
    write_secure(&temporary.join("manifest.json"), &manifest_json)?;
    fs_err::rename(&temporary, destination).map_err(|source| {
        return Error::Filesystem {
            path: destination.to_path_buf(),
            source,
        };
    })?;
    return Ok(manifest);
}

/// Run the interactive read-only semantic reviewer.
pub fn run(
    session: impl AsRef<Path>,
    around_marker: Option<&str>,
    search_query: Option<&str>,
) -> Result<()> {
    let mut model = ReviewModel::open(session, around_marker, search_query)?;
    let interrupted = Arc::new(AtomicBool::new(false));
    register_signal(SIGINT, Arc::clone(&interrupted)).map_err(|source| {
        return Error::Filesystem {
            path: PathBuf::from("<reviewer-signal-handler>"),
            source,
        };
    })?;
    register_signal(SIGTERM, Arc::clone(&interrupted)).map_err(|source| {
        return Error::Filesystem {
            path: PathBuf::from("<reviewer-signal-handler>"),
            source,
        };
    })?;
    let _guard = TerminalModeGuard::enter()?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).map_err(terminal_error)?;
    terminal.clear().map_err(terminal_error)?;
    while !interrupted.load(Ordering::Relaxed) {
        draw(&mut terminal, &model)?;
        if !event::poll(Duration::from_millis(100)).map_err(terminal_error)? {
            continue;
        }
        match event::read().map_err(terminal_error)? {
            TerminalEvent::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                KeyCode::Char('q') => break,
                KeyCode::Char('j') | KeyCode::Down | KeyCode::Right => model.move_selection(1),
                KeyCode::Char('k') | KeyCode::Up | KeyCode::Left => model.move_selection(-1),
                KeyCode::Char('p') => model.presented_only = !model.presented_only,
                KeyCode::Char('m') => model.jump_event(1, true),
                KeyCode::Char('M') => model.jump_event(-1, true),
                KeyCode::Char('i') => model.jump_event(1, false),
                KeyCode::Char('I') => model.jump_event(-1, false),
                KeyCode::Char('n') => model.jump_search(1),
                KeyCode::Char('N') => model.jump_search(-1),
                KeyCode::Char('?' | ':') => model.show_help = !model.show_help,
                _ => {}
            },
            TerminalEvent::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollDown => model.move_selection(1),
                MouseEventKind::ScrollUp => model.move_selection(-1),
                _ => {}
            },
            _ => {}
        }
    }
    terminal.show_cursor().map_err(terminal_error)?;
    return Ok(());
}

fn draw(terminal: &mut Terminal<CrosstermBackend<Stdout>>, model: &ReviewModel) -> Result<()> {
    let selected = model.selected_record()?;
    let inspection = model.selected_inspection()?;
    let viewport = sanitize_text(&state_text(&inspection.state));
    terminal
        .draw(|frame| {
            let [main, status] = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(4), Constraint::Length(2)])
                .areas(frame.area());
            let [timeline_area, viewport_area, details_area] = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(24),
                    Constraint::Percentage(60),
                    Constraint::Min(28),
                ])
                .areas(main);
            let start = model.selected.saturating_sub(10);
            let timeline = model
                .replay
                .states
                .iter()
                .enumerate()
                .skip(start)
                .take(21)
                .map(|(index, state)| {
                    let marker = if index == model.selected { ">" } else { " " };
                    let presentation = if state.presented { "P" } else { "H" };
                    return ListItem::new(format!(
                        "{marker} {presentation} s{} e{}",
                        state.state_id,
                        state
                            .event_seq
                            .map_or_else(|| return "-".to_owned(), |seq| return seq.to_string())
                    ));
                })
                .collect::<Vec<_>>();
            frame.render_widget(
                List::new(timeline).block(Block::default().title(" timeline ").borders(Borders::ALL)),
                timeline_area,
            );
            frame.render_widget(
                Paragraph::new(viewport.clone())
                    .block(Block::default().title(" semantic viewport ").borders(Borders::ALL))
                    .wrap(Wrap { trim: false }),
                viewport_area,
            );
            let native = model
                .native_correlations
                .get(&selected.state_id)
                .map_or_else(|| return "no sampled/proven native frame".to_owned(), |correlations| {
                    return correlations
                        .iter()
                        .map(|correlation| {
                            return format!(
                                "{:?} frame={} {}",
                                correlation.level,
                                correlation
                                    .frame
                                    .map_or_else(|| return "-".to_owned(), |frame| return frame.to_string()),
                                correlation.limitation
                            );
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                });
            let details = if model.show_help {
                "navigation\n  j/k state\n  m/M marker\n  i/I input\n  n/N search match\n  p presented/all\n  mouse wheel state\n  ? or : close help\n  q quit"
                    .to_owned()
            } else {
                format!(
                "state: {}\nevent: {}\ntime: {} ns\npresented: {}\nhash: {}\ndirty: {:?}\ndirty rows: {:?}\ncursor: {:?},{:?} visible={}\nmode filter: {}\nsearch: {} ({})\nnative: {}",
                selected.state_id,
                selected
                    .event_seq
                    .map_or_else(|| return "initial".to_owned(), |seq| return seq.to_string()),
                selected.monotonic_ns,
                selected.presented,
                selected.state_hash,
                selected.ghostty_dirty,
                selected.ghostty_dirty_rows,
                inspection.state.cursor.row,
                inspection.state.cursor.column,
                inspection.state.cursor.visible,
                if model.presented_only { "presented" } else { "all" },
                model.search_query.as_deref().unwrap_or("-") ,
                model.search_matches.len(),
                native,
            )
            };
            frame.render_widget(
                Paragraph::new(details)
                    .block(Block::default().title(" evidence ").borders(Borders::ALL))
                    .wrap(Wrap { trim: false }),
                details_area,
            );
            frame.render_widget(
                Paragraph::new(Line::styled(
                    " q quit · j/k state · m/M marker · i/I input · n/N match · ? help ",
                    Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
                )),
                status,
            );
        })
        .map_err(terminal_error)?;
    return Ok(());
}

fn search_state_indexes(log: &EventLog, states: &[StateRecord], query: &str) -> Result<Vec<usize>> {
    if query.is_empty() || query.len() > 4096 {
        return Err(review_error(
            "review search must contain 1 through 4096 bytes",
        ));
    }
    let mut matches = Vec::new();
    for (index, state) in states.iter().enumerate() {
        let inspection = inspect(log, StateSelector::StateId(state.state_id))?;
        if state_text(&inspection.state).contains(query) {
            matches.push(index);
        }
    }
    return Ok(matches);
}

fn marker_state_index(log: &EventLog, states: &[StateRecord], label: &str) -> Result<usize> {
    let matches = log
        .events
        .iter()
        .filter(|record| {
            return matches!(&record.event, Event::Marker { label: value, .. } if value == label);
        })
        .collect::<Vec<_>>();
    let [marker] = matches.as_slice() else {
        return Err(review_error(format!(
            "review marker `{label}` must occur exactly once, found {}",
            matches.len()
        )));
    };
    return states
        .iter()
        .position(|state| return state.monotonic_ns >= marker.monotonic_ns)
        .or_else(|| return states.len().checked_sub(1))
        .ok_or_else(|| return review_error("review contains no state near marker"));
}

fn sanitize_text(text: &str) -> String {
    return text
        .chars()
        .map(|character| {
            return if character == '\n' || character == '\t' || !character.is_control() {
                character
            } else {
                '\u{fffd}'
            };
        })
        .collect();
}

fn terminal_error(source: io::Error) -> Error {
    return Error::Filesystem {
        path: PathBuf::from("<reviewer-terminal>"),
        source,
    };
}

fn review_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorded_control_characters_are_inert_in_rendered_text() {
        let hostile = "safe\u{1b}]52;c;payload\u{7}title\rtext";
        let sanitized = sanitize_text(hostile);
        assert!(!sanitized.contains('\u{1b}'));
        assert!(!sanitized.contains('\u{7}'));
        assert!(!sanitized.contains('\r'));
        assert!(sanitized.contains("safe"));
    }
}
