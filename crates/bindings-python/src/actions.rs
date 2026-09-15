//! Fluent action builder for executing state-guarded terminal input in Python.

use crate::errors::to_py_err;
use crate::screen::PyResolvedState;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use std::time::Duration;
use termtrace_model::protocol::control::ScreenPredicate;
use termtrace_model::terminal::state::{KeyModifiers, NamedKey};
use termtrace_sdk::actions::{ActionCondition, ActionOptions, InputAtom};
use termtrace_sdk::client::Terminal;
use termtrace_sdk::screen::frame_text;

fn parse_key(key: &str) -> PyResult<(NamedKey, KeyModifiers)> {
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
                .ok_or_else(|| return PyValueError::new_err("empty key string"))?;

            NamedKey::Letter(c)
        }
        _ => return Err(PyValueError::new_err(format!("unknown key name: '{key}'"))),
    };

    return Ok((named, modifiers));
}

/// Fluent builder for atomic, state-guarded input sequences.
#[pyclass(name = "ActionBuilder")]
pub struct PyActionBuilder {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: String,
    pub(crate) atoms: Vec<InputAtom>,
    pub(crate) options: ActionOptions,
}

#[pymethods]
impl PyActionBuilder {
    /// Enforce that the terminal must be at the specified state hash before executing.
    pub fn if_state(mut slf: PyRefMut<'_, Self>, state_hash: String) -> PyRefMut<'_, Self> {
        slf.options.if_state = Some(state_hash);
        slf
    }

    /// Append UTF-8 text to send to the terminal.
    pub fn send_text(mut slf: PyRefMut<'_, Self>, text: String) -> PyRefMut<'_, Self> {
        slf.atoms.push(InputAtom::Text(text));
        slf
    }

    /// Append a symbolic named key (e.g. 'Enter', 'Ctrl-C', 'Tab').
    pub fn send_key(mut slf: PyRefMut<'_, Self>, key: String) -> PyResult<PyRefMut<'_, Self>> {
        let (named, modifiers) = parse_key(&key)?;
        slf.atoms.push(InputAtom::Key {
            key: named,
            modifiers,
        });
        Ok(slf)
    }

    /// Append exact raw bytes to send to the terminal.
    pub fn send_bytes(mut slf: PyRefMut<'_, Self>, bytes: Vec<u8>) -> PyRefMut<'_, Self> {
        slf.atoms.push(InputAtom::Bytes(bytes));
        slf
    }

    /// Send bracketed paste text.
    pub fn paste(mut slf: PyRefMut<'_, Self>, text: String) -> PyRefMut<'_, Self> {
        slf.atoms.push(InputAtom::Paste(text));
        slf
    }

    /// Set delay pacing between consecutive action atoms in milliseconds.
    pub fn pace(mut slf: PyRefMut<'_, Self>, delay_ms: u64) -> PyRefMut<'_, Self> {
        slf.options.pace = Duration::from_millis(delay_ms);

        return slf;
    }

    /// Wait until the screen contains the specified pattern after applying input.
    #[pyo3(signature = (pattern, timeout_secs = None))]
    pub fn until_screen(
        mut slf: PyRefMut<'_, Self>,
        pattern: String,
        timeout_secs: Option<f64>,
    ) -> PyRefMut<'_, Self> {
        let timeout = Duration::from_secs_f64(timeout_secs.unwrap_or(10.0));
        slf.options.postcondition = ActionCondition::Screen {
            predicate: ScreenPredicate::Contains { text: pattern },
            timeout,
        };
        slf
    }

    /// Wait until the screen is idle and unchanged after applying input.
    #[pyo3(signature = (stable_for_ms = None, timeout_secs = None))]
    pub fn until_idle(
        mut slf: PyRefMut<'_, Self>,
        stable_for_ms: Option<u64>,
        timeout_secs: Option<f64>,
    ) -> PyRefMut<'_, Self> {
        let stable_for = Duration::from_millis(stable_for_ms.unwrap_or(20));
        let timeout = Duration::from_secs_f64(timeout_secs.unwrap_or(10.0));
        slf.options.postcondition = ActionCondition::Idle {
            stable_for,
            timeout,
        };
        slf
    }

    /// Execute the action sequence and return the terminating screen frame.
    pub fn execute(&mut self, py: Python<'_>) -> PyResult<Option<PyResolvedState>> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();
        let atoms = std::mem::take(&mut self.atoms);
        let options = self.options.clone();

        py.detach(move || {
            let session = terminal.attach(path).map_err(to_py_err)?;
            let result = session.act(&atoms, options).map_err(to_py_err)?;
            if let Some(frame) = result.frame {
                let text = frame_text(&frame);
                Ok(Some(PyResolvedState {
                    hash: frame.state_hash,
                    text,
                    cursor_row: frame.cursor.row.unwrap_or(0),
                    cursor_col: frame.cursor.column.unwrap_or(0),
                }))
            } else {
                Ok(None)
            }
        })
    }
}
