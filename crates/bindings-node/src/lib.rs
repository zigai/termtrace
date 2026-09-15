//! Native Node.js and TypeScript bindings for Termtrace.

#![allow(
    unsafe_code,
    clippy::single_char_lifetime_names,
    clippy::implicit_return,
    clippy::string_slice,
    clippy::absolute_paths,
    clippy::redundant_clone,
    clippy::option_if_let_else,
    clippy::map_unwrap_or,
    reason = "NAPI-RS derive and function macros generate FFI code, single-char lifetimes, and implicit returns"
)]

use napi_derive::napi;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use termtrace_model::artifact::bundle::{BundleProfile, BundleVerificationPolicy};
use termtrace_model::artifact::save::SaveFormat;
use termtrace_model::execution::session::VerificationLevel;
use termtrace_model::protocol::control::ScreenPredicate;
use termtrace_model::terminal::observation::StatePoint;
use termtrace_model::terminal::screen::ScreenProjectionRequest;
use termtrace_model::terminal::state::{KeyModifiers, MouseButton, NamedKey};
use termtrace_sdk::actions::{ActionCondition, ActionOptions, InputAtom};
use termtrace_sdk::analysis::AnalysisService;
use termtrace_sdk::artifacts::ArtifactService;
use termtrace_sdk::client::{Session, Terminal};
use termtrace_sdk::events::{EventCursor, EventsService};
use termtrace_sdk::integrity::IntegrityService;
use termtrace_sdk::output::{OutputCursor, OutputService};
use termtrace_sdk::registry::RegistryService;
use termtrace_sdk::screen::frame_text;
use termtrace_sdk::system::SystemService;
use termtrace_sdk::tmux::TmuxService;

fn to_napi_err(err: termtrace_sdk::Error) -> napi::Error {
    return napi::Error::new(napi::Status::GenericFailure, err.to_string());
}

fn default_termtrace_bin() -> PathBuf {
    if let Ok(bin) = std::env::var("TERMTRACE_BIN") {
        return PathBuf::from(bin);
    }

    if let Ok(exe) = std::env::current_exe() {
        let candidate = exe.with_file_name("termtrace");
        if candidate.is_file() {
            return candidate;
        }
    }

    return PathBuf::from("termtrace");
}

fn default_evidence_root() -> PathBuf {
    if let Ok(root) = std::env::var("TERMTRACE_ROOT") {
        return PathBuf::from(root);
    }
    return std::env::temp_dir().join("termtrace");
}

fn parse_key(key: &str) -> napi::Result<(NamedKey, KeyModifiers)> {
    let lower = key.to_ascii_lowercase();
    let mut modifiers = KeyModifiers::default();
    let key_part = if lower.starts_with("ctrl+") || lower.starts_with("ctrl-") {
        modifiers.control = true;

        &lower[5..]
    } else if lower.starts_with("alt+") || lower.starts_with("alt-") {
        modifiers.alt = true;

        &lower[4..]
    } else if lower.starts_with("shift+") || lower.starts_with("shift-") {
        modifiers.shift = true;

        &lower[6..]
    } else {
        lower.as_str()
    };

    let named = match key_part {
        "enter" | "return" => NamedKey::Enter,
        "esc" | "escape" => NamedKey::Escape,
        "tab" => NamedKey::Tab,
        "backspace" => NamedKey::Backspace,
        "delete" | "del" => NamedKey::Delete,
        "insert" | "ins" => NamedKey::Insert,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "pageup" | "page_up" => NamedKey::PageUp,
        "pagedown" | "page_down" => NamedKey::PageDown,
        "up" | "arrowup" | "arrow_up" => NamedKey::ArrowUp,
        "down" | "arrowdown" | "arrow_down" => NamedKey::ArrowDown,
        "left" | "arrowleft" | "arrow_left" => NamedKey::ArrowLeft,
        "right" | "arrowright" | "arrow_right" => NamedKey::ArrowRight,
        s if s.len() == 1 => {
            let c = s
                .chars()
                .next()
                .ok_or_else(|| return napi::Error::from_reason("empty key string"))?;

            NamedKey::Letter(c)
        }
        _ => {
            return Err(napi::Error::from_reason(format!(
                "unknown key name: '{key}'"
            )))
        }
    };

    return Ok((named, modifiers));
}

/// Returns the Termtrace core version.
#[napi]
pub fn version() -> String {
    return "0.1.0".to_string();
}

/// Rich screen frame rendered against terminal geometry.
#[napi(js_name = "ScreenFrame")]
#[derive(Clone)]
pub struct JsFrame {
    /// Viewport rows.
    pub rows: u32,
    /// Viewport columns.
    pub cols: u32,
    /// Plain text of all rendered rows.
    pub text: String,
}

/// Immutable snapshot of a resolved presented terminal state.
#[napi(js_name = "ResolvedState")]
#[derive(Clone)]
pub struct JsResolvedState {
    /// Content-addressed semantic state hash.
    pub hash: String,
    /// Plain text of the viewport.
    pub text: String,
    /// Zero-indexed cursor row.
    pub cursor_row: u32,
    /// Zero-indexed cursor column.
    pub cursor_col: u32,
}

/// Result of reading a page of raw child PTY output bytes.
#[napi(object)]
pub struct JsOutputPage {
    /// Raw bytes produced by the child process.
    pub bytes: Vec<u8>,
    /// Byte offset for fetching the subsequent page.
    pub next_offset: u32,
    /// Whether additional bytes remain beyond this page.
    pub has_more: bool,
}

/// Screen inspection capability attached to a live or completed session.
#[napi(js_name = "Screen")]
#[derive(Clone)]
pub struct JsScreen {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: String,
}

#[napi]
impl JsScreen {
    /// Return the current presented screen frame.
    #[napi]
    pub async fn frame(&self) -> napi::Result<JsFrame> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();

        return tokio::task::spawn_blocking(move || {
            let session = terminal.attach(path).map_err(to_napi_err)?;
            let screen = session.screen();
            let frame = screen
                .frame(ScreenProjectionRequest::default())
                .map_err(to_napi_err)?;
            let text = frame_text(&frame);

            return Ok(JsFrame {
                rows: u32::from(frame.geometry.rows),
                cols: u32::from(frame.geometry.cols),
                text,
            });
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Return the plain text of the latest presented frame.
    #[napi]
    pub async fn text(&self) -> napi::Result<String> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();

        return tokio::task::spawn_blocking(move || {
            let session = terminal.attach(path).map_err(to_napi_err)?;
            let screen = session.screen();
            let frame = screen
                .frame(ScreenProjectionRequest::default())
                .map_err(to_napi_err)?;

            return Ok(frame_text(&frame));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Wait until the presented screen contains the specified text pattern.
    #[napi]
    pub async fn wait_for_text(
        &self,
        pattern: String,
        timeout_ms: Option<u32>,
    ) -> napi::Result<JsResolvedState> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();
        let timeout = Duration::from_millis(u64::from(timeout_ms.unwrap_or(10_000)));

        return tokio::task::spawn_blocking(move || {
            let session = terminal.attach(path).map_err(to_napi_err)?;
            let screen = session.screen();
            let outcome = screen
                .wait_for_text(&pattern, timeout)
                .map_err(to_napi_err)?;
            if !outcome.matched {
                return Err(napi::Error::from_reason(format!(
                    "timed out waiting for pattern: '{pattern}'"
                )));
            }
            let text = frame_text(&outcome.frame);

            return Ok(JsResolvedState {
                hash: outcome.frame.state_hash.clone(),
                text,
                cursor_row: u32::from(outcome.frame.cursor.row.unwrap_or(0)),
                cursor_col: u32::from(outcome.frame.cursor.column.unwrap_or(0)),
            });
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Wait until the presented screen remains unchanged for an idle window.
    #[napi]
    pub async fn wait_for_idle(&self, timeout_ms: Option<u32>) -> napi::Result<JsResolvedState> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();
        let timeout = Duration::from_millis(u64::from(timeout_ms.unwrap_or(10_000)));

        return tokio::task::spawn_blocking(move || {
            let session = terminal.attach(path).map_err(to_napi_err)?;
            let screen = session.screen();
            let capture = screen.wait_for_idle(timeout).map_err(to_napi_err)?;
            let text = frame_text(&capture.frame);

            return Ok(JsResolvedState {
                hash: capture.frame.state_hash.clone(),
                text,
                cursor_row: u32::from(capture.frame.cursor.row.unwrap_or(0)),
                cursor_col: u32::from(capture.frame.cursor.column.unwrap_or(0)),
            });
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }
}

/// Fluent builder for atomic, state-guarded input sequences.
#[napi(js_name = "ActionBuilder")]
pub struct JsActionBuilder {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: String,
    pub(crate) atoms: Mutex<Vec<InputAtom>>,
    pub(crate) options: Mutex<ActionOptions>,
}

#[napi]
impl JsActionBuilder {
    /// Enforce that the terminal must be at the specified state hash before executing.
    #[napi]
    pub fn if_state(&self, state_hash: String) {
        if let Ok(mut opt) = self.options.lock() {
            opt.if_state = Some(state_hash);
        }
    }

    /// Append UTF-8 text to send to the terminal.
    #[napi]
    pub fn send_text(&self, text: String) {
        if let Ok(mut atoms) = self.atoms.lock() {
            atoms.push(InputAtom::Text(text));
        }
    }

    /// Append a symbolic named key (e.g. 'Enter', 'Ctrl-C', 'Tab').
    #[napi]
    pub fn send_key(&self, key: String) -> napi::Result<()> {
        let (named, modifiers) = parse_key(&key)?;
        if let Ok(mut atoms) = self.atoms.lock() {
            atoms.push(InputAtom::Key {
                key: named,
                modifiers,
            });
        }

        return Ok(());
    }

    /// Set delay pacing between consecutive action atoms in milliseconds.
    #[napi]
    pub fn pace(&self, delay_ms: u32) {
        if let Ok(mut opt) = self.options.lock() {
            opt.pace = Duration::from_millis(u64::from(delay_ms));
        }
    }

    /// Append exact raw bytes to send to the terminal.
    #[napi]
    pub fn send_bytes(&self, bytes: Vec<u8>) {
        if let Ok(mut atoms) = self.atoms.lock() {
            atoms.push(InputAtom::Bytes(bytes));
        }
    }

    /// Send bracketed paste text.
    #[napi]
    pub fn paste(&self, text: String) {
        if let Ok(mut atoms) = self.atoms.lock() {
            atoms.push(InputAtom::Paste(text));
        }
    }

    /// Wait until the screen contains the specified pattern after applying input.
    #[napi]
    pub fn until_screen(&self, pattern: String, timeout_ms: Option<u32>) {
        let timeout = Duration::from_millis(u64::from(timeout_ms.unwrap_or(10_000)));
        if let Ok(mut opt) = self.options.lock() {
            opt.postcondition = ActionCondition::Screen {
                predicate: ScreenPredicate::Contains { text: pattern },
                timeout,
            };
        }
    }

    /// Wait until the screen is idle and unchanged after applying input.
    #[napi]
    pub fn until_idle(&self, stable_for_ms: Option<u32>, timeout_ms: Option<u32>) {
        let stable_for = Duration::from_millis(u64::from(stable_for_ms.unwrap_or(20)));
        let timeout = Duration::from_millis(u64::from(timeout_ms.unwrap_or(10_000)));
        if let Ok(mut opt) = self.options.lock() {
            opt.postcondition = ActionCondition::Idle {
                stable_for,
                timeout,
            };
        }
    }

    /// Execute the action sequence and return the terminating screen frame.
    #[napi]
    pub async fn execute(&self) -> napi::Result<Option<JsResolvedState>> {
        let atoms = match self.atoms.lock() {
            Ok(mut a) => std::mem::take(&mut *a),
            Err(e) => std::mem::take(&mut *e.into_inner()),
        };
        let options = match self.options.lock() {
            Ok(o) => o.clone(),
            Err(e) => e.into_inner().clone(),
        };
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();

        return tokio::task::spawn_blocking(move || {
            let session = terminal.attach(path).map_err(to_napi_err)?;
            let result = session.act(&atoms, options).map_err(to_napi_err)?;
            if let Some(frame) = result.frame {
                let text = frame_text(&frame);
                return Ok(Some(JsResolvedState {
                    hash: frame.state_hash,
                    text,
                    cursor_row: u32::from(frame.cursor.row.unwrap_or(0)),
                    cursor_col: u32::from(frame.cursor.column.unwrap_or(0)),
                }));
            }

            return Ok(None);
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }
}

/// A logical Termtrace session reference.
#[napi(js_name = "Session")]
#[derive(Clone)]
pub struct JsSession {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: String,
    pub(crate) session_id: String,
}

#[napi]
impl JsSession {
    /// Canonical session identity string.
    #[napi(getter)]
    pub fn session_id(&self) -> String {
        return self.session_id.clone();
    }

    /// Filesystem path to the session directory.
    #[napi(getter)]
    pub fn path(&self) -> String {
        return self.session_path.clone();
    }

    /// Access screen inspection capabilities.
    #[napi]
    pub fn screen(&self) -> JsScreen {
        return JsScreen {
            terminal: self.terminal.clone(),
            session_path: self.session_path.clone(),
        };
    }

    /// Create an action builder for input automation.
    #[napi]
    pub fn actions(&self) -> JsActionBuilder {
        return JsActionBuilder {
            terminal: self.terminal.clone(),
            session_path: self.session_path.clone(),
            atoms: Mutex::new(Vec::new()),
            options: Mutex::new(ActionOptions::default()),
        };
    }

    /// Click mouse button at a terminal cell.
    #[napi]
    pub async fn pointer_click(
        &self,
        column: u32,
        row: u32,
        button: Option<String>,
        strict: Option<bool>,
    ) -> napi::Result<()> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();
        let col_val = u16::try_from(column).map_err(|_| {
            return napi::Error::from_reason(format!("column {column} exceeds maximum 65535"));
        })?;
        let row_val = u16::try_from(row).map_err(|_| {
            return napi::Error::from_reason(format!("row {row} exceeds maximum 65535"));
        })?;
        let btn = match button
            .as_deref()
            .unwrap_or("left")
            .to_ascii_lowercase()
            .as_str()
        {
            "left" => MouseButton::Left,
            "right" => MouseButton::Right,
            "middle" => MouseButton::Middle,
            "wheel_up" | "wheelup" => MouseButton::WheelUp,
            "wheel_down" | "wheeldown" => MouseButton::WheelDown,
            other => {
                return Err(napi::Error::from_reason(format!(
                    "unknown mouse button: '{other}'"
                )))
            }
        };
        let is_strict = strict.unwrap_or(false);

        return tokio::task::spawn_blocking(move || {
            let session = terminal.attach(path).map_err(to_napi_err)?;
            session
                .pointer()
                .click(col_val, row_val, btn, is_strict)
                .map_err(to_napi_err)?;

            return Ok(());
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Scroll mouse wheel at a terminal cell.
    #[napi]
    pub async fn pointer_scroll(
        &self,
        column: u32,
        row: u32,
        lines: Option<i32>,
        strict: Option<bool>,
    ) -> napi::Result<()> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();
        let col_val = u16::try_from(column).map_err(|_| {
            return napi::Error::from_reason(format!("column {column} exceeds maximum 65535"));
        })?;
        let row_val = u16::try_from(row).map_err(|_| {
            return napi::Error::from_reason(format!("row {row} exceeds maximum 65535"));
        })?;
        let line_count = i16::try_from(lines.unwrap_or(1)).unwrap_or(1);
        let is_strict = strict.unwrap_or(false);

        return tokio::task::spawn_blocking(move || {
            let session = terminal.attach(path).map_err(to_napi_err)?;
            session
                .pointer()
                .scroll(col_val, row_val, line_count, is_strict)
                .map_err(to_napi_err)?;

            return Ok(());
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Report terminal focus state.
    #[napi]
    pub async fn pointer_focus(&self, focused: bool, strict: Option<bool>) -> napi::Result<()> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();
        let is_strict = strict.unwrap_or(false);

        return tokio::task::spawn_blocking(move || {
            let session = terminal.attach(path).map_err(to_napi_err)?;
            session
                .pointer()
                .focus(focused, is_strict)
                .map_err(to_napi_err)?;

            return Ok(());
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Atomically publish visual representations (PNG, SVG, HTML, TXT, JSON) of a state point.
    #[napi]
    pub async fn save(
        &self,
        destination: String,
        formats: Option<Vec<String>>,
        state_hash: Option<String>,
        include_hidden: Option<bool>,
    ) -> napi::Result<String> {
        let session_path = PathBuf::from(&self.session_path);
        let dest = PathBuf::from(destination);
        let save_formats = if let Some(fmts) = formats {
            let mut list = Vec::new();
            for f in fmts {
                match f.to_ascii_lowercase().as_str() {
                    "png" => list.push(SaveFormat::Png),
                    "svg" => list.push(SaveFormat::Svg),
                    "html" => list.push(SaveFormat::Html),
                    "txt" => list.push(SaveFormat::Txt),
                    "json" => list.push(SaveFormat::Json),
                    "deep-json" | "deep_json" => list.push(SaveFormat::DeepJson),
                    "vt" => list.push(SaveFormat::Vt),
                    other => {
                        return Err(napi::Error::from_reason(format!(
                            "unknown save format: '{other}'"
                        )))
                    }
                }
            }

            list
        } else {
            vec![
                SaveFormat::Png,
                SaveFormat::Svg,
                SaveFormat::Txt,
                SaveFormat::Json,
            ]
        };
        let point = state_hash.map_or(StatePoint::LatestPresented, |h| {
            return StatePoint::StateHash { state_hash: h };
        });
        let hidden = include_hidden.unwrap_or(false);

        return tokio::task::spawn_blocking(move || {
            let outcome = ArtifactService::save(&session_path, &dest, point, &save_formats, hidden)
                .map_err(to_napi_err)?;

            return serde_json::to_string_pretty(&outcome.manifest)
                .map_err(|e| return napi::Error::from_reason(e.to_string()));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Export session terminal playback as an asciicast v2 file.
    #[napi]
    pub async fn export_asciicast(&self, output_path: String) -> napi::Result<()> {
        let session_path = PathBuf::from(&self.session_path);
        let target = PathBuf::from(output_path);

        return tokio::task::spawn_blocking(move || {
            ArtifactService::export_asciicast(&session_path, &target).map_err(to_napi_err)?;

            return Ok(());
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Create a compressed evidence bundle (.tar.zst) for archiving or forensics.
    #[napi]
    pub async fn create_bundle(&self, output_path: String) -> napi::Result<()> {
        let session_path = PathBuf::from(&self.session_path);
        let target = PathBuf::from(output_path);

        return tokio::task::spawn_blocking(move || {
            ArtifactService::create_bundle(&session_path, &target, BundleProfile::Forensic)
                .map_err(to_napi_err)?;

            return Ok(());
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Compute semantic difference between two presented states and return JSON.
    #[napi]
    pub async fn diff(&self, from_hash: String, to_hash: String) -> napi::Result<String> {
        let session_path = PathBuf::from(&self.session_path);

        return tokio::task::spawn_blocking(move || {
            let from = StatePoint::StateHash {
                state_hash: from_hash,
            };
            let to = StatePoint::StateHash {
                state_hash: to_hash,
            };
            let (diff, _, _) =
                AnalysisService::diff(&session_path, from, to).map_err(to_napi_err)?;

            return serde_json::to_string_pretty(&diff)
                .map_err(|e| return napi::Error::from_reason(e.to_string()));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Read raw child PTY output bytes using an optional byte offset cursor.
    #[napi]
    pub async fn read_output(
        &self,
        offset: Option<u32>,
        limit: Option<u32>,
    ) -> napi::Result<JsOutputPage> {
        let session_path = PathBuf::from(&self.session_path);
        let cursor = offset.map(|off| {
            return OutputCursor {
                session_path: session_path.clone(),
                byte_offset: u64::from(off),
                event_seq: None,
            };
        });
        let limit_val = usize::try_from(limit.unwrap_or(65536)).unwrap_or(65536);

        return tokio::task::spawn_blocking(move || {
            let page =
                OutputService::read_page(&session_path, cursor, limit_val).map_err(to_napi_err)?;
            let next_offset = u32::try_from(page.cursor.byte_offset).unwrap_or(u32::MAX);

            return Ok(JsOutputPage {
                bytes: page.bytes,
                next_offset,
                has_more: page.has_more,
            });
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Wait for exact child output bytes to appear in the canonical stream.
    #[napi]
    pub async fn wait_for_output(
        &self,
        pattern: String,
        timeout_ms: Option<u32>,
    ) -> napi::Result<bool> {
        let session_path = PathBuf::from(&self.session_path);
        let timeout = Duration::from_millis(u64::from(timeout_ms.unwrap_or(10_000)));

        return tokio::task::spawn_blocking(move || {
            let _outcome =
                OutputService::wait_for_bytes(&session_path, pattern.as_bytes(), timeout)
                    .map_err(to_napi_err)?;

            return Ok(true);
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Read canonical event records as pretty JSON starting at an optional event sequence.
    #[napi]
    pub async fn read_events(
        &self,
        cursor: Option<u32>,
        limit: Option<u32>,
    ) -> napi::Result<String> {
        let session_path = PathBuf::from(&self.session_path);
        let event_cursor = cursor.map(|seq| {
            return EventCursor {
                session_path: session_path.clone(),
                next_event_seq: u64::from(seq),
            };
        });
        let limit_val = usize::try_from(limit.unwrap_or(100)).unwrap_or(100);

        return tokio::task::spawn_blocking(move || {
            let page = EventsService::read_page(&session_path, event_cursor, limit_val)
                .map_err(to_napi_err)?;

            return serde_json::to_string_pretty(&page.events)
                .map_err(|e| return napi::Error::from_reason(e.to_string()));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Explain VT and semantic evidence around a target state point.
    #[napi]
    pub async fn explain(
        &self,
        target_hash: Option<String>,
        context: Option<u32>,
    ) -> napi::Result<String> {
        let session_path = PathBuf::from(&self.session_path);
        let point = target_hash.map_or(StatePoint::LatestPresented, |h| {
            return StatePoint::StateHash { state_hash: h };
        });
        let ctx = u64::from(context.unwrap_or(1));

        return tokio::task::spawn_blocking(move || {
            let report =
                AnalysisService::explain(&session_path, point, ctx).map_err(to_napi_err)?;

            return serde_json::to_string_pretty(&report)
                .map_err(|e| return napi::Error::from_reason(e.to_string()));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Run conservative evidence-backed rendering diagnostics on the session.
    #[napi]
    pub async fn diagnose(&self, broad_change_percent: Option<u32>) -> napi::Result<String> {
        let session_path = PathBuf::from(&self.session_path);
        let pct = u8::try_from(broad_change_percent.unwrap_or(25)).unwrap_or(25);

        return tokio::task::spawn_blocking(move || {
            let report =
                AnalysisService::diagnose(&session_path, Vec::new(), pct).map_err(to_napi_err)?;

            return serde_json::to_string_pretty(&report)
                .map_err(|e| return napi::Error::from_reason(e.to_string()));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Perform full triage analysis on the most recent canonical action.
    #[napi]
    pub async fn last_action(&self, broad_change_percent: Option<u32>) -> napi::Result<String> {
        let session_path = PathBuf::from(&self.session_path);
        let pct = u8::try_from(broad_change_percent.unwrap_or(25)).unwrap_or(25);

        return tokio::task::spawn_blocking(move || {
            let analysis = AnalysisService::last_action(&session_path, pct).map_err(to_napi_err)?;

            return serde_json::to_string_pretty(&analysis)
                .map_err(|e| return napi::Error::from_reason(e.to_string()));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Build a bounded page of timeline entries as pretty JSON.
    #[napi]
    pub async fn timeline_page(
        &self,
        offset: Option<u32>,
        limit: Option<u32>,
        include_hidden: Option<bool>,
        include_controls: Option<bool>,
    ) -> napi::Result<String> {
        let session_path = PathBuf::from(&self.session_path);
        let off = u64::from(offset.unwrap_or(0));
        let lim = u16::try_from(limit.unwrap_or(50)).unwrap_or(50);
        let hidden = include_hidden.unwrap_or(false);
        let controls = include_controls.unwrap_or(false);

        return tokio::task::spawn_blocking(move || {
            let page =
                AnalysisService::timeline_page(&session_path, hidden, controls, None, off, lim)
                    .map_err(to_napi_err)?;

            return serde_json::to_string_pretty(&page)
                .map_err(|e| return napi::Error::from_reason(e.to_string()));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }
}

/// An active recording session owning child processes and evidence buffers.
#[napi(js_name = "Recording")]
#[derive(Clone)]
pub struct JsRecording {
    pub(crate) terminal: Terminal,
    pub(crate) session: Arc<Mutex<Option<Session>>>,
    pub(crate) session_id: String,
    pub(crate) path: String,
}

#[napi]
impl JsRecording {
    /// Unique session identifier.
    #[napi(getter)]
    pub fn session_id(&self) -> String {
        return self.session_id.clone();
    }

    /// Filesystem path to the canonical session directory.
    #[napi(getter)]
    pub fn path(&self) -> String {
        return self.path.clone();
    }

    /// Check if the recording session is still active.
    #[napi]
    pub fn is_active(&self) -> bool {
        let guard = self
            .session
            .lock()
            .unwrap_or_else(|e| return e.into_inner());
        return guard.is_some();
    }

    /// Access screen inspection capabilities.
    #[napi]
    pub fn screen(&self) -> napi::Result<JsScreen> {
        if !self.is_active() {
            return Err(napi::Error::from_reason(
                "recording has already been stopped or detached",
            ));
        }

        return Ok(JsScreen {
            terminal: self.terminal.clone(),
            session_path: self.path.clone(),
        });
    }

    /// Create an action builder for input automation.
    #[napi]
    pub fn actions(&self) -> napi::Result<JsActionBuilder> {
        if !self.is_active() {
            return Err(napi::Error::from_reason(
                "recording has already been stopped or detached",
            ));
        }

        return Ok(JsActionBuilder {
            terminal: self.terminal.clone(),
            session_path: self.path.clone(),
            atoms: Mutex::new(Vec::new()),
            options: Mutex::new(ActionOptions::default()),
        });
    }

    /// Get a logical session reference.
    #[napi]
    pub fn session(&self) -> napi::Result<JsSession> {
        if !self.is_active() {
            return Err(napi::Error::from_reason(
                "recording has already been stopped or detached",
            ));
        }

        return Ok(JsSession {
            terminal: self.terminal.clone(),
            session_path: self.path.clone(),
            session_id: self.session_id.clone(),
        });
    }

    /// Click mouse button at a terminal cell.
    #[napi]
    pub async fn pointer_click(
        &self,
        column: u32,
        row: u32,
        button: Option<String>,
        strict: Option<bool>,
    ) -> napi::Result<()> {
        let sess = self.session()?;

        return sess.pointer_click(column, row, button, strict).await;
    }

    /// Scroll mouse wheel at a terminal cell.
    #[napi]
    pub async fn pointer_scroll(
        &self,
        column: u32,
        row: u32,
        lines: Option<i32>,
        strict: Option<bool>,
    ) -> napi::Result<()> {
        let sess = self.session()?;

        return sess.pointer_scroll(column, row, lines, strict).await;
    }

    /// Report terminal focus state.
    #[napi]
    pub async fn pointer_focus(&self, focused: bool, strict: Option<bool>) -> napi::Result<()> {
        let sess = self.session()?;

        return sess.pointer_focus(focused, strict).await;
    }

    /// Atomically publish visual representations (PNG, SVG, HTML, TXT, JSON) of a state point.
    #[napi]
    pub async fn save(
        &self,
        destination: String,
        formats: Option<Vec<String>>,
        state_hash: Option<String>,
        include_hidden: Option<bool>,
    ) -> napi::Result<String> {
        let sess = self.session()?;

        return sess
            .save(destination, formats, state_hash, include_hidden)
            .await;
    }

    /// Read raw child PTY output bytes using an optional byte offset cursor.
    #[napi]
    pub async fn read_output(
        &self,
        offset: Option<u32>,
        limit: Option<u32>,
    ) -> napi::Result<JsOutputPage> {
        let sess = self.session()?;

        return sess.read_output(offset, limit).await;
    }

    /// Wait for exact child output bytes to appear in the canonical stream.
    #[napi]
    pub async fn wait_for_output(
        &self,
        pattern: String,
        timeout_ms: Option<u32>,
    ) -> napi::Result<bool> {
        let sess = self.session()?;

        return sess.wait_for_output(pattern, timeout_ms).await;
    }

    /// Read canonical event records as pretty JSON starting at an optional event sequence.
    #[napi]
    pub async fn read_events(
        &self,
        cursor: Option<u32>,
        limit: Option<u32>,
    ) -> napi::Result<String> {
        let sess = self.session()?;

        return sess.read_events(cursor, limit).await;
    }

    /// Explain VT and semantic evidence around a target state point.
    #[napi]
    pub async fn explain(
        &self,
        target_hash: Option<String>,
        context: Option<u32>,
    ) -> napi::Result<String> {
        let sess = self.session()?;

        return sess.explain(target_hash, context).await;
    }

    /// Run conservative evidence-backed rendering diagnostics on the session.
    #[napi]
    pub async fn diagnose(&self, broad_change_percent: Option<u32>) -> napi::Result<String> {
        let sess = self.session()?;

        return sess.diagnose(broad_change_percent).await;
    }

    /// Perform full triage analysis on the most recent canonical action.
    #[napi]
    pub async fn last_action(&self, broad_change_percent: Option<u32>) -> napi::Result<String> {
        let sess = self.session()?;

        return sess.last_action(broad_change_percent).await;
    }

    /// Stop the recording session and finalize evidence.
    #[napi]
    pub async fn stop(&self) -> napi::Result<()> {
        let mut session_opt = {
            let mut guard = self
                .session
                .lock()
                .unwrap_or_else(|e| return e.into_inner());
            guard.take()
        };

        if let Some(mut session) = session_opt.take() {
            tokio::task::spawn_blocking(move || {
                let _ = session.stop(Some(Duration::from_secs(5)));
            })
            .await
            .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
        }

        return Ok(());
    }

    /// Explicit resource management disposal for `await using`.
    #[napi]
    pub async fn async_dispose(&self) -> napi::Result<()> {
        return self.stop().await;
    }

    /// Detach the recording so it continues running in the background.
    #[napi]
    pub fn detach(&self) -> napi::Result<()> {
        let mut guard = self
            .session
            .lock()
            .unwrap_or_else(|e| return e.into_inner());
        return guard.take().map_or_else(
            || {
                return Err(napi::Error::from_reason(
                    "recording has already been stopped or detached",
                ));
            },
            |session| {
                let _ = session.detach();
                return Ok(());
            },
        );
    }
}

/// Primary entry point for launching and managing Termtrace recordings in Node.js.
#[napi(js_name = "Termtrace")]
#[derive(Clone)]
pub struct JsTermtrace {
    pub(crate) terminal: Terminal,
}

#[napi]
impl JsTermtrace {
    /// Create a new Termtrace instance with optional binary and evidence root paths.
    #[napi(constructor)]
    pub fn new(binary: Option<String>, evidence_root: Option<String>) -> Self {
        let bin_path = binary.map_or_else(default_termtrace_bin, PathBuf::from);
        let root_path = evidence_root.map_or_else(default_evidence_root, PathBuf::from);
        return Self {
            terminal: Terminal::new(bin_path, root_path),
        };
    }

    /// Launch a new recorded command session.
    #[napi]
    pub async fn record(
        &self,
        command: Vec<String>,
        rows: Option<u32>,
        cols: Option<u32>,
        env: Option<HashMap<String, String>>,
        cwd: Option<String>,
    ) -> napi::Result<JsRecording> {
        let terminal = self.terminal.clone();
        let cmd_os: Vec<OsString> = command.into_iter().map(OsString::from).collect();
        let rows_val = u16::try_from(rows.unwrap_or(24)).unwrap_or(24);
        let cols_val = u16::try_from(cols.unwrap_or(80)).unwrap_or(80);

        return tokio::task::spawn_blocking(move || {
            let mut builder = terminal.launch(cmd_os).geometry(cols_val, rows_val);
            if let Some(dir) = cwd {
                builder = builder.working_directory(dir);
            }
            if let Some(env_map) = env {
                for (k, v) in env_map {
                    builder = builder.set_environment(k, v.into_bytes());
                }
            }

            let session = builder.start().map_err(to_napi_err)?;
            let session_id = session.session_id().to_string();
            let path = session.path().display().to_string();

            return Ok(JsRecording {
                terminal: terminal.clone(),
                session: Arc::new(Mutex::new(Some(session))),
                session_id,
                path,
            });
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Attach to an existing session directory.
    #[napi]
    pub async fn attach(&self, session_path: String) -> napi::Result<JsSession> {
        let terminal = self.terminal.clone();
        let path = PathBuf::from(session_path);

        return tokio::task::spawn_blocking(move || {
            let session = terminal.attach(&path).map_err(to_napi_err)?;
            let session_id = session.session_id().to_string();
            let session_path = session.path().display().to_string();

            return Ok(JsSession {
                terminal,
                session_path,
                session_id,
            });
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Verify an evidence bundle (.tar.zst or directory) and return whether it passed integrity verification.
    #[napi]
    pub async fn verify_bundle(
        &self,
        bundle_path: String,
        source_session: Option<String>,
    ) -> napi::Result<bool> {
        let path = PathBuf::from(bundle_path);
        let source = source_session.map_or_else(
            || {
                return path.join("canonical");
            },
            PathBuf::from,
        );

        return tokio::task::spawn_blocking(move || {
            let policy = BundleVerificationPolicy::new(&source, BundleProfile::Forensic);
            let res = ArtifactService::verify_bundle(&path, policy);

            return Ok(res.is_ok());
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Verify a completed session's structure and evidence bindings offline.
    #[napi]
    pub async fn verify_session(
        &self,
        session_path: String,
        level: Option<String>,
    ) -> napi::Result<String> {
        let path = PathBuf::from(session_path);
        let lvl = match level
            .as_deref()
            .unwrap_or("content")
            .to_ascii_lowercase()
            .as_str()
        {
            "structure" => VerificationLevel::Structure,
            "durability" | "durability_record" | "durability-record" => {
                VerificationLevel::DurabilityRecord
            }
            "signature" => VerificationLevel::Signature,
            _ => VerificationLevel::Content,
        };

        return tokio::task::spawn_blocking(move || {
            let report = IntegrityService::verify(&path, lvl, None);

            return serde_json::to_string_pretty(&report)
                .map_err(|e| return napi::Error::from_reason(e.to_string()));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Recover the longest integrity-checked event prefix from a damaged session.
    #[napi]
    pub async fn recover_session(
        &self,
        session_path: String,
        destination: String,
    ) -> napi::Result<String> {
        let src = PathBuf::from(session_path);
        let dest = PathBuf::from(destination);

        return tokio::task::spawn_blocking(move || {
            let report = IntegrityService::recover_session(&src, &dest).map_err(to_napi_err)?;

            return serde_json::to_string_pretty(&report)
                .map_err(|e| return napi::Error::from_reason(e.to_string()));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// List registered sessions in the default user registry.
    #[napi]
    pub async fn registry_list(&self, include_all: Option<bool>) -> napi::Result<String> {
        let all = include_all.unwrap_or(false);

        return tokio::task::spawn_blocking(move || {
            let reg = RegistryService::open_default().map_err(to_napi_err)?;
            let entries = reg.list(all).map_err(to_napi_err)?;

            return serde_json::to_string_pretty(&entries)
                .map_err(|e| return napi::Error::from_reason(e.to_string()));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Resolve a friendly session name to its registered session path.
    #[napi]
    pub async fn registry_resolve(&self, name: String) -> napi::Result<Option<String>> {
        return tokio::task::spawn_blocking(move || {
            let reg = RegistryService::open_default().map_err(to_napi_err)?;
            match reg.resolve(&name) {
                Ok(entry) => {
                    return Ok(Some(entry.session_path().display().to_string()));
                }
                Err(_) => {
                    return Ok(None);
                }
            }
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Register a friendly name for an existing canonical session directory.
    #[napi]
    pub async fn registry_register(
        &self,
        name: String,
        session_path: String,
    ) -> napi::Result<String> {
        let path = PathBuf::from(session_path);

        return tokio::task::spawn_blocking(move || {
            let reg = RegistryService::open_default().map_err(to_napi_err)?;
            let entry = reg.register(&name, &path).map_err(to_napi_err)?;

            return serde_json::to_string_pretty(&entry)
                .map_err(|e| return napi::Error::from_reason(e.to_string()));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Remove a friendly name binding.
    #[napi]
    pub async fn registry_remove(&self, name: String) -> napi::Result<bool> {
        return tokio::task::spawn_blocking(move || {
            let reg = RegistryService::open_default().map_err(to_napi_err)?;
            let res = reg.remove(&name);

            return Ok(res.is_ok());
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Plan or execute garbage collection over completed sessions.
    #[napi]
    pub async fn registry_gc(
        &self,
        older_than_days: Option<u32>,
        keep_latest: Option<u32>,
        max_completed_bytes: Option<u32>,
        apply: Option<bool>,
    ) -> napi::Result<String> {
        let days = u64::from(older_than_days.unwrap_or(30));
        let keep = u64::from(keep_latest.unwrap_or(10));
        let quota = max_completed_bytes.map(u64::from);
        let is_apply = apply.unwrap_or(false);

        return tokio::task::spawn_blocking(move || {
            let reg = RegistryService::open_default().map_err(to_napi_err)?;
            let report = reg.gc(days, keep, quota, is_apply).map_err(to_napi_err)?;

            return serde_json::to_string_pretty(&report)
                .map_err(|e| return napi::Error::from_reason(e.to_string()));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Bundle an aggregate tmux recording into an evidentiary streaming bundle.
    #[napi]
    pub async fn tmux_bundle(
        &self,
        manifest_path: String,
        destination: String,
    ) -> napi::Result<String> {
        let manifest = PathBuf::from(manifest_path);
        let dest = PathBuf::from(destination);

        return tokio::task::spawn_blocking(move || {
            let report = TmuxService::bundle(&manifest, &dest).map_err(to_napi_err)?;

            return serde_json::to_string_pretty(&report)
                .map_err(|e| return napi::Error::from_reason(e.to_string()));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Verify an aggregate tmux streaming bundle against its manifest.
    #[napi]
    pub async fn tmux_verify_bundle(
        &self,
        bundle_directory: String,
        manifest_path: String,
    ) -> napi::Result<bool> {
        let bundle_dir = PathBuf::from(bundle_directory);
        let manifest = PathBuf::from(manifest_path);

        return tokio::task::spawn_blocking(move || {
            let res = TmuxService::verify_bundle(&bundle_dir, &manifest);

            return Ok(res.is_ok());
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Probe runtime system capabilities across engine, media, and evidence signers.
    #[napi]
    pub async fn probe_capabilities(&self) -> napi::Result<String> {
        return tokio::task::spawn_blocking(move || {
            let report = SystemService::probe_capabilities();

            return serde_json::to_string_pretty(&report)
                .map_err(|e| return napi::Error::from_reason(e.to_string()));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }
}
