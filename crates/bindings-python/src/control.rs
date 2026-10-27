//! Typed pointer, session-control, ordered-send, status, and launch facades for Python.

use crate::actions::{request_id_arg, PyActionResult};
use crate::errors::{mutation_err, send_err, to_py_err};
use crate::host::{to_python, Arg, Model};
use crate::screen::Framed;
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;
use pyo3_stub_gen::derive::{gen_stub_pyclass, gen_stub_pymethods};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use termtrace_model::protocol::control::ProcessSignal;
use termtrace_model::terminal::state::MouseButton;
use termtrace_sdk::client::{Session, Terminal};
use termtrace_sdk::contracts::control::guard;
use termtrace_sdk::contracts::{
    CommandOutcome, ConditionSpec, ErrorReport, LaunchOptions, ProjectionSpec, SendSpec,
    SendStepSpec,
};
use termtrace_sdk::input::PointerOutcome;
use termtrace_sdk::screen::{FrameDetail, ScreenRegion};
use termtrace_sdk::session::{
    CommandPostcondition, ControlAcknowledgement, MarkOutcome, SendLastStep, SendOutcome,
    SessionStatus,
};
use termtrace_sdk::Uuid;

/// Parse a mouse button name: `left`, `middle`, `right`, `wheel_up`, or `wheel_down`.
///
/// # Errors
///
/// Raises `ValueError` for any other name.
pub fn parse_button(button: Option<&str>) -> PyResult<MouseButton> {
    return match button.unwrap_or("left").to_ascii_lowercase().as_str() {
        "left" => Ok(MouseButton::Left),
        "right" => Ok(MouseButton::Right),
        "middle" => Ok(MouseButton::Middle),
        "wheel_up" | "wheelup" => Ok(MouseButton::WheelUp),
        "wheel_down" | "wheeldown" => Ok(MouseButton::WheelDown),
        other => Err(PyValueError::new_err(format!(
            "unknown mouse button: '{other}'"
        ))),
    };
}

fn attached<T>(
    py: Python<'_>,
    terminal: &Terminal,
    path: &Path,
    request_id: Uuid,
    call: impl FnOnce(&Session) -> termtrace_sdk::Result<T> + Send,
) -> PyResult<T>
where
    T: Send,
{
    return py.detach(|| {
        return terminal
            .attach(path)
            .and_then(|session| return call(&session))
            .map_err(|error| return mutation_err(error, request_id));
    });
}

/// Mode-aware pointer, focus, and text-activation operations with typed results.
///
/// Each call is one mutation; pass `request_id` to retry it safely with the same identity.
#[gen_stub_pyclass]
#[pyclass(name = "Pointer", frozen)]
pub struct PyPointer {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: PathBuf,
}

impl PyPointer {
    fn apply(
        &self,
        py: Python<'_>,
        request_id: Option<&Bound<'_, PyAny>>,
        if_state: Option<String>,
        call: impl FnOnce(&Session, &termtrace_sdk::MutationGuard) -> termtrace_sdk::Result<PointerOutcome>
            + Send,
    ) -> PyResult<Model<PointerOutcome>> {
        let mutation = guard(request_id_arg(request_id)?, if_state);
        let request_id = mutation.request_id.unwrap_or_default();
        let outcome = attached(
            py,
            &self.terminal,
            &self.session_path,
            request_id,
            |session| {
                return call(session, &mutation);
            },
        )?;

        return Ok(Model(outcome));
    }
}

#[gen_stub_pymethods]
#[pymethods]
impl PyPointer {
    /// Press and release one button at a zero-based cell.
    ///
    /// # Errors
    ///
    /// Raises `ValueError` for an unknown button and `TermtraceError` or a subclass, with
    /// `request_id`, when the recorder rejects the click (with `strict`, when the application does
    /// not report mouse input).
    #[pyo3(signature = (column, row, button = None, *, strict = false, request_id = None, if_state = None))]
    pub fn click(
        &self,
        py: Python<'_>,
        column: u16,
        row: u16,
        button: Option<&str>,
        strict: bool,
        request_id: Option<&Bound<'_, PyAny>>,
        if_state: Option<String>,
    ) -> PyResult<Model<PointerOutcome>> {
        let button = parse_button(button)?;

        return self.apply(py, request_id, if_state, |session, mutation| {
            let request_id = mutation.request_id.unwrap_or_default();
            return session
                .pointer()
                .click(column, row, button, strict, mutation)
                .and_then(|payload| return PointerOutcome::from_payload(request_id, payload));
        });
    }

    /// Emit `count` complete clicks at a zero-based cell.
    ///
    /// # Errors
    ///
    /// Raises as `click`.
    #[pyo3(signature = (column, row, count, button = None, *, strict = false, request_id = None, if_state = None))]
    pub fn multi_click(
        &self,
        py: Python<'_>,
        column: u16,
        row: u16,
        count: u8,
        button: Option<&str>,
        strict: bool,
        request_id: Option<&Bound<'_, PyAny>>,
        if_state: Option<String>,
    ) -> PyResult<Model<PointerOutcome>> {
        let button = parse_button(button)?;

        return self.apply(py, request_id, if_state, |session, mutation| {
            return session
                .pointer()
                .multi_click(column, row, button, count, strict, mutation);
        });
    }

    /// Press at a zero-based cell, emit `steps` motion samples, and release at the end cell.
    ///
    /// # Errors
    ///
    /// Raises as `click`.
    #[pyo3(signature = (column, row, to_column, to_row, button = None, *, steps = 1, strict = false, request_id = None, if_state = None))]
    pub fn drag(
        &self,
        py: Python<'_>,
        column: u16,
        row: u16,
        to_column: u16,
        to_row: u16,
        button: Option<&str>,
        steps: u8,
        strict: bool,
        request_id: Option<&Bound<'_, PyAny>>,
        if_state: Option<String>,
    ) -> PyResult<Model<PointerOutcome>> {
        let button = parse_button(button)?;

        return self.apply(py, request_id, if_state, |session, mutation| {
            return session.pointer().drag(
                column, row, to_column, to_row, button, steps, strict, mutation,
            );
        });
    }

    /// Emit signed vertical wheel ticks at a zero-based cell; negative scrolls up.
    ///
    /// # Errors
    ///
    /// Raises as `click`.
    #[pyo3(signature = (column, row, lines = 1, *, strict = false, request_id = None, if_state = None))]
    pub fn scroll(
        &self,
        py: Python<'_>,
        column: u16,
        row: u16,
        lines: i16,
        strict: bool,
        request_id: Option<&Bound<'_, PyAny>>,
        if_state: Option<String>,
    ) -> PyResult<Model<PointerOutcome>> {
        return self.apply(py, request_id, if_state, |session, mutation| {
            let request_id = mutation.request_id.unwrap_or_default();
            return session
                .pointer()
                .scroll(column, row, lines, strict, mutation)
                .and_then(|payload| return PointerOutcome::from_payload(request_id, payload));
        });
    }

    /// Report a focus transition when the application enabled focus reporting.
    ///
    /// # Errors
    ///
    /// Raises as `click`.
    #[pyo3(signature = (focused, *, strict = false, request_id = None, if_state = None))]
    pub fn focus(
        &self,
        py: Python<'_>,
        focused: bool,
        strict: bool,
        request_id: Option<&Bound<'_, PyAny>>,
        if_state: Option<String>,
    ) -> PyResult<Model<PointerOutcome>> {
        return self.apply(py, request_id, if_state, |session, mutation| {
            let request_id = mutation.request_id.unwrap_or_default();
            return session
                .pointer()
                .focus(focused, strict, mutation)
                .and_then(|payload| return PointerOutcome::from_payload(request_id, payload));
        });
    }

    /// Resolve visible row-contained text and click its center from one actor-ordered state.
    ///
    /// # Errors
    ///
    /// Raises as `click`; the codes `activation_text_not_found`, `activation_text_concealed`, and
    /// `activation_text_ambiguous` report resolution failures.
    #[pyo3(signature = (text, *, unique = false, strict = false, request_id = None, if_state = None))]
    pub fn activate(
        &self,
        py: Python<'_>,
        text: String,
        unique: bool,
        strict: bool,
        request_id: Option<&Bound<'_, PyAny>>,
        if_state: Option<String>,
    ) -> PyResult<Model<PointerOutcome>> {
        return self.apply(py, request_id, if_state, |session, mutation| {
            return session.pointer().activate(text, unique, strict, mutation);
        });
    }
}

/// Resize the child PTY.
///
/// # Errors
///
/// Raises `TermtraceError` or a subclass with `request_id` when the recorder rejects it.
pub fn resize(
    py: Python<'_>,
    terminal: &Terminal,
    path: &Path,
    cols: u16,
    rows: u16,
    request_id: Option<&Bound<'_, PyAny>>,
    if_state: Option<String>,
) -> PyResult<Model<ControlAcknowledgement>> {
    let mutation = guard(request_id_arg(request_id)?, if_state);
    let id = mutation.request_id.unwrap_or_default();
    let outcome = attached(py, terminal, path, id, |session| {
        return session.resize(cols, rows, &mutation);
    })?;

    return Ok(Model(outcome));
}

/// Send one allow-listed signal to the child process group.
///
/// # Errors
///
/// Raises `ValueError` for an unknown signal and `TermtraceError` or a subclass with
/// `request_id` when the recorder rejects it.
pub fn signal(
    py: Python<'_>,
    terminal: &Terminal,
    path: &Path,
    signal: Arg<ProcessSignal>,
    request_id: Option<&Bound<'_, PyAny>>,
    if_state: Option<String>,
) -> PyResult<Model<ControlAcknowledgement>> {
    let signal = signal.0;
    let mutation = guard(request_id_arg(request_id)?, if_state);
    let id = mutation.request_id.unwrap_or_default();
    let outcome = attached(py, terminal, path, id, |session| {
        return session.signal(signal, &mutation);
    })?;

    return Ok(Model(outcome));
}

/// Append a canonical marker.
///
/// # Errors
///
/// Raises `TermtraceError` or a subclass with `request_id` when the recorder rejects it.
pub fn mark(
    py: Python<'_>,
    terminal: &Terminal,
    path: &Path,
    label: String,
    request_id: Option<&Bound<'_, PyAny>>,
    if_state: Option<String>,
) -> PyResult<Model<MarkOutcome>> {
    let mutation = guard(request_id_arg(request_id)?, if_state);
    let id = mutation.request_id.unwrap_or_default();
    let outcome = attached(py, terminal, path, id, |session| {
        return session.mark(label, &mutation);
    })?;

    return Ok(Model(outcome));
}

/// Read the live status or the completion of a session.
///
/// # Errors
///
/// Raises `TermtraceError` or a subclass when the session cannot be attached or read.
pub fn status(py: Python<'_>, terminal: &Terminal, path: &Path) -> PyResult<Model<SessionStatus>> {
    let status = py.detach(|| {
        return terminal
            .attach(path)
            .and_then(|session| return session.status())
            .map_err(to_py_err);
    })?;

    return Ok(Model(status));
}

/// Options of one ordered send, gathered from keyword arguments.
pub struct SendOptions<'py> {
    /// Identity of the whole send.
    pub request_id: Option<Bound<'py, PyAny>>,
    /// Presented-state guard of the first step.
    pub if_state: Option<String>,
    /// Caller intent retained with input operations.
    pub intent: Option<String>,
    /// Delay between input atoms.
    pub pace_ms: Option<u64>,
    /// Report per-atom outcomes of input steps.
    pub verbose: bool,
    /// Condition checked after the last step.
    pub until: Option<Arg<ConditionSpec>>,
    /// Terminating-frame detail.
    pub detail: Option<Arg<FrameDetail>>,
    /// Terminating-frame region.
    pub region: Option<Arg<ScreenRegion>>,
    /// Report presented-screen changes across the send.
    pub report_changes: bool,
}

fn send_spec(steps: Vec<SendStepSpec>, options: SendOptions<'_>) -> PyResult<SendSpec> {
    return Ok(SendSpec {
        steps,
        request_id: request_id_arg(options.request_id.as_ref())?,
        if_state: options.if_state,
        intent: options.intent,
        pace_ms: options.pace_ms,
        verbose: options.verbose,
        until: options.until.map(|until| return until.0),
        projection: Some(ProjectionSpec {
            detail: options.detail.map(|detail| return detail.0),
            region: options.region.map(|region| return region.0),
        }),
        report_changes: options.report_changes,
    });
}

fn send_result(
    py: Python<'_>,
    send_request_id: Uuid,
    last_step: usize,
    outcome: SendOutcome,
) -> PyResult<Py<PyAny>> {
    let last = PyDict::new(py);
    last.set_item("step", last_step)?;
    match outcome.last {
        SendLastStep::Action(action) => {
            last.set_item("kind", "action")?;
            last.set_item("request_id", action.request_id.to_string())?;
            last.set_item("result", PyActionResult::new(Arc::new(*action)))?;
        }
        SendLastStep::Command {
            request_id,
            payload,
            postcondition,
        } => {
            last.set_item("kind", "command")?;
            last.set_item("request_id", request_id.to_string())?;
            let command = CommandOutcome::from_payload(request_id, &payload).map_err(to_py_err)?;
            last.set_item("outcome", Model(command))?;
            let checked = match postcondition.map(|postcondition| return *postcondition) {
                None => py.None(),
                Some(CommandPostcondition::Idle(Ok(capture))) => {
                    let checked = PyDict::new(py);
                    checked.set_item("kind", "idle")?;
                    checked.set_item("capture", Framed::from(capture))?;

                    checked.into_any().unbind()
                }
                Some(CommandPostcondition::Screen(Ok(wait))) => {
                    let checked = PyDict::new(py);
                    checked.set_item("kind", "screen")?;
                    checked.set_item("wait", Framed::from(wait))?;

                    checked.into_any().unbind()
                }
                Some(CommandPostcondition::Idle(Err(error))) => {
                    let checked = PyDict::new(py);
                    checked.set_item("kind", "idle")?;
                    checked.set_item("error", Model(ErrorReport::from(&error)))?;

                    checked.into_any().unbind()
                }
                Some(CommandPostcondition::Screen(Err(error))) => {
                    let checked = PyDict::new(py);
                    checked.set_item("kind", "screen")?;
                    checked.set_item("error", Model(ErrorReport::from(&error)))?;

                    checked.into_any().unbind()
                }
            };
            last.set_item("postcondition", checked)?;
        }
    }

    let result = PyDict::new(py);
    result.set_item("request_id", send_request_id.to_string())?;
    result.set_item("last", last)?;
    result.set_item("changes", to_python(py, &outcome.changes)?)?;

    return Ok(result.into_any().unbind());
}

/// Apply ordered steps, each a separate mutation with an identity derived from the send's.
///
/// # Errors
///
/// Raises `ValueError` for malformed steps or options, and on a failed step the step's
/// `TermtraceError` with `step`, its derived `request_id`, and `send_request_id`; earlier steps
/// were applied.
pub fn send(
    py: Python<'_>,
    terminal: &Terminal,
    path: &Path,
    steps: Vec<Arg<SendStepSpec>>,
    options: SendOptions<'_>,
) -> PyResult<Py<PyAny>> {
    let steps = steps.into_iter().map(|step| return step.0).collect();
    let request = send_spec(steps, options)?.to_request().map_err(to_py_err)?;
    let send_request_id = request.guard.request_id.unwrap_or_default();
    let last_step = request.steps.len().saturating_sub(1);
    let outcome = py.detach(|| {
        let session = terminal
            .attach(path)
            .map_err(|error| return mutation_err(error, send_request_id))?;

        return session
            .send(request)
            .map_err(|failure| return send_err(failure, send_request_id));
    })?;

    return send_result(py, send_request_id, last_step, outcome);
}

/// Launch options decoded from a host mapping.
///
/// # Errors
///
/// Raises `ValueError` for unknown or malformed options and `TermtraceError`
/// (`invalid_operation`) for an invalid command, geometry, environment, or limit.
pub fn launch_spec(
    command: Vec<String>,
    options: Option<Arg<LaunchOptions>>,
) -> PyResult<termtrace_sdk::LaunchSpec> {
    let options = options.map(|options| return options.0).unwrap_or_default();

    return launch_spec_from(command, &options);
}

/// Launch specification for already decoded options.
///
/// # Errors
///
/// Raises `TermtraceError` (`invalid_operation`) for an invalid command, geometry, environment, or
/// limit.
pub fn launch_spec_from(
    command: Vec<String>,
    options: &LaunchOptions,
) -> PyResult<termtrace_sdk::LaunchSpec> {
    return options
        .to_spec(command.into_iter().map(OsString::from).collect())
        .map_err(to_py_err);
}
