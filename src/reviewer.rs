//! Read-only semantic forensic reviewer with safe terminal rendering.

use std::{
    io::{self, Stdout},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use crossterm::{
    event::{self, Event as TerminalEvent, KeyCode, KeyEventKind},
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

use crate::{
    Error, Result,
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
    /// Final semantic state hash.
    pub final_state_hash: String,
    /// Safety boundary used for viewport rendering.
    pub rendering_policy: String,
}

struct ReviewModel {
    session: PathBuf,
    log: EventLog,
    replay: CollectedReplay,
    selected: usize,
    presented_only: bool,
}

impl ReviewModel {
    fn open(session: impl AsRef<Path>, around_marker: Option<&str>) -> Result<Self> {
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
        return Ok(Self {
            session,
            log,
            replay,
            selected,
            presented_only: true,
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
}

struct TerminalModeGuard;

impl TerminalModeGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode().map_err(terminal_error)?;
        execute!(io::stdout(), EnterAlternateScreen).map_err(terminal_error)?;
        return Ok(Self);
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        let _leave_result = execute!(io::stdout(), LeaveAlternateScreen);
        let _raw_result = disable_raw_mode();
    }
}

/// Load and summarize a validated completed session without changing it.
pub fn summarize(session: impl AsRef<Path>, around_marker: Option<&str>) -> Result<ReviewSummary> {
    return Ok(ReviewModel::open(session, around_marker)?.summary());
}

/// Run the interactive read-only semantic reviewer.
pub fn run(session: impl AsRef<Path>, around_marker: Option<&str>) -> Result<()> {
    let mut model = ReviewModel::open(session, around_marker)?;
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
        let TerminalEvent::Key(key) = event::read().map_err(terminal_error)? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') => break,
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Right => model.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up | KeyCode::Left => model.move_selection(-1),
            KeyCode::Char('p') => model.presented_only = !model.presented_only,
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
            let details = format!(
                "state: {}\nevent: {}\ntime: {} ns\npresented: {}\nhash: {}\ndirty: {:?}\ndirty rows: {:?}\ncursor: {:?},{:?} visible={}\nmode filter: {}",
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
            );
            frame.render_widget(
                Paragraph::new(details)
                    .block(Block::default().title(" evidence ").borders(Borders::ALL))
                    .wrap(Wrap { trim: false }),
                details_area,
            );
            frame.render_widget(
                Paragraph::new(Line::styled(
                    " q quit · j/k next/previous · p presented/all · semantic cells only ",
                    Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD),
                )),
                status,
            );
        })
        .map_err(terminal_error)?;
    return Ok(());
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
