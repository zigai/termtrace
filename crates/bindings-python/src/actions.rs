//! Fluent action builder for executing state-guarded terminal input in Python.

use crate::errors::{mutation_err, to_py_err};
use crate::host::{Arg, Model};
use crate::screen::{PyFrame, PyResolvedState};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyString;
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use termtrace_model::protocol::control::ActionInputResult;
use termtrace_model::protocol::control::MAX_CONTROL_WAIT;
use termtrace_model::terminal::observation::ActorBoundaries;
use termtrace_sdk::client::Terminal;
use termtrace_sdk::contracts::{control::parse_request_id, ActionDraft, PostconditionSummary};
use termtrace_sdk::defaults;
use termtrace_sdk::input::{ActOutcome, ActRequest, ActionCondition, KeyChord, Reaction};
use termtrace_sdk::screen::{FrameDetail, ScreenPredicate, ScreenProjectionRequest, ScreenRegion};
use termtrace_sdk::Uuid;

/// Parse one key chord with the shared grammar, e.g. `Enter`, `ctrl+c`, `ctrl+shift+F5`.
///
/// # Errors
///
/// Returns `ValueError` when `key` is not a valid key chord or names an unknown key.
pub fn parse_key(key: &str) -> PyResult<KeyChord> {
    return KeyChord::parse(key)
        .map_err(|error| return PyValueError::new_err(error.to_string()))?
        .ok_or_else(|| {
            return PyValueError::new_err(format!(
                "unknown key name: '{key}'; send plain characters as text"
            ));
        });
}

/// Parse a caller-owned request identity given as `str` or `uuid.UUID`.
///
/// # Errors
///
/// Raises `TypeError` for other types and `TermtraceError` (`invalid_operation`) for text that is
/// not a hyphenated non-nil UUID.
pub fn request_id_arg(value: Option<&Bound<'_, PyAny>>) -> PyResult<Option<Uuid>> {
    let Some(value) = value.filter(|value| return !value.is_none()) else {
        return Ok(None);
    };
    let text = if let Ok(text) = value.cast::<PyString>() {
        text.to_str()?.to_owned()
    } else if value.is_instance(&value.py().import("uuid")?.getattr("UUID")?)? {
        value.str()?.to_str()?.to_owned()
    } else {
        return Err(PyTypeError::new_err(
            "request_id must be a str or uuid.UUID",
        ));
    };

    return parse_request_id(&text).map(Some).map_err(to_py_err);
}

/// Validate whole milliseconds against the control wait bound.
///
/// # Errors
///
/// Raises `ValueError` above the bound.
pub fn wait_millis(milliseconds: u64) -> PyResult<Duration> {
    let duration = Duration::from_millis(milliseconds);
    if duration > MAX_CONTROL_WAIT {
        return Err(PyValueError::new_err(format!(
            "duration {milliseconds} ms exceeds the maximum of {} ms",
            MAX_CONTROL_WAIT.as_millis()
        )));
    }
    return Ok(duration);
}

/// Complete outcome of one action: input application and postcondition, reported separately.
#[gen_stub_pyclass]
#[pyclass(name = "ActionResult", frozen)]
pub struct PyActionResult {
    outcome: Arc<ActOutcome>,
}

impl PyActionResult {
    /// Wrap one exact outcome.
    pub fn new(outcome: Arc<ActOutcome>) -> Self {
        return Self { outcome };
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl PyActionResult {
    /// Identity of the action request.
    #[getter]
    fn request_id(&self) -> String {
        return self.outcome.request_id.to_string();
    }

    /// Exact input application outcome; counts are wire atoms (paced text is one per scalar).
    #[getter]
    fn input(&self) -> Model<ActionInputResult> {
        return Model(self.outcome.input.clone());
    }

    /// Postcondition outcome without its frame: `kind`, `matched`, `quality`, `state_hash`.
    #[getter]
    fn postcondition(&self) -> Model<PostconditionSummary> {
        return Model(PostconditionSummary::from(&self.outcome.postcondition));
    }

    /// Exact terminating frame, or `None` when none was projected.
    #[getter]
    fn frame(&self) -> Option<PyFrame> {
        return self
            .outcome
            .frame()
            .map(|frame| return PyFrame::new(Arc::new(frame.clone())));
    }

    /// Whether the postcondition held; input is applied either way.
    #[getter]
    fn matched(&self) -> bool {
        return self.outcome.is_matched();
    }

    /// Terminating presented-state hash, or `None` when the actor reported none.
    #[getter]
    fn state_hash(&self) -> Option<String> {
        return self.outcome.state_hash().map(str::to_owned);
    }

    /// Canonical positions enclosing the action.
    #[getter]
    fn boundaries(&self) -> Model<ActorBoundaries> {
        return Model(self.outcome.boundaries.clone());
    }

    /// Portable JSON: every u64 field, including the nested frame's, is a decimal string.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when the report cannot be encoded.
    pub fn to_json(&self) -> PyResult<String> {
        return serde_json::to_string(self.outcome.as_ref())
            .map_err(|error| return PyValueError::new_err(error.to_string()));
    }

    /// Decode portable JSON, rejecting numeric, signed, or non-canonical wide fields and any
    /// inconsistent summary.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `json` is not a valid action report.
    #[staticmethod]
    pub fn from_json(json: &str) -> PyResult<Self> {
        return serde_json::from_str::<ActOutcome>(json)
            .map(|outcome| return Self::new(Arc::new(outcome)))
            .map_err(|error| return PyValueError::new_err(error.to_string()));
    }

    fn __repr__(&self) -> String {
        return format!(
            "ActionResult(request_id='{}', atoms={}, matched={})",
            self.outcome.request_id,
            self.outcome.input.atom_count,
            if self.outcome.is_matched() { "True" } else { "False" }
        );
    }
}

/// Fluent builder for atomic, state-guarded input sequences.
///
/// The request identity exists from construction. `run()` seals the exact request, so every
/// later `run()` is a same-identity retry and every change raises `invalid_operation`. Legacy
/// `execute()` sends the current request as a fresh one and starts over with a new identity.
#[gen_stub_pyclass]
#[pyclass(name = "ActionBuilder", frozen)]
pub struct PyActionBuilder {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: PathBuf,
    draft: Mutex<ActionDraft>,
}

impl PyActionBuilder {
    /// Start an empty action against one session.
    pub(crate) fn new(terminal: Terminal, session_path: PathBuf) -> Self {
        return Self {
            terminal,
            session_path,
            draft: Mutex::new(ActionDraft::new()),
        };
    }

    fn draft(&self) -> MutexGuard<'_, ActionDraft> {
        return self
            .draft
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner());
    }

    fn edit<'builder>(
        slf: PyRef<'builder, Self>,
        change: impl FnOnce(&mut ActRequest),
    ) -> PyResult<PyRef<'builder, Self>> {
        slf.draft().edit(change).map_err(to_py_err)?;

        return Ok(slf);
    }

    fn act(&self, py: Python<'_>, request: ActRequest) -> PyResult<ActOutcome> {
        let request_id = request.guard.request_id.unwrap_or_default();

        return py.detach(|| {
            return self
                .terminal
                .attach(&self.session_path)
                .and_then(|session| return session.act(request))
                .map_err(|error| return mutation_err(error, request_id));
        });
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl PyActionBuilder {
    /// Identity of the next send; readable before any `run()` and stable once sealed.
    #[getter]
    fn request_id(&self) -> String {
        return self.draft().request_id().to_string();
    }

    /// Whether `run()` has frozen the request.
    #[getter]
    fn sealed(&self) -> bool {
        return self.draft().is_sealed();
    }

    /// Replace the pre-allocated request identity before sealing.
    ///
    /// # Errors
    ///
    /// Raises `TypeError` for a value that is not `str` or `uuid.UUID`, and `TermtraceError`
    /// (`invalid_operation`) for an invalid UUID or a sealed builder.
    pub fn with_request_id<'builder>(
        slf: PyRef<'builder, Self>,
        request_id: &Bound<'_, PyAny>,
    ) -> PyResult<PyRef<'builder, Self>> {
        let request_id = request_id_arg(Some(request_id))?.ok_or_else(|| {
            return PyTypeError::new_err("request_id must be a str or uuid.UUID");
        })?;
        slf.draft().set_request_id(request_id).map_err(to_py_err)?;

        return Ok(slf);
    }

    /// Enforce that the terminal must be at the specified state hash before executing.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` (`invalid_operation`) once sealed.
    pub fn if_state(slf: PyRef<'_, Self>, state_hash: String) -> PyResult<PyRef<'_, Self>> {
        return Self::edit(slf, |request| request.guard.if_state = Some(state_hash));
    }

    /// Append UTF-8 text to send to the terminal.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` (`invalid_operation`) once sealed.
    pub fn send_text(slf: PyRef<'_, Self>, text: String) -> PyResult<PyRef<'_, Self>> {
        return Self::edit(slf, |request| {
            *request = std::mem::take(request).text(text);
        });
    }

    /// Append a key chord (e.g. 'Enter', 'ctrl+c', 'ctrl+shift+F5').
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `key` is not a valid key chord or names an unknown key, and
    /// `TermtraceError` (`invalid_operation`) once sealed.
    pub fn send_key(slf: PyRef<'_, Self>, key: String) -> PyResult<PyRef<'_, Self>> {
        let chord = parse_key(&key)?;
        return Self::edit(slf, |request| *request = std::mem::take(request).key(chord));
    }

    /// Append exact raw bytes (`bytes`, `bytearray`, or `memoryview`) to send to the terminal.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` (`invalid_operation`) once sealed.
    pub fn send_bytes(slf: PyRef<'_, Self>, bytes: Vec<u8>) -> PyResult<PyRef<'_, Self>> {
        return Self::edit(slf, |request| {
            *request = std::mem::take(request).bytes(bytes);
        });
    }

    /// Send bracketed paste text.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` (`invalid_operation`) once sealed.
    pub fn paste(slf: PyRef<'_, Self>, text: String) -> PyResult<PyRef<'_, Self>> {
        return Self::edit(slf, |request| {
            *request = std::mem::take(request).paste(text);
        });
    }

    /// Set delay pacing between consecutive action atoms in milliseconds; paced text is typed
    /// one character at a time.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` (`invalid_operation`) once sealed.
    pub fn pace(slf: PyRef<'_, Self>, delay_ms: u64) -> PyResult<PyRef<'_, Self>> {
        return Self::edit(slf, |request| {
            request.pace = Duration::from_millis(delay_ms);
        });
    }

    /// Attach a human-readable caller intent retained with the input operations.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` (`invalid_operation`) once sealed.
    pub fn intent(slf: PyRef<'_, Self>, intent: String) -> PyResult<PyRef<'_, Self>> {
        return Self::edit(slf, |request| request.intent = Some(intent));
    }

    /// Report per-wire-atom outcomes in `ActionResult.input["atoms"]`.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` (`invalid_operation`) once sealed.
    #[pyo3(signature = (enabled = true))]
    pub fn verbose(slf: PyRef<'_, Self>, enabled: bool) -> PyResult<PyRef<'_, Self>> {
        return Self::edit(slf, |request| request.verbose_atom_outcomes = enabled);
    }

    /// Return an unmet postcondition as `matched == False` instead of raising
    /// `postcondition_incomplete`.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` (`invalid_operation`) once sealed.
    #[pyo3(signature = (enabled = true))]
    pub fn allow_incomplete(slf: PyRef<'_, Self>, enabled: bool) -> PyResult<PyRef<'_, Self>> {
        return Self::edit(slf, |request| request.allow_incomplete = enabled);
    }

    /// Request the terminating frame's detail and half-open region.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` for an invalid detail or region, and `TermtraceError`
    /// (`invalid_operation`) once sealed.
    #[pyo3(signature = (detail = None, region = None))]
    pub fn projection<'builder>(
        slf: PyRef<'builder, Self>,
        detail: Option<Arg<FrameDetail>>,
        region: Option<Arg<ScreenRegion>>,
    ) -> PyResult<PyRef<'builder, Self>> {
        let projection = ScreenProjectionRequest {
            detail: detail.map(|detail| return detail.0).unwrap_or_default(),
            region: region.map(|region| return region.0),
        };
        return Self::edit(slf, |request| request.projection = projection);
    }

    /// Wait until the screen contains the specified pattern after applying input.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when `timeout_secs` is negative, not finite, or out of range, and
    /// `TermtraceError` (`invalid_operation`) once sealed.
    #[pyo3(signature = (pattern, timeout_secs = None))]
    pub fn until_screen(
        slf: PyRef<'_, Self>,
        pattern: String,
        timeout_secs: Option<f64>,
    ) -> PyResult<PyRef<'_, Self>> {
        let timeout = crate::parse_timeout(timeout_secs, defaults::WAIT_TIMEOUT)?;
        return Self::edit(slf, |request| {
            request.postcondition = ActionCondition::Screen {
                predicate: ScreenPredicate::Contains { text: pattern },
                timeout,
                settle: Duration::ZERO,
            };
        });
    }

    /// Wait after applying input until a presented-screen predicate holds.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` for an invalid predicate or a timeout above the wait bound, and
    /// `TermtraceError` (`invalid_operation`) once sealed.
    #[pyo3(signature = (predicate, timeout_ms = None))]
    pub fn until_screen_where<'builder>(
        slf: PyRef<'builder, Self>,
        predicate: Arg<ScreenPredicate>,
        timeout_ms: Option<u64>,
    ) -> PyResult<PyRef<'builder, Self>> {
        let predicate = predicate.0;
        let timeout = timeout_ms.map_or(Ok(defaults::WAIT_TIMEOUT), wait_millis)?;
        return Self::edit(slf, |request| {
            request.postcondition = ActionCondition::Screen {
                predicate,
                timeout,
                settle: Duration::ZERO,
            };
        });
    }

    /// Wait until the screen changes after applying input and then stays unchanged; the change
    /// must start within `reaction_timeout_ms`, or the condition settles as `unchanged`.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` when a duration is negative, not finite, or out of range, and
    /// `TermtraceError` (`invalid_operation`) once sealed.
    #[pyo3(signature = (stable_for_ms = None, timeout_secs = None, reaction_timeout_ms = None))]
    pub fn until_idle(
        slf: PyRef<'_, Self>,
        stable_for_ms: Option<u64>,
        timeout_secs: Option<f64>,
        reaction_timeout_ms: Option<u64>,
    ) -> PyResult<PyRef<'_, Self>> {
        let stable_for = stable_for_ms.map_or(defaults::IDLE_STABLE_FOR, Duration::from_millis);
        let timeout = crate::parse_timeout(timeout_secs, defaults::WAIT_TIMEOUT)?;
        let reaction = Reaction {
            changed_from: None,
            timeout: reaction_timeout_ms.map_or(Ok(defaults::REACTION_TIMEOUT), wait_millis)?,
        };
        return Self::edit(slf, |request| {
            request.postcondition = ActionCondition::Idle {
                stable_for,
                timeout,
                reaction,
            };
        });
    }

    /// Freeze the exact request without sending it; idempotent.
    pub fn seal(&self) {
        let _request = self.draft().seal();
    }

    /// Seal the request and apply it, returning separate input and postcondition outcomes.
    ///
    /// Calling `run()` again resends the identical sealed request: the actor applies it at most
    /// once and returns the same `sequence_id`.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` or a subclass with `request_id`, `effect`, and `retry`; an unmet
    /// postcondition raises `postcondition_incomplete` with the applied `result` unless
    /// `allow_incomplete()` was set.
    pub fn run(&self, py: Python<'_>) -> PyResult<PyActionResult> {
        let request = self.draft().seal();
        let outcome = self.act(py, request)?;

        return Ok(PyActionResult::new(Arc::new(outcome)));
    }

    /// Legacy fresh request: send the current request with the current identity, then start over
    /// empty with a new identity; returns the terminating frame summary.
    ///
    /// # Errors
    ///
    /// Raises `TermtraceError` (`invalid_operation`) once sealed, and otherwise `TermtraceError` or
    /// a subclass when the session cannot be attached or the recorder rejects or cannot complete
    /// the queued actions.
    pub fn execute(&self, py: Python<'_>) -> PyResult<Option<PyResolvedState>> {
        let request = self.draft().take_fresh().map_err(to_py_err)?;
        let outcome = self.act(py, request)?;

        return Ok(outcome.frame().map(PyResolvedState::from_frame));
    }
}
