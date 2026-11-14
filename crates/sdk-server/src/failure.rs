use crate::protocol::{INTERNAL_ERROR, INVALID_PARAMS, TERMTRACE_ERROR};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::Arc;
use termtrace_model::artifact::presentation::EditFailure;
use termtrace_model::protocol::control::ControlErrorDetails;
use termtrace_model::protocol::management::{
    FailureContext, ManagementFailure, ManagementGuidance,
};
use termtrace_model::protocol::mutation::{MutationEffect, Retry};
use termtrace_model::terminal::screen::ScreenFrame;
use termtrace_sdk::contracts::ErrorReport;
use termtrace_sdk::input::ActOutcome;
use termtrace_sdk::locator::{LocatorError, LocatorFailure, QueryError};
use termtrace_sdk::session::SendFailure;
use termtrace_sdk::Uuid;

/// Whether a failure describes one actor request or a management workflow.
#[derive(Clone, Copy, Debug, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureScope {
    /// An actor request; `effect` and `retry` are set.
    Actor,
    /// A management workflow; `guidance` and `management` are set.
    Management,
}

/// Typed facts of a failed call, carried as the JSON-RPC error `data`.
#[derive(Clone, Debug, Serialize, JsonSchema)]
pub(crate) struct ErrorData {
    /// Error family, such as `invalid_operation`, `wait_unsatisfied`, or a query failure kind.
    kind: String,
    /// Stable machine-readable code.
    code: String,
    /// Whether the failure is scoped to an actor request or a management workflow.
    scope: FailureScope,
    /// What the actor applied before failing.
    effect: Option<MutationEffect>,
    /// Whether the same request identity can be retried.
    retry: Option<Retry>,
    /// Management guidance.
    guidance: Option<ManagementGuidance>,
    /// Proven management progress and paths.
    management: Option<ManagementFailure>,
    /// Identity of the failed mutation.
    request_id: Option<Uuid>,
    /// Actor rejection details.
    control: Option<ControlErrorDetails>,
    /// Edit-plan failure details.
    edit: Option<EditFailure>,
    /// Locator query or wait failure details.
    locator: Option<LocatorFailure>,
    /// Complete action result of an incomplete postcondition.
    action_result: Option<ActOutcome>,
    /// Zero-based index of the failed step of an ordered send.
    step: Option<usize>,
    /// Identity of the ordered send that failed.
    send_request_id: Option<Uuid>,
    /// Last frame a locator wait observed.
    frame: Option<ScreenFrame>,
}

impl ErrorData {
    fn new(kind: &str, code: &str, context: FailureContext) -> Self {
        let (scope, effect, retry, guidance, management) = match context {
            FailureContext::Actor { effect, retry } => {
                (FailureScope::Actor, Some(effect), Some(retry), None, None)
            }
            FailureContext::Management {
                guidance,
                management,
            } => (
                FailureScope::Management,
                None,
                None,
                Some(guidance),
                Some(*management),
            ),
        };

        return Self {
            kind: kind.to_owned(),
            code: code.to_owned(),
            scope,
            effect,
            retry,
            guidance,
            management,
            request_id: None,
            control: None,
            edit: None,
            locator: None,
            action_result: None,
            step: None,
            send_request_id: None,
            frame: None,
        };
    }
}

/// A JSON-RPC error.
#[derive(Debug)]
pub(crate) struct Failure {
    code: i64,
    message: String,
    data: Option<Box<ErrorData>>,
}

impl Failure {
    pub(crate) const fn protocol(code: i64, message: String) -> Self {
        return Self {
            code,
            message,
            data: None,
        };
    }

    pub(crate) const fn invalid_params(message: String) -> Self {
        return Self::protocol(INVALID_PARAMS, message);
    }

    pub(crate) const fn internal(message: String) -> Self {
        return Self::protocol(INTERNAL_ERROR, message);
    }

    /// A failure with no typed facts, such as a call on a recording that already stopped.
    pub(crate) const fn plain(message: String) -> Self {
        return Self::protocol(TERMTRACE_ERROR, message);
    }

    fn typed(message: String, data: ErrorData) -> Self {
        return Self {
            code: TERMTRACE_ERROR,
            message,
            data: Some(Box::new(data)),
        };
    }

    pub(crate) fn sdk(error: &termtrace_sdk::Error) -> Self {
        let report = ErrorReport::from(error);
        let mut data = ErrorData::new(&report.kind, &report.code, error.failure_context());
        data.edit = error.edit_failure().cloned();
        return Self::typed(error.to_string(), data);
    }

    pub(crate) fn mutation(error: &termtrace_sdk::Error, request_id: Uuid) -> Self {
        return Self::mutation_with(error, request_id, None);
    }

    fn mutation_with(
        error: &termtrace_sdk::Error,
        request_id: Uuid,
        send: Option<(usize, Uuid)>,
    ) -> Self {
        let mut data = ErrorData::new(
            error.kind().as_str(),
            error.code(),
            FailureContext::actor(error.effect()),
        );
        data.request_id = Some(request_id);
        data.control = error.details().cloned();
        data.step = send.map(|(step, _)| return step);
        data.send_request_id = send.map(|(_, id)| return id);
        if let termtrace_sdk::Error::PostconditionIncomplete { result } = error {
            data.action_result = Some((**result).clone());
        }

        return Self::typed(error.to_string(), data);
    }

    pub(crate) fn send(failure: SendFailure, send_request_id: Uuid) -> Self {
        return Self::mutation_with(
            &failure.error,
            failure.request_id,
            Some((failure.step, send_request_id)),
        );
    }

    pub(crate) fn unsatisfied_wait(message: String) -> Self {
        return Self::typed(
            message,
            ErrorData::new(
                "wait_unsatisfied",
                "wait_unsatisfied",
                FailureContext::actor(MutationEffect::NotApplied),
            ),
        );
    }

    pub(crate) fn query(error: &QueryError) -> Self {
        return Self::located(
            error.to_string(),
            Some(LocatorFailure::Query(error.clone())),
            None,
        );
    }

    pub(crate) fn locator(error: LocatorError) -> Self {
        let message = error.to_string();
        let details = error.failure();

        return match error {
            LocatorError::Capture(capture) => Self::sdk(&capture),
            LocatorError::Timeout { frame, .. } | LocatorError::Cancelled { frame, .. } => {
                Self::located(message, details, frame)
            }
            LocatorError::Query(_) => Self::located(message, details, None),
        };
    }

    fn located(
        message: String,
        details: Option<LocatorFailure>,
        frame: Option<Arc<ScreenFrame>>,
    ) -> Self {
        let kind = details
            .as_ref()
            .and_then(|details| return serde_json::to_value(details).ok())
            .and_then(|value| {
                return value.get("kind").and_then(Value::as_str).map(str::to_owned);
            })
            .unwrap_or_else(|| return "invalid_query".to_owned());

        let mut data = ErrorData::new(
            &kind,
            &kind,
            FailureContext::actor(MutationEffect::NotApplied),
        );
        data.locator = details;
        data.frame = frame.map(|frame| return (*frame).clone());

        return Self::typed(message, data);
    }

    pub(crate) fn to_object(&self) -> Value {
        return self.data.as_ref().map_or_else(
            || return json!({"code": self.code, "message": self.message}),
            |data| return json!({"code": self.code, "message": self.message, "data": data}),
        );
    }
}
