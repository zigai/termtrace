//! Typed pointer, session-control, ordered-send, status, and launch facades for Node.

use crate::actions::{mutation_err, request_id_arg, send_err, JsActionResult};
use crate::host::{self, Arg, Model};
use crate::{blocking_failure, to_napi_err, with_frame, JsCaptured};
use napi::bindgen_prelude::{Object, ToNapiValue};
use napi::{sys, Env};
use napi_derive::napi;
use std::ffi::OsString;
use std::path::PathBuf;
use termtrace_model::protocol::control::ProcessSignal;
use termtrace_model::terminal::state::MouseButton;
use termtrace_sdk::client::{Session, Terminal};
use termtrace_sdk::contracts::control::{
    guard, CommandOutcome, ErrorReport, LaunchOptions, SendSpec,
};
use termtrace_sdk::input::{MutationGuard, PointerOutcome};
use termtrace_sdk::screen::{ScreenWaitResult, StableScreenCapture};
use termtrace_sdk::session::{CommandPostcondition, LaunchSpec, SendLastStep, SendOutcome};
use termtrace_sdk::Uuid;

/// One send result containing model data and native frame and action classes.
pub enum SendResult {
    /// A text field.
    Text(String),
    Index(usize),
    Null,
    Command(CommandOutcome),
    Error(ErrorReport),
    Changes(Option<termtrace_model::terminal::diff::ScreenChanges>),
    /// Model fields plus the exact frame they describe.
    Stable(Box<JsCaptured<StableScreenCapture>>),
    Wait(JsCaptured<ScreenWaitResult>),
    /// A native action result.
    Action(JsActionResult),
    /// An object of named entries.
    Object(Vec<(&'static str, SendResult)>),
}

impl ToNapiValue for SendResult {
    unsafe fn to_napi_value(env: sys::napi_env, val: Self) -> napi::Result<sys::napi_value> {
        // SAFETY: napi supplies a live env; every value is created in it during this call.
        unsafe {
            return match val {
                Self::Text(v) => Model::to_napi_value(env, Model(v)),
                Self::Index(v) => Model::to_napi_value(env, Model(v)),
                Self::Null => Model::to_napi_value(env, Model(())),
                Self::Command(v) => Model::to_napi_value(env, Model(v)),
                Self::Error(v) => Model::to_napi_value(env, Model(v)),
                Self::Changes(v) => Model::to_napi_value(env, Model(v)),
                Self::Stable(v) => JsCaptured::to_napi_value(env, *v),
                Self::Wait(v) => JsCaptured::to_napi_value(env, v),
                Self::Action(result) => JsActionResult::to_napi_value(env, result),
                Self::Object(entries) => {
                    let mut object = Object::new(&Env::from(env))?;
                    for (name, entry) in entries {
                        object.set(name, entry)?;
                    }

                    Object::to_napi_value(env, object)
                }
            };
        }
    }
}

fn byte(value: u32, what: &str) -> napi::Result<u8> {
    return u8::try_from(value).map_err(|error| {
        return host::range_error(format!(
            "{what} must fit an unsigned 8-bit integer: {error}"
        ));
    });
}

/// Apply one mutation with an identity allocated now unless the caller supplied one.
///
/// # Errors
///
/// Fails for an invalid request identity, and with the mutation's error, carrying
/// `request_id`, when the session cannot be attached or the recorder rejects it.
pub(crate) async fn mutate<T>(
    terminal: Terminal,
    path: PathBuf,
    request_id: Option<&str>,
    if_state: Option<String>,
    call: impl FnOnce(&Session, &MutationGuard) -> termtrace_sdk::Result<T> + Send + 'static,
) -> napi::Result<T>
where
    T: Send + 'static,
{
    let mutation = guard(request_id_arg(request_id)?, if_state);
    let request_id = mutation.request_id.unwrap_or_default();

    return tokio::task::spawn_blocking(move || {
        return terminal
            .attach(path)
            .and_then(|session| return call(&session, &mutation))
            .map_err(|error| return mutation_err(error, request_id));
    })
    .await
    .map_err(blocking_failure)?;
}

/// Options of one pointer click.
#[napi(object, object_to_js = false, js_name = "PointerOptions")]
pub struct JsPointerOptions {
    /// Mouse button; `left` by default.
    pub button: Option<Arg<MouseButton>>,
    /// Reject instead of a no-op when the application does not report mouse input.
    pub strict: Option<bool>,
    /// Identity of this mutation, to retry it safely.
    pub request_id: Option<String>,
    /// Presented-state guard.
    pub if_state: Option<String>,
}

/// Options of one pointer drag.
#[napi(object, object_to_js = false, js_name = "DragOptions")]
pub struct JsDragOptions {
    /// Mouse button; `left` by default.
    pub button: Option<Arg<MouseButton>>,
    /// Motion samples including the destination; 1 by default.
    pub steps: Option<u32>,
    /// Reject instead of a no-op when the application does not report mouse input.
    pub strict: Option<bool>,
    /// Identity of this mutation, to retry it safely.
    pub request_id: Option<String>,
    /// Presented-state guard.
    pub if_state: Option<String>,
}

/// Options of one wheel scroll.
#[napi(object, object_to_js = false, js_name = "ScrollOptions")]
pub struct JsScrollOptions {
    /// Signed wheel ticks; negative scrolls up; 1 by default.
    pub lines: Option<i32>,
    /// Reject instead of a no-op when the application does not report mouse input.
    pub strict: Option<bool>,
    /// Identity of this mutation, to retry it safely.
    pub request_id: Option<String>,
    /// Presented-state guard.
    pub if_state: Option<String>,
}

/// Options of one focus report.
#[napi(object, object_to_js = false, js_name = "FocusOptions")]
pub struct JsFocusOptions {
    /// Reject instead of a no-op when the application does not report focus.
    pub strict: Option<bool>,
    /// Identity of this mutation, to retry it safely.
    pub request_id: Option<String>,
    /// Presented-state guard.
    pub if_state: Option<String>,
}

/// Options of one text activation.
#[napi(object, object_to_js = false, js_name = "ActivateOptions")]
pub struct JsActivateOptions {
    /// Reject when the text appears more than once.
    pub unique: Option<bool>,
    /// Reject instead of a no-op when the application does not report mouse input.
    pub strict: Option<bool>,
    /// Identity of this mutation, to retry it safely.
    pub request_id: Option<String>,
    /// Presented-state guard.
    pub if_state: Option<String>,
}

/// Identity and guard of one session-control mutation.
#[napi(object, object_to_js = false, js_name = "MutationOptions")]
pub struct JsMutationOptions {
    /// Identity of this mutation, to retry it safely.
    pub request_id: Option<String>,
    /// Presented-state guard.
    pub if_state: Option<String>,
}

/// Mode-aware pointer, focus, and text-activation operations with typed results.
///
/// Each call is one mutation; pass `requestId` to retry it safely with the same identity.
/// Abandoning a promise does not stop or roll back the mutation.
#[napi(js_name = "Pointer")]
pub struct JsPointer {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: PathBuf,
}

impl JsPointer {
    async fn apply(
        &self,
        request_id: Option<String>,
        if_state: Option<String>,
        call: impl FnOnce(&Session, &MutationGuard) -> termtrace_sdk::Result<PointerOutcome>
            + Send
            + 'static,
    ) -> napi::Result<Model<PointerOutcome>> {
        let outcome = mutate(
            self.terminal.clone(),
            self.session_path.clone(),
            request_id.as_deref(),
            if_state,
            call,
        )
        .await?;

        return Ok(Model(outcome));
    }
}

#[napi]
impl JsPointer {
    /// Press and release one button at a zero-based cell.
    ///
    /// # Errors
    ///
    /// Fails for an invalid cell, button, or request identity, and rejects with `request_id` when
    /// the recorder rejects the click (with `strict`, when the application does not report mouse
    /// input).
    #[napi]
    pub async fn click(
        &self,
        column: f64,
        row: f64,
        options: Option<JsPointerOptions>,
    ) -> napi::Result<Model<PointerOutcome>> {
        let column = host::cell_coordinate(column, "column")?;
        let row = host::cell_coordinate(row, "row")?;
        let options = options.unwrap_or(JsPointerOptions {
            button: None,
            strict: None,
            request_id: None,
            if_state: None,
        });
        let button = options.button.map_or(MouseButton::Left, |button| button.0);
        let strict = options.strict.unwrap_or(false);

        return self
            .apply(
                options.request_id,
                options.if_state,
                move |session, mutation| {
                    let request_id = mutation.request_id.unwrap_or_default();
                    return session
                        .pointer()
                        .click(column, row, button, strict, mutation)
                        .and_then(|payload| {
                            return PointerOutcome::from_payload(request_id, payload);
                        });
                },
            )
            .await;
    }

    /// Emit `count` complete clicks at a zero-based cell.
    ///
    /// # Errors
    ///
    /// Fails and rejects as `click`.
    #[napi]
    pub async fn multi_click(
        &self,
        column: f64,
        row: f64,
        count: u32,
        options: Option<JsPointerOptions>,
    ) -> napi::Result<Model<PointerOutcome>> {
        let column = host::cell_coordinate(column, "column")?;
        let row = host::cell_coordinate(row, "row")?;
        let count = byte(count, "count")?;
        let options = options.unwrap_or(JsPointerOptions {
            button: None,
            strict: None,
            request_id: None,
            if_state: None,
        });
        let button = options.button.map_or(MouseButton::Left, |button| button.0);
        let strict = options.strict.unwrap_or(false);

        return self
            .apply(
                options.request_id,
                options.if_state,
                move |session, mutation| {
                    return session
                        .pointer()
                        .multi_click(column, row, button, count, strict, mutation);
                },
            )
            .await;
    }

    /// Press at a zero-based cell, emit `steps` motion samples, and release at the end cell.
    ///
    /// # Errors
    ///
    /// Fails and rejects as `click`.
    #[napi]
    pub async fn drag(
        &self,
        column: f64,
        row: f64,
        to_column: f64,
        to_row: f64,
        options: Option<JsDragOptions>,
    ) -> napi::Result<Model<PointerOutcome>> {
        let column = host::cell_coordinate(column, "column")?;
        let row = host::cell_coordinate(row, "row")?;
        let to_column = host::cell_coordinate(to_column, "toColumn")?;
        let to_row = host::cell_coordinate(to_row, "toRow")?;
        let options = options.unwrap_or(JsDragOptions {
            button: None,
            steps: None,
            strict: None,
            request_id: None,
            if_state: None,
        });
        let button = options.button.map_or(MouseButton::Left, |button| button.0);
        let steps = byte(options.steps.unwrap_or(1), "steps")?;
        let strict = options.strict.unwrap_or(false);

        return self
            .apply(
                options.request_id,
                options.if_state,
                move |session, mutation| {
                    return session.pointer().drag(
                        column, row, to_column, to_row, button, steps, strict, mutation,
                    );
                },
            )
            .await;
    }

    /// Emit signed vertical wheel ticks at a zero-based cell; negative scrolls up.
    ///
    /// # Errors
    ///
    /// Fails and rejects as `click`.
    #[napi]
    pub async fn scroll(
        &self,
        column: f64,
        row: f64,
        options: Option<JsScrollOptions>,
    ) -> napi::Result<Model<PointerOutcome>> {
        let column = host::cell_coordinate(column, "column")?;
        let row = host::cell_coordinate(row, "row")?;
        let options = options.unwrap_or(JsScrollOptions {
            lines: None,
            strict: None,
            request_id: None,
            if_state: None,
        });
        let lines = i16::try_from(options.lines.unwrap_or(1)).map_err(|error| {
            return host::range_error(format!("lines must fit a signed 16-bit integer: {error}"));
        })?;
        let strict = options.strict.unwrap_or(false);

        return self
            .apply(
                options.request_id,
                options.if_state,
                move |session, mutation| {
                    let request_id = mutation.request_id.unwrap_or_default();
                    return session
                        .pointer()
                        .scroll(column, row, lines, strict, mutation)
                        .and_then(|payload| {
                            return PointerOutcome::from_payload(request_id, payload);
                        });
                },
            )
            .await;
    }

    /// Report a focus transition when the application enabled focus reporting.
    ///
    /// # Errors
    ///
    /// Fails and rejects as `click`.
    #[napi]
    pub async fn focus(
        &self,
        focused: bool,
        options: Option<JsFocusOptions>,
    ) -> napi::Result<Model<PointerOutcome>> {
        let options = options.unwrap_or(JsFocusOptions {
            strict: None,
            request_id: None,
            if_state: None,
        });
        let strict = options.strict.unwrap_or(false);

        return self
            .apply(
                options.request_id,
                options.if_state,
                move |session, mutation| {
                    let request_id = mutation.request_id.unwrap_or_default();
                    return session.pointer().focus(focused, strict, mutation).and_then(
                        |payload| return PointerOutcome::from_payload(request_id, payload),
                    );
                },
            )
            .await;
    }

    /// Resolve visible row-contained text and click its center from one actor-ordered state.
    ///
    /// # Errors
    ///
    /// Fails and rejects as `click`; the codes `activation_text_not_found`,
    /// `activation_text_concealed`, and `activation_text_ambiguous` report resolution failures.
    #[napi]
    pub async fn activate(
        &self,
        text: String,
        options: Option<JsActivateOptions>,
    ) -> napi::Result<Model<PointerOutcome>> {
        let options = options.unwrap_or(JsActivateOptions {
            unique: None,
            strict: None,
            request_id: None,
            if_state: None,
        });
        let unique = options.unique.unwrap_or(false);
        let strict = options.strict.unwrap_or(false);

        return self
            .apply(
                options.request_id,
                options.if_state,
                move |session, mutation| {
                    return session.pointer().activate(text, unique, strict, mutation);
                },
            )
            .await;
    }
}

fn mutation_options(options: Option<JsMutationOptions>) -> (Option<String>, Option<String>) {
    return options.map_or((None, None), |options| {
        return (options.request_id, options.if_state);
    });
}

/// Resize the child PTY.
///
/// # Errors
///
/// Fails for an invalid geometry or request identity, and rejects with `request_id` when the
/// recorder rejects it.
pub(crate) async fn resize(
    terminal: Terminal,
    path: PathBuf,
    cols: f64,
    rows: f64,
    options: Option<JsMutationOptions>,
) -> napi::Result<Model<termtrace_sdk::ControlAcknowledgement>> {
    let cols = host::cell_coordinate(cols, "cols")?;
    let rows = host::cell_coordinate(rows, "rows")?;
    let (request_id, if_state) = mutation_options(options);
    let outcome = mutate(
        terminal,
        path,
        request_id.as_deref(),
        if_state,
        move |session, mutation| {
            return session.resize(cols, rows, mutation);
        },
    )
    .await?;

    return Ok(Model(outcome));
}

/// Send one allow-listed signal to the child process group.
///
/// # Errors
///
/// Fails for an unknown signal or invalid request identity, and rejects with `request_id` when
/// the recorder rejects it.
pub(crate) async fn signal(
    terminal: Terminal,
    path: PathBuf,
    signal: Arg<ProcessSignal>,
    options: Option<JsMutationOptions>,
) -> napi::Result<Model<termtrace_sdk::ControlAcknowledgement>> {
    let signal = signal.0;
    let (request_id, if_state) = mutation_options(options);
    let outcome = mutate(
        terminal,
        path,
        request_id.as_deref(),
        if_state,
        move |session, mutation| {
            return session.signal(signal, mutation);
        },
    )
    .await?;

    return Ok(Model(outcome));
}

/// Append a canonical marker.
///
/// # Errors
///
/// Fails for an invalid request identity, and rejects with `request_id` when the recorder
/// rejects it.
pub(crate) async fn mark(
    terminal: Terminal,
    path: PathBuf,
    label: String,
    options: Option<JsMutationOptions>,
) -> napi::Result<Model<termtrace_sdk::MarkOutcome>> {
    let (request_id, if_state) = mutation_options(options);
    let outcome = mutate(
        terminal,
        path,
        request_id.as_deref(),
        if_state,
        move |session, mutation| {
            return session.mark(label, mutation);
        },
    )
    .await?;

    return Ok(Model(outcome));
}

/// Read the live status or the completion of a session.
///
/// # Errors
///
/// Rejects when the session cannot be attached or read.
pub(crate) async fn status(
    terminal: Terminal,
    path: PathBuf,
) -> napi::Result<Model<termtrace_sdk::SessionStatus>> {
    return tokio::task::spawn_blocking(move || {
        return terminal
            .attach(path)
            .and_then(|session| return session.status())
            .map(Model)
            .map_err(to_napi_err);
    })
    .await
    .map_err(blocking_failure)?;
}

fn checked(kind: &'static str, entry: (&'static str, SendResult)) -> SendResult {
    return SendResult::Object(vec![("kind", SendResult::Text(kind.to_owned())), entry]);
}

fn send_result(
    send_request_id: Uuid,
    last_step: usize,
    outcome: SendOutcome,
) -> napi::Result<SendResult> {
    let step = ("step", SendResult::Index(last_step));
    let last = match outcome.last {
        SendLastStep::Action(action) => SendResult::Object(vec![
            ("kind", SendResult::Text("action".to_owned())),
            step,
            (
                "request_id",
                SendResult::Text(action.request_id.to_string()),
            ),
            ("result", SendResult::Action(JsActionResult::new(*action))),
        ]),
        SendLastStep::Command {
            request_id,
            payload,
            postcondition,
        } => {
            let command =
                CommandOutcome::from_payload(request_id, &payload).map_err(to_napi_err)?;

            let postcondition = match postcondition.map(|postcondition| return *postcondition) {
                None => SendResult::Null,
                Some(CommandPostcondition::Idle(Ok(capture))) => checked(
                    "idle",
                    (
                        "capture",
                        SendResult::Stable(Box::new(with_frame(capture.clone(), capture.frame))),
                    ),
                ),
                Some(CommandPostcondition::Screen(Ok(wait))) => checked(
                    "screen",
                    (
                        "wait",
                        SendResult::Wait(with_frame(ScreenWaitResult::from(&wait), wait.frame)),
                    ),
                ),
                Some(CommandPostcondition::Idle(Err(error))) => checked(
                    "idle",
                    ("error", SendResult::Error(ErrorReport::from(&error))),
                ),
                Some(CommandPostcondition::Screen(Err(error))) => checked(
                    "screen",
                    ("error", SendResult::Error(ErrorReport::from(&error))),
                ),
            };

            SendResult::Object(vec![
                ("kind", SendResult::Text("command".to_owned())),
                step,
                ("request_id", SendResult::Text(request_id.to_string())),
                ("outcome", SendResult::Command(command)),
                ("postcondition", postcondition),
            ])
        }
    };

    return Ok(SendResult::Object(vec![
        ("request_id", SendResult::Text(send_request_id.to_string())),
        ("last", last),
        ("changes", SendResult::Changes(outcome.changes)),
    ]));
}

/// Apply ordered steps, each a separate mutation with an identity derived from the send's.
///
/// # Errors
///
/// Fails for malformed steps or options, and on a failed step rejects with the step's error
/// carrying `step`, its derived `request_id`, and `send_request_id`; earlier steps were applied.
pub(crate) async fn send(
    terminal: Terminal,
    path: PathBuf,
    spec: Arg<SendSpec>,
) -> napi::Result<SendResult> {
    let request = spec.0.to_request().map_err(to_napi_err)?;
    let send_request_id = request.guard.request_id.unwrap_or_default();
    let last_step = request.steps.len().saturating_sub(1);

    let outcome = tokio::task::spawn_blocking(move || {
        let session = terminal
            .attach(path)
            .map_err(|error| return mutation_err(error, send_request_id))?;

        return session
            .send(request)
            .map_err(|failure| return send_err(failure, send_request_id));
    })
    .await
    .map_err(blocking_failure)??;

    return send_result(send_request_id, last_step, outcome);
}

/// Launch specification for decoded options.
///
/// # Errors
///
/// Fails with `invalid_operation` for an invalid command, geometry, environment, or limit.
pub(crate) fn launch_spec_from(
    command: Vec<String>,
    options: &LaunchOptions,
) -> napi::Result<LaunchSpec> {
    return options
        .to_spec(command.into_iter().map(OsString::from).collect())
        .map_err(to_napi_err);
}

/// Launch options decoded from a host object.
///
/// # Errors
///
/// Fails with a `TypeError` for unknown or malformed options and with `invalid_operation` for an
/// invalid command, geometry, environment, or limit.
pub(crate) fn launch_spec(
    command: Vec<String>,
    options: Option<Arg<LaunchOptions>>,
) -> napi::Result<LaunchSpec> {
    let options = options.map(|v| v.0).unwrap_or_default();

    return launch_spec_from(command, &options);
}
