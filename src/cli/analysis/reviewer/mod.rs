//! Read-only semantic forensic reviewer with safe terminal rendering.

use std::{
    collections::BTreeMap,
    io::{self, Stdout},
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
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    flag::register as register_signal,
};

use termtrace_engine::{
    Error, Result,
    analysis::diagnostics::native_correlations,
    observation::state_text,
    replay::{inspect, visit_states},
};
use termtrace_evidence::{event_log::EventLog, session::validate_completed_log};
use termtrace_media::native_capture::validated_visual_manifests;
use termtrace_model::{
    execution::event::{Event, EventRecord},
    terminal::{
        analysis::NativeCorrelation,
        replay::{StateRecord, StateSelector},
    },
};

struct RenderedViewport {
    text: String,
    cursor_row: Option<u16>,
    cursor_column: Option<u16>,
    cursor_visible: bool,
}

#[derive(Default)]
struct SelectedViewportCache {
    rendered: Option<(usize, RenderedViewport)>,
}

impl SelectedViewportCache {
    fn ensure_with(
        &mut self,
        selected: usize,
        render: impl FnOnce() -> Result<RenderedViewport>,
    ) -> Result<()> {
        if self
            .rendered
            .as_ref()
            .is_some_and(|(cached, _)| return *cached == selected)
        {
            return Ok(());
        }
        self.rendered = Some((selected, render()?));
        return Ok(());
    }

    fn get(&self, selected: usize) -> Result<&RenderedViewport> {
        let Some((cached, rendered)) = &self.rendered else {
            return Err(review_error("selected reviewer viewport cache is empty"));
        };
        if *cached != selected {
            return Err(review_error("selected reviewer viewport is not rendered"));
        }
        return Ok(rendered);
    }
}

struct ReviewModel {
    log: EventLog,
    entries: Vec<StateRecord>,
    selected: usize,
    selected_viewport: SelectedViewportCache,
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
        if let Some(query) = search_query {
            validate_search_query(query)?;
        }
        let mut entries: Vec<StateRecord> = Vec::new();
        let mut search_matches: Vec<usize> = Vec::new();
        visit_states(&log, |record, state| {
            if let Some(query) = search_query
                && state_text(state).contains(query)
            {
                search_matches.push(entries.len());
            }
            entries.push(record.clone());
            return Ok(());
        })?;
        if entries.is_empty() {
            return Err(review_error(
                "completed session contains no semantic states",
            ));
        }
        let selected = match around_marker {
            Some(label) => marker_state_index(&log, &entries, label)?,
            None => entries.len().saturating_sub(1),
        };
        let visual_evidence = validated_visual_manifests(&session).map_err(|source| {
            return Error::InvalidReplay {
                reason: source.to_string(),
            };
        })?;
        let native_correlations = native_correlations(&log, &visual_evidence);
        return Ok(Self {
            log,
            entries,
            selected,
            selected_viewport: SelectedViewportCache::default(),
            presented_only: true,
            search_query: search_query.map(str::to_owned),
            search_matches,
            show_help: false,
            native_correlations,
        });
    }

    fn selected_entry(&self) -> Result<&StateRecord> {
        return self
            .entries
            .get(self.selected)
            .ok_or_else(|| return review_error("selected reviewer state is out of bounds"));
    }

    fn refresh_selected_viewport(&mut self) -> Result<()> {
        let selected = self.selected;
        let state_id = self.selected_entry()?.state_id;
        let log = &self.log;
        return self.selected_viewport.ensure_with(selected, || {
            let inspection = inspect(log, StateSelector::StateId(state_id))?;
            return Ok(RenderedViewport {
                text: sanitize_text(&state_text(&inspection.state)),
                cursor_row: inspection.state.cursor.row,
                cursor_column: inspection.state.cursor.column,
                cursor_visible: inspection.state.cursor.visible,
            });
        });
    }

    fn selected_viewport(&self) -> Result<&RenderedViewport> {
        return self.selected_viewport.get(self.selected);
    }

    fn move_selection(&mut self, direction: isize) {
        let mut candidate = self.selected;
        loop {
            candidate = if direction < 0 {
                candidate.saturating_sub(1)
            } else {
                candidate
                    .saturating_add(1)
                    .min(self.entries.len().saturating_sub(1))
            };
            let Some(entry) = self.entries.get(candidate) else {
                return;
            };
            if !self.presented_only || entry.presented {
                self.selected = candidate;
                return;
            }
            if candidate == 0 || candidate + 1 == self.entries.len() {
                return;
            }
        }
    }

    fn jump_event(&mut self, direction: isize, marker: bool) {
        let selected_time = self
            .entries
            .get(self.selected)
            .map_or(0, |entry| return entry.monotonic_ns);
        let predicate = |record: &&EventRecord| {
            if marker {
                matches!(record.event, Event::Marker { .. })
            } else {
                matches!(record.event, Event::InputIntent { .. })
            }
        };
        let target = if direction < 0 {
            self.log
                .events
                .iter()
                .rev()
                .filter(predicate)
                .map(|record| record.monotonic_ns)
                .find(|time| *time < selected_time)
        } else {
            self.log
                .events
                .iter()
                .filter(predicate)
                .map(|record| record.monotonic_ns)
                .find(|time| *time > selected_time)
        };
        if let Some(time) = target
            && let Some(index) = self
                .entries
                .iter()
                .position(|entry| return entry.monotonic_ns >= time)
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
        let guard = Self;
        execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture).map_err(terminal_error)?;
        return Ok(guard);
    }
}

impl Drop for TerminalModeGuard {
    fn drop(&mut self) {
        let _leave_result = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
        let _raw_result = disable_raw_mode();
    }
}

/// Run the interactive read-only semantic reviewer.
pub(in crate::cli) fn run(
    session: impl AsRef<Path>,
    around_marker: Option<&str>,
    search_query: Option<&str>,
) -> Result<()> {
    let mut model = ReviewModel::open(session, around_marker, search_query)?;
    model.refresh_selected_viewport()?;
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
        model.refresh_selected_viewport()?;
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
    let selected = model.selected_entry()?;
    let viewport = model.selected_viewport()?;
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
            let timeline = model.entries.iter()
                .enumerate()
                .skip(start)
                .take(21)
                .map(|(index, entry)| {
                    let state = entry;
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
                Paragraph::new(viewport.text.as_str())
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
                viewport.cursor_row,
                viewport.cursor_column,
                viewport.cursor_visible,
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

fn validate_search_query(query: &str) -> Result<()> {
    if query.is_empty() || query.len() > 4096 {
        return Err(review_error(
            "review search must contain 1 through 4096 bytes",
        ));
    }
    return Ok(());
}

fn marker_state_index(log: &EventLog, entries: &[StateRecord], label: &str) -> Result<usize> {
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
    return entries
        .iter()
        .position(|entry| return entry.monotonic_ns >= marker.monotonic_ns)
        .or_else(|| return entries.len().checked_sub(1))
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
    use std::cell::Cell;

    use super::{RenderedViewport, SelectedViewportCache, sanitize_text};

    #[test]
    fn recorded_control_characters_are_inert_in_rendered_text() {
        let hostile = "safe\u{1b}]52;c;payload\u{7}title\rtext";
        let sanitized = sanitize_text(hostile);
        assert!(!sanitized.contains('\u{1b}'));
        assert!(!sanitized.contains('\u{7}'));
        assert!(!sanitized.contains('\r'));
        assert!(sanitized.contains("safe"));
    }

    #[test]
    fn selected_viewport_cache_replays_only_for_selection_changes() {
        let replay_count = Cell::new(0);
        let mut cache = SelectedViewportCache::default();
        let render = || {
            replay_count.set(replay_count.get() + 1);
            return Ok(RenderedViewport {
                text: format!("viewport {}", replay_count.get()),
                cursor_row: None,
                cursor_column: None,
                cursor_visible: false,
            });
        };

        cache.ensure_with(4, render).expect("first selected render");
        cache
            .ensure_with(4, render)
            .expect("unchanged selected render");
        assert_eq!(replay_count.get(), 1);
        assert_eq!(
            cache.get(4).expect("cached selected viewport").text,
            "viewport 1"
        );

        cache
            .ensure_with(5, render)
            .expect("changed selected render");
        assert_eq!(replay_count.get(), 2);
        assert!(cache.get(4).is_err(), "previous viewport must be evicted");
        assert_eq!(
            cache.get(5).expect("replacement selected viewport").text,
            "viewport 2"
        );
    }
}
