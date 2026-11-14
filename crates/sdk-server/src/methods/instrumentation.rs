use super::{method, Registry};
use crate::failure::Failure;
use crate::server::Call;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use termtrace_model::execution::application::ApplicationAcknowledgement;
use termtrace_model::value::ExactPath;
use termtrace_sdk::instrumentation::Instrumentation;
use termtrace_sdk::{Client, Uuid};

type Attributes = BTreeMap<String, Value>;

/// Params of `instrumentation.open`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OpenParams {
    /// Live session directory, such as the value of `TERMTRACE_SESSION` in a recorded child.
    session: ExactPath,
}

/// An emitter this client owns.
#[derive(Serialize, JsonSchema)]
struct EmitterInfo {
    /// Handle that addresses the emitter in `instrumentation.*` calls.
    emitter: String,
    /// Process identity of every event and span of this emitter.
    process_instance_id: Uuid,
}

/// Params of `instrumentation.event`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EventParams {
    /// Emitter handle.
    emitter: String,
    /// Event name.
    name: String,
    /// Event attributes.
    #[serde(default)]
    attributes: Attributes,
}

/// Params of `instrumentation.start_span`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StartSpanParams {
    /// Emitter handle.
    emitter: String,
    /// Span name.
    name: String,
    /// Span that contains this one.
    #[serde(default)]
    parent_span_id: Option<Uuid>,
    /// Span attributes.
    #[serde(default)]
    attributes: Attributes,
}

/// Params of `instrumentation.end_span`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EndSpanParams {
    /// Emitter handle.
    emitter: String,
    /// Span name.
    name: String,
    /// Span to end.
    span_id: Uuid,
    /// Attributes recorded at the end.
    #[serde(default)]
    attributes: Attributes,
}

/// Params of `instrumentation.close`.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CloseParams {
    /// Emitter handle.
    emitter: String,
}

pub(super) fn register(registry: &mut Registry) {
    method(
        registry,
        "instrumentation.open",
        "Create an application event emitter bound to a live session.",
        |call, params: OpenParams| -> Result<EmitterInfo, Failure> {
            let emitter = Instrumentation::new(Client::new(params.session.0));
            let process_instance_id = emitter.process_instance_id();

            return Ok(EmitterInfo {
                emitter: call.server.emitters.insert(emitter),
                process_instance_id,
            });
        },
    );
    method(
        registry,
        "instrumentation.event",
        "Emit one application event.",
        event,
    );
    method(
        registry,
        "instrumentation.start_span",
        "Start an application span; it ends only when the caller ends it.",
        start_span,
    );
    method(
        registry,
        "instrumentation.end_span",
        "End an application span.",
        end_span,
    );
    method(
        registry,
        "instrumentation.close",
        "Release the emitter handle; open spans stay open.",
        |call, params: CloseParams| return call.server.emitters.remove(&params.emitter),
    );
}

fn acknowledged<E: Into<termtrace_sdk::Error>>(
    result: Result<ApplicationAcknowledgement, E>,
) -> Result<ApplicationAcknowledgement, Failure> {
    return result.map_err(|error| return Failure::sdk(&error.into()));
}

fn event(call: &Call<'_>, params: EventParams) -> Result<ApplicationAcknowledgement, Failure> {
    let emitter = call.server.emitters.get(&params.emitter)?;

    return acknowledged(emitter.event_ack(&params.name, params.attributes));
}

fn start_span(
    call: &Call<'_>,
    params: StartSpanParams,
) -> Result<ApplicationAcknowledgement, Failure> {
    let emitter = call.server.emitters.get(&params.emitter)?;

    return acknowledged(emitter.start_span_ack(
        &params.name,
        params.parent_span_id,
        params.attributes,
    ));
}

fn end_span(call: &Call<'_>, params: EndSpanParams) -> Result<ApplicationAcknowledgement, Failure> {
    let emitter = call.server.emitters.get(&params.emitter)?;

    return acknowledged(emitter.end_span_ack(&params.name, params.span_id, params.attributes));
}
