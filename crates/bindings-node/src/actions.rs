//! Typed action results, the sealing action builder, and mutation error facts.

use crate::host::{self, Arg, Model};
use crate::{
    blocking_failure, parse_key, resolved_state, timeout_or_default, typed_error_with, JsFrame,
    JsResolvedState,
};
use napi::bindgen_prelude::{Either, PromiseRaw, Uint8Array};
use napi::Env;
use napi_derive::napi;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use termtrace_model::protocol::control::ScreenPredicate;
use termtrace_sdk::client::Terminal;
use termtrace_sdk::contracts::control::{parse_request_id, PostconditionSummary};
use termtrace_sdk::contracts::ActionDraft;
use termtrace_sdk::defaults;
use termtrace_sdk::input::{ActOutcome, ActRequest, ActionCondition, Reaction};
use termtrace_sdk::session::SendFailure;
use termtrace_sdk::Uuid;

/// Translate the failure of the mutation `request_id`.
///
/// `index.js` lifts `request_id`, the actor's structured `details` (or `null`), and, for
/// `postcondition_incomplete`, the applied `result` onto the thrown error.
pub(crate) fn mutation_err(err: termtrace_sdk::Error, request_id: Uuid) -> napi::Error {
    return mutation_err_with(err, request_id, None);
}

fn mutation_err_with(
    err: termtrace_sdk::Error,
    request_id: Uuid,
    send: Option<(usize, Uuid)>,
) -> napi::Error {
    #[derive(serde::Serialize)]
    struct MutationFailure<'a> {
        request_id: Uuid,
        details: Option<&'a termtrace_model::protocol::control::ControlErrorDetails>,
        #[serde(skip_serializing_if = "Option::is_none")]
        action_result_json: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        step: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        send_request_id: Option<Uuid>,
    }

    let result = match &err {
        termtrace_sdk::Error::PostconditionIncomplete { result } => {
            serde_json::to_string(result).ok()
        }
        _ => None,
    };

    return typed_error_with(
        err.to_string(),
        err.kind().as_str(),
        err.code(),
        err.effect(),
        MutationFailure {
            request_id,
            details: err.details(),
            action_result_json: result,
            step: send.map(|v| v.0),
            send_request_id: send.map(|v| v.1),
        },
    );
}

pub(crate) fn send_err(failure: SendFailure, send_request_id: Uuid) -> napi::Error {
    return mutation_err_with(
        *failure.error,
        failure.request_id,
        Some((failure.step, send_request_id)),
    );
}

/// Parse an optional caller-owned request identity.
///
/// # Errors
///
/// Fails with `invalid_operation` for text that is not a hyphenated non-nil UUID.
pub(crate) fn request_id_arg(request_id: Option<&str>) -> napi::Result<Option<Uuid>> {
    return request_id
        .map(|text| return parse_request_id(text).map_err(crate::to_napi_err))
        .transpose();
}

/// Complete outcome of one action: input application and postcondition, reported separately.
#[napi(js_name = "ActionResult")]
pub struct JsActionResult {
    outcome: Arc<ActOutcome>,
}

impl JsActionResult {
    pub(crate) fn new(outcome: ActOutcome) -> Self {
        return Self {
            outcome: Arc::new(outcome),
        };
    }
}

#[napi]
impl JsActionResult {
    /// Identity of the action request.
    #[napi(getter)]
    pub fn request_id(&self) -> String {
        return self.outcome.request_id.to_string();
    }

    /// Exact input application outcome; counts are wire atoms (paced text is one per scalar).
    #[napi(getter)]
    pub fn input(&self) -> Model<termtrace_model::protocol::control::ActionInputResult> {
        return Model(self.outcome.input.clone());
    }

    /// Postcondition outcome without its frame: `kind`, `matched`, `quality`, `state_hash`.
    #[napi(getter)]
    pub fn postcondition(&self) -> Model<PostconditionSummary> {
        return Model(PostconditionSummary::from(&self.outcome.postcondition));
    }

    /// Exact terminating frame, or `null` when none was projected.
    #[napi(getter)]
    pub fn frame(&self) -> Option<JsFrame> {
        return self
            .outcome
            .frame()
            .map(|frame| return JsFrame::new(Arc::new(frame.clone())));
    }

    /// Whether the postcondition held; input is applied either way.
    #[napi(getter)]
    pub fn matched(&self) -> bool {
        return self.outcome.is_matched();
    }

    /// Terminating presented-state hash, or `null` when the actor reported none.
    #[napi(getter)]
    pub fn state_hash(&self) -> Option<String> {
        return self.outcome.state_hash().map(str::to_owned);
    }

    /// Canonical positions enclosing the action.
    #[napi(getter)]
    pub fn boundaries(&self) -> Model<termtrace_model::terminal::observation::ActorBoundaries> {
        return Model(self.outcome.boundaries.clone());
    }

    /// Canonical model JSON of the action and its frame.
    ///
    /// # Errors
    ///
    /// Fails when the report cannot be encoded.
    #[napi]
    pub fn to_json(&self) -> napi::Result<String> {
        return serde_json::to_string(self.outcome.as_ref())
            .map_err(|e| napi::Error::from_reason(e.to_string()));
    }

    /// Decode canonical model JSON of an action outcome.
    ///
    /// # Errors
    ///
    /// Fails with a `TypeError` when `json` is not a valid action report.
    #[napi(factory)]
    pub fn from_json(json: String) -> napi::Result<Self> {
        return serde_json::from_str::<ActOutcome>(&json)
            .map(Self::new)
            .map_err(|error| return host::type_error(error.to_string()));
    }
}

/// Fluent builder for atomic, state-guarded input sequences.
///
/// The request identity exists from construction. `run()` seals the exact request before it
/// returns its promise, so every later `run()` is a same-identity retry and every change throws
/// `invalid_operation`. Legacy `execute()` sends the current request as a fresh one and starts
/// over with a new identity.
#[napi(js_name = "ActionBuilder")]
pub struct JsActionBuilder {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: PathBuf,
    draft: Mutex<ActionDraft>,
}

impl JsActionBuilder {
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

    fn edit(&self, change: impl FnOnce(&mut ActRequest)) -> napi::Result<&Self> {
        self.draft().edit(change).map_err(crate::to_napi_err)?;

        return Ok(self);
    }

    fn act<'env, T>(
        &self,
        env: &'env Env,
        request: ActRequest,
        project: fn(ActOutcome) -> T,
    ) -> napi::Result<PromiseRaw<'env, T>>
    where
        T: 'static + Send + napi::bindgen_prelude::ToNapiValue,
    {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();
        let request_id = request.guard.request_id.unwrap_or_default();

        return env.spawn_future(async move {
            return tokio::task::spawn_blocking(move || {
                return terminal
                    .attach(path)
                    .and_then(|session| return session.act(request))
                    .map(project)
                    .map_err(|error| return mutation_err(error, request_id));
            })
            .await
            .map_err(blocking_failure)?;
        });
    }
}

#[napi]
impl JsActionBuilder {
    /// Identity of the next send; readable before any `run()` and stable once sealed.
    #[napi(getter)]
    pub fn request_id(&self) -> String {
        return self.draft().request_id().to_string();
    }

    /// Whether `run()` has frozen the request.
    #[napi(getter)]
    pub fn sealed(&self) -> bool {
        return self.draft().is_sealed();
    }

    /// Replace the pre-allocated request identity before sealing.
    ///
    /// # Errors
    ///
    /// Fails with `invalid_operation` for text that is not a hyphenated non-nil UUID or a sealed
    /// builder.
    #[napi]
    pub fn with_request_id(&self, request_id: String) -> napi::Result<&Self> {
        let request_id = parse_request_id(&request_id).map_err(crate::to_napi_err)?;

        return self
            .draft()
            .set_request_id(request_id)
            .map(|()| self)
            .map_err(crate::to_napi_err);
    }

    /// Enforce that the terminal must be at the specified state hash before executing.
    ///
    /// # Errors
    ///
    /// Fails with `invalid_operation` once sealed.
    #[napi]
    pub fn if_state(&self, state_hash: String) -> napi::Result<&Self> {
        return self.edit(|request| request.guard.if_state = Some(state_hash));
    }

    /// Append UTF-8 text to send to the terminal.
    ///
    /// # Errors
    ///
    /// Fails with `invalid_operation` once sealed.
    #[napi]
    pub fn send_text(&self, text: String) -> napi::Result<&Self> {
        return self.edit(|request| *request = std::mem::take(request).text(text));
    }

    /// Append a key chord (e.g. 'Enter', 'ctrl+c', 'ctrl+shift+F5').
    ///
    /// # Errors
    ///
    /// Fails when `key` is not a valid key chord or names an unknown key, and with
    /// `invalid_operation` once sealed.
    #[napi]
    pub fn send_key(&self, key: String) -> napi::Result<&Self> {
        let chord = parse_key(&key)?;

        return self.edit(|request| *request = std::mem::take(request).key(chord));
    }

    /// Append exact raw bytes to send to the terminal; a legacy `number[]` is still accepted.
    ///
    /// # Errors
    ///
    /// Fails with `invalid_operation` once sealed.
    #[napi]
    pub fn send_bytes(&self, bytes: Either<Uint8Array, Vec<u8>>) -> napi::Result<&Self> {
        let bytes = match bytes {
            Either::A(bytes) => bytes.to_vec(),
            Either::B(bytes) => bytes,
        };

        return self.edit(|request| *request = std::mem::take(request).bytes(bytes));
    }

    /// Send bracketed paste text.
    ///
    /// # Errors
    ///
    /// Fails with `invalid_operation` once sealed.
    #[napi]
    pub fn paste(&self, text: String) -> napi::Result<&Self> {
        return self.edit(|request| *request = std::mem::take(request).paste(text));
    }

    /// Set delay pacing between consecutive action atoms in milliseconds; paced text is typed
    /// one character at a time.
    ///
    /// # Errors
    ///
    /// Fails with `invalid_operation` once sealed.
    #[napi]
    pub fn pace(&self, delay_ms: u32) -> napi::Result<&Self> {
        return self.edit(|request| request.pace = Duration::from_millis(u64::from(delay_ms)));
    }

    /// Attach a human-readable caller intent retained with the input operations.
    ///
    /// # Errors
    ///
    /// Fails with `invalid_operation` once sealed.
    #[napi]
    pub fn intent(&self, intent: String) -> napi::Result<&Self> {
        return self.edit(|request| request.intent = Some(intent));
    }

    /// Report per-wire-atom outcomes in `ActionResult.input.atoms`.
    ///
    /// # Errors
    ///
    /// Fails with `invalid_operation` once sealed.
    #[napi]
    pub fn verbose(&self, enabled: Option<bool>) -> napi::Result<&Self> {
        return self.edit(|request| request.verbose_atom_outcomes = enabled.unwrap_or(true));
    }

    /// Resolve an unmet postcondition as `matched === false` instead of rejecting with
    /// `postcondition_incomplete`.
    ///
    /// # Errors
    ///
    /// Fails with `invalid_operation` once sealed.
    #[napi]
    pub fn allow_incomplete(&self, enabled: Option<bool>) -> napi::Result<&Self> {
        return self.edit(|request| request.allow_incomplete = enabled.unwrap_or(true));
    }

    /// Request the terminating frame's detail and half-open region.
    ///
    /// # Errors
    ///
    /// Fails for an invalid detail or region, and with `invalid_operation` once sealed.
    #[napi]
    pub fn projection(
        &self,
        detail: Option<Arg<termtrace_sdk::FrameDetail>>,
        region: Option<Arg<termtrace_sdk::ScreenRegion>>,
    ) -> napi::Result<&Self> {
        let projection = crate::projection(detail, region);

        return self.edit(|request| request.projection = projection);
    }

    /// Wait until the screen contains the specified pattern after applying input.
    ///
    /// # Errors
    ///
    /// Fails with `invalid_operation` once sealed.
    #[napi]
    pub fn until_screen(&self, pattern: String, timeout_ms: Option<u32>) -> napi::Result<&Self> {
        let timeout = timeout_or_default(timeout_ms);

        return self.edit(|request| {
            request.postcondition = ActionCondition::Screen {
                predicate: ScreenPredicate::Contains { text: pattern },
                timeout,
            };
        });
    }

    /// Wait after applying input until a presented-screen predicate holds.
    ///
    /// # Errors
    ///
    /// Fails for an invalid predicate or a timeout above the wait bound, and with
    /// `invalid_operation` once sealed.
    #[napi]
    pub fn until_screen_where(
        &self,
        predicate: Arg<ScreenPredicate>,
        timeout_ms: Option<f64>,
    ) -> napi::Result<&Self> {
        let predicate = predicate.0;
        let timeout = host::millis(timeout_ms, defaults::WAIT_TIMEOUT)?;

        return self.edit(|request| {
            request.postcondition = ActionCondition::Screen { predicate, timeout };
        });
    }

    /// Wait until the screen changes after applying input and then stays unchanged; the change
    /// must start within `reactionTimeoutMs`, or the condition settles as `unchanged`.
    ///
    /// # Errors
    ///
    /// Fails for a reaction timeout above the wait bound, and with `invalid_operation` once
    /// sealed.
    #[napi]
    pub fn until_idle(
        &self,
        stable_for_ms: Option<u32>,
        timeout_ms: Option<u32>,
        reaction_timeout_ms: Option<f64>,
    ) -> napi::Result<&Self> {
        let stable_for = stable_for_ms.map_or(defaults::IDLE_STABLE_FOR, |millis| {
            return Duration::from_millis(u64::from(millis));
        });
        let timeout = timeout_or_default(timeout_ms);
        let reaction = Reaction {
            changed_from: None,
            timeout: host::millis(reaction_timeout_ms, defaults::REACTION_TIMEOUT)?,
        };

        return self.edit(|request| {
            request.postcondition = ActionCondition::Idle {
                stable_for,
                timeout,
                reaction,
            };
        });
    }

    /// Freeze the exact request without sending it; idempotent.
    #[napi]
    pub fn seal(&self) {
        let _request = self.draft().seal();
    }

    /// Seal the request, then apply it, resolving with separate input and postcondition
    /// outcomes.
    ///
    /// The request is frozen before the promise is returned. Calling `run()` again resends the
    /// identical request: the actor applies it at most once and returns the same `sequence_id`.
    /// Abandoning the promise does not stop or roll back the action.
    ///
    /// # Errors
    ///
    /// Rejects with `request_id`, `effect`, and `retry`; an unmet postcondition rejects with
    /// `postcondition_incomplete` and the applied `result` unless `allowIncomplete()` was set.
    #[napi]
    pub fn run<'env>(&self, env: &'env Env) -> napi::Result<PromiseRaw<'env, JsActionResult>> {
        let request = self.draft().seal();

        return self.act(env, request, JsActionResult::new);
    }

    /// Legacy fresh request: send the current request with the current identity, then start over
    /// empty with a new identity; resolves with the terminating frame summary.
    ///
    /// # Errors
    ///
    /// Throws `invalid_operation` once sealed, and otherwise rejects when the session cannot be
    /// attached or the recorder rejects or cannot complete the queued actions.
    #[napi]
    pub fn execute<'env>(
        &self,
        env: &'env Env,
    ) -> napi::Result<PromiseRaw<'env, Option<JsResolvedState>>> {
        let request = self.draft().take_fresh().map_err(crate::to_napi_err)?;

        return self.act(env, request, |outcome| {
            return outcome.frame().map(resolved_state);
        });
    }
}
