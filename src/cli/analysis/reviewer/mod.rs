//! Read-only semantic forensic reviewer with safe terminal rendering.

use crate::cli::command::should_disable_color;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event as TerminalEvent, KeyCode,
        KeyEventKind, KeyModifiers, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Terminal,
};
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    flag::register as register_signal,
};
use std::{
    io::{self, Stdout},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use termtrace_engine::{
    analysis::{
        diagnostics::native_correlations_for_state,
        reviewer::{load_review_page, ReviewEntry, ReviewPage, ReviewQuery, ReviewViewport},
    },
    observation::{semantic_state_with_hash_for_session_cancellable, StatePoint},
    Error, Result,
};
use termtrace_evidence::session::read_completion;
use termtrace_media::native_capture::validated_visual_manifests_for_state;
use termtrace_model::terminal::analysis::NativeCorrelation;

struct Endpoint {
    viewport: ReviewViewport,
    state_hash: String,
    source_hash: String,
}

enum WorkKind {
    Endpoint,
    Page(ReviewQuery),
    Native(u64),
}

struct Request {
    generation: u64,
    kind: WorkKind,
    cancelled: Arc<AtomicBool>,
}

enum Loaded {
    Endpoint(Endpoint),
    Page(Option<ReviewPage>),
    Native(Vec<NativeCorrelation>),
}

#[derive(Default)]
struct Work {
    pending: Option<Request>,
    completed: Option<(u64, Result<Loaded>)>,
    stopped: bool,
}

struct ReviewWorker {
    shared: Arc<(Mutex<Work>, Condvar)>,
    running: Option<JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
    generation: u64,
}

impl ReviewWorker {
    fn start(session: PathBuf) -> Result<Self> {
        let shared = Arc::new((Mutex::new(Work::default()), Condvar::new()));
        let worker_shared = Arc::clone(&shared);
        let running = thread::Builder::new()
            .name("semantic-review-pages".to_owned())
            .spawn(move || loop {
                let (lock, changed) = &*worker_shared;
                let Ok(mut work) = lock.lock() else {
                    return;
                };

                while work.pending.is_none() && !work.stopped {
                    let Ok(next) = changed.wait(work) else {
                        return;
                    };
                    work = next;
                }

                if work.stopped {
                    return;
                }

                let Some(request) = work.pending.take() else {
                    continue;
                };
                drop(work);

                let loaded = load_work(&session, &request);

                if request.cancelled.load(Ordering::Relaxed) {
                    continue;
                }

                let Ok(mut work) = lock.lock() else {
                    return;
                };

                if !work.stopped {
                    work.completed = Some((request.generation, loaded));
                }
            })
            .map_err(terminal_error)?;

        return Ok(Self {
            shared,
            running: Some(running),
            cancelled: Arc::new(AtomicBool::new(false)),
            generation: 0,
        });
    }

    fn submit(&mut self, kind: WorkKind) -> Result<()> {
        self.cancelled.store(true, Ordering::Relaxed);
        self.cancelled = Arc::new(AtomicBool::new(false));
        self.generation = self.generation.saturating_add(1);

        let (lock, changed) = &*self.shared;
        let mut work = lock
            .lock()
            .map_err(|_| return review_error("review worker state poisoned"))?;
        work.completed = None;
        work.pending = Some(Request {
            generation: self.generation,
            kind,
            cancelled: Arc::clone(&self.cancelled),
        });

        changed.notify_one();

        return Ok(());
    }

    fn take(&self) -> Result<Option<Loaded>> {
        if self.running.as_ref().is_some_and(JoinHandle::is_finished) {
            return Err(review_error("review page worker exited unexpectedly"));
        }

        let mut work = self
            .shared
            .0
            .lock()
            .map_err(|_| return review_error("review worker state poisoned"))?;
        let Some((generation, result)) = work.completed.take() else {
            return Ok(None);
        };

        if generation != self.generation {
            return Ok(None);
        }
        return result.map(Some);
    }
}

impl Drop for ReviewWorker {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        let (lock, changed) = &*self.shared;

        if let Ok(mut work) = lock.lock() {
            work.stopped = true;
            work.pending = None;

            changed.notify_one();
        }

        if let Some(running) = self.running.take() {
            let _joined = running.join();
        }
    }
}

fn load_work(session: &Path, request: &Request) -> Result<Loaded> {
    return match &request.kind {
        WorkKind::Endpoint => {
            let (state, state_hash) = semantic_state_with_hash_for_session_cancellable(
                session,
                &StatePoint::LatestPresented,
                false,
                &request.cancelled,
            )?;

            let completion = read_completion(session)?;

            Ok(Loaded::Endpoint(Endpoint {
                viewport: ReviewViewport::from_state(&state),
                state_hash,
                source_hash: completion.final_event_hash,
            }))
        }
        WorkKind::Page(query) => {
            load_review_page(session, query, &request.cancelled).map(Loaded::Page)
        }
        WorkKind::Native(state_id) => {
            let evidence =
                validated_visual_manifests_for_state(session, *state_id, &request.cancelled)
                    .map_err(|source| return review_error(source.to_string()))?;

            let completion = read_completion(session)?;

            Ok(Loaded::Native(native_correlations_for_state(
                completion.session_id,
                *state_id,
                &evidence,
            )))
        }
    };
}

struct ReviewModel {
    session: PathBuf,
    worker: ReviewWorker,
    endpoint: Option<Endpoint>,
    page: Option<ReviewPage>,
    initial_query: Option<ReviewQuery>,
    presented_only: bool,
    search_query: Option<String>,
    show_help: bool,
    native: Vec<NativeCorrelation>,
    status: &'static str,
}

impl ReviewModel {
    fn open(
        session: &Path,
        around_marker: Option<&str>,
        search_query: Option<&str>,
    ) -> Result<Self> {
        if let Some(query) = search_query {
            validate_search_query(query)?;
        }

        let mut worker = ReviewWorker::start(session.to_path_buf())?;
        worker.submit(WorkKind::Endpoint)?;

        return Ok(Self {
            session: session.to_path_buf(),
            worker,
            endpoint: None,
            page: None,
            initial_query: Some(around_marker.map_or(ReviewQuery::Final, |label| {
                return ReviewQuery::Marker(label.to_owned());
            })),
            presented_only: true,
            search_query: search_query.map(str::to_owned),
            show_help: false,
            native: Vec::new(),
            status: "validating completed canonical source",
        });
    }

    fn update(&mut self) -> Result<()> {
        match self.worker.take()? {
            Some(Loaded::Endpoint(endpoint)) => {
                self.endpoint = Some(endpoint);

                if let Some(query) = self.initial_query.take() {
                    self.query(query)?;
                }
            }
            Some(Loaded::Page(Some(page))) => {
                let selected = page.selected;
                self.page = Some(page);
                self.native.clear();
                self.status = "exact semantic page loaded; validating selected native evidence";
                self.worker.submit(WorkKind::Native(selected))?;
            }
            Some(Loaded::Page(None)) => self.status = "no matching semantic boundary",
            Some(Loaded::Native(native)) => {
                self.native = native;
                self.status = "exact semantic page loaded";
            }
            None => {}
        }

        return Ok(());
    }

    fn selected_entry(&self) -> Option<&ReviewEntry> {
        return self.page.as_ref().and_then(|page| {
            return page
                .entries
                .iter()
                .find(|entry| return entry.state_id == page.selected);
        });
    }

    fn query(&mut self, query: ReviewQuery) -> Result<()> {
        self.status = "loading exact semantic page; navigation, Escape and quit remain available";
        self.worker.submit(WorkKind::Page(query))?;

        return Ok(());
    }

    fn move_selection(&mut self, forward: bool) -> Result<()> {
        let Some(page) = self.page.as_mut() else {
            return self.query(if forward { ReviewQuery::State(0) } else { ReviewQuery::Final });
        };
        let mut target = page.selected;

        loop {
            target = if forward { target.saturating_add(1) } else { target.saturating_sub(1) };

            if target == page.selected {
                return Ok(());
            }

            let Some(entry) = page
                .entries
                .iter()
                .find(|entry| return entry.state_id == target)
            else {
                break;
            };

            if !self.presented_only || entry.presented {
                page.selected = target;
                self.native.clear();
                self.status = "exact semantic page loaded; validating selected native evidence";
                self.worker.submit(WorkKind::Native(target))?;

                return Ok(());
            }

            if target == 0 || target == u64::MAX {
                return Ok(());
            }
        }

        let from = page.selected;

        return self.query(ReviewQuery::Move {
            from,
            forward,
            presented_only: self.presented_only,
        });
    }

    fn jump_event(&mut self, forward: bool, marker: bool) -> Result<()> {
        let from_ns = self
            .selected_entry()
            .map_or(u64::MAX, |entry| return entry.monotonic_ns);

        return self.query(ReviewQuery::Event {
            from_ns,
            forward,
            marker,
        });
    }

    fn jump_search(&mut self, forward: bool) -> Result<()> {
        let Some(query) = self.search_query.clone() else {
            self.status = "supply --search TEXT to navigate semantic matches";

            return Ok(());
        };
        let from = self.selected_entry().map(|entry| return entry.state_id);

        return self.query(ReviewQuery::Search {
            from,
            forward,
            query,
        });
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
) -> Result<ExitCode> {
    let sigint = Arc::new(AtomicBool::new(false));
    let sigterm = Arc::new(AtomicBool::new(false));
    register_signal(SIGINT, Arc::clone(&sigint)).map_err(terminal_error)?;

    register_signal(SIGTERM, Arc::clone(&sigterm)).map_err(terminal_error)?;

    let mut model = ReviewModel::open(session.as_ref(), around_marker, search_query)?;
    let _guard = TerminalModeGuard::enter()?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).map_err(terminal_error)?;
    terminal.clear().map_err(terminal_error)?;

    while !sigint.load(Ordering::Relaxed) && !sigterm.load(Ordering::Relaxed) {
        model.update()?;
        draw(&mut terminal, &model)?;

        if !event::poll(Duration::from_millis(30)).map_err(terminal_error)? {
            continue;
        }

        match event::read().map_err(terminal_error)? {
            TerminalEvent::Key(key) if key.kind == KeyEventKind::Press => {
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    sigint.store(true, Ordering::Relaxed);

                    break;
                }

                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Esc => {
                        model.worker.cancelled.store(true, Ordering::Relaxed);
                        model.status = "query cancelled; displayed evidence retained";
                    }
                    KeyCode::Char('j') | KeyCode::Down | KeyCode::Right => {
                        model.move_selection(true)?
                    }
                    KeyCode::Char('k') | KeyCode::Up | KeyCode::Left => {
                        model.move_selection(false)?
                    }
                    KeyCode::Home => model.query(ReviewQuery::State(0))?,
                    KeyCode::End => model.query(ReviewQuery::Final)?,
                    KeyCode::Char('p') => model.presented_only = !model.presented_only,
                    KeyCode::Char('m') => model.jump_event(true, true)?,
                    KeyCode::Char('M') => model.jump_event(false, true)?,
                    KeyCode::Char('i') => model.jump_event(true, false)?,
                    KeyCode::Char('I') => model.jump_event(false, false)?,
                    KeyCode::Char('n') => model.jump_search(true)?,
                    KeyCode::Char('N') => model.jump_search(false)?,
                    KeyCode::Char('?' | ':') => model.show_help = !model.show_help,
                    _ => {}
                }
            }
            TerminalEvent::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollDown => model.move_selection(true)?,
                MouseEventKind::ScrollUp => model.move_selection(false)?,
                _ => {}
            },
            _ => {}
        }
    }

    terminal.show_cursor().map_err(terminal_error)?;

    if sigint.load(Ordering::Relaxed) {
        return Ok(ExitCode::from(130));
    }
    return Ok(ExitCode::SUCCESS);
}

fn draw(terminal: &mut Terminal<CrosstermBackend<Stdout>>, model: &ReviewModel) -> Result<()> {
    let selected = model.selected_entry();
    let viewport = selected.map(|entry| return &entry.viewport).or_else(|| {
        return model
            .endpoint
            .as_ref()
            .map(|endpoint| return &endpoint.viewport);
    });

    let text = viewport.map_or_else(
        || return "Validating completed canonical source…".to_owned(),
        |viewport| return sanitize_text(&viewport.text),
    );

    terminal.draw(|frame| {
        let [main, status] = Layout::default().direction(Direction::Vertical).constraints([Constraint::Min(4), Constraint::Length(3)]).areas(frame.area());
        let [timeline_area, viewport_area, details_area] = Layout::default().direction(Direction::Horizontal).constraints([Constraint::Length(24), Constraint::Percentage(60), Constraint::Min(28)]).areas(main);
        let timeline: Vec<ListItem<'_>> = model.page.as_ref().map_or_else(
            || return vec![ListItem::new("Exact ordinals loading"), ListItem::new("Home: initial state")],
            |page| {
                let index = page.entries.iter().position(|entry| return entry.state_id == page.selected).unwrap_or(0);

                return page.entries.iter().skip(index.saturating_sub(10)).take(21).map(|entry| {
                    return ListItem::new(format!("{} {} s{} e{}", if entry.state_id == page.selected { ">" } else { " " }, if entry.presented { "P" } else { "H" }, entry.state_id, entry.event_seq.map_or_else(|| return "-".to_owned(), |seq| return seq.to_string())));
                }).collect();
            },
        );
        frame.render_widget(List::new(timeline).block(Block::default().title(" timeline page ").borders(Borders::ALL)), timeline_area);
        frame.render_widget(Paragraph::new(text.as_str()).block(Block::default().title(if selected.is_some() { " semantic viewport " } else if viewport.is_some() { " semantic viewport · authenticated endpoint " } else { " validating source " }).borders(Borders::ALL)).wrap(Wrap { trim: false }), viewport_area);

        let details = if model.show_help {
            "navigation\n  j/k state\n  Home/End first/final\n  m/M marker\n  i/I input\n  n/N search match\n  p presented/all\n  mouse wheel state\n  Escape cancel query\n  ? or : close help\n  q quit".to_owned()
        } else {
            let identity = selected.map_or_else(|| return model.endpoint.as_ref().map_or_else(|| return "source: validating".to_owned(), |endpoint| return format!("state: endpoint (ordinal pending)\npresented: true\nhash: {}\ncanonical final hash: {}", endpoint.state_hash, endpoint.source_hash)), |entry| return format!("state: {}\nevent: {}\nbyte offset: {:?}\ntime: {} ns\npresented: {}\nhash: {}\ndirty: {}\ndirty rows: {:?}", entry.state_id, entry.event_seq.map_or_else(|| return "initial".to_owned(), |seq| return seq.to_string()), entry.event_byte_offset, entry.monotonic_ns, entry.presented, entry.state_hash, entry.dirty, entry.dirty_rows));
            let native = if model.native.is_empty() { "no sampled/proven native frame".to_owned() } else { model.native.iter().map(|correlation| return format!("{:?} frame={:?} {}", correlation.level, correlation.frame, correlation.limitation)).collect::<Vec<_>>().join("\n") };

            format!("{}\nsession: {}\ncursor: {:?},{:?} visible={}\nmode filter: {}\nsearch: {} (on-demand exact scan)\nnative: {}", identity, sanitize_text(&model.session.display().to_string()), viewport.and_then(|viewport| return viewport.cursor_row), viewport.and_then(|viewport| return viewport.cursor_column), viewport.is_some_and(|viewport| return viewport.cursor_visible), if model.presented_only { "presented" } else { "all" }, sanitize_text(model.search_query.as_deref().unwrap_or("-")), native)
        };
        frame.render_widget(Paragraph::new(details).block(Block::default().title(" evidence ").borders(Borders::ALL)).wrap(Wrap { trim: false }), details_area);
        let footer_style = if should_disable_color() {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
        };
        frame.render_widget(Paragraph::new(vec![Line::styled(" q quit · j/k state · m/M marker · i/I input · n/N match · ? help ", footer_style), Line::from(model.status)]), status);
    }).map_err(terminal_error)?;

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

fn sanitize_text(text: &str) -> String {
    return text
        .chars()
        .map(|character| {
            if character == '\n' || character == '\t' || !character.is_control() {
                return character;
            }
            return '\u{fffd}';
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
    use super::sanitize_text;

    #[test]
    fn recorded_control_characters_are_inert_in_rendered_text() {
        let sanitized = sanitize_text("safe\u{1b}]52;c;payload\u{7}title\rtext");
        assert!(!sanitized.contains('\u{1b}'));
        assert!(!sanitized.contains('\u{7}'));
        assert!(!sanitized.contains('\r'));
        assert!(sanitized.contains("safe"));
    }
}
