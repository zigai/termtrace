//! Application-reported diagnostics received independently of terminal bytes.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Maximum encoded application record, excluding the canonical envelope.
pub const MAX_APPLICATION_EVENT_BYTES: usize = 64 * 1024;

/// Application instrumentation lifecycle.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationEventKind {
    /// A discrete diagnostic event.
    Event,
    /// Beginning of a caller-owned span.
    SpanStart,
    /// End of a caller-owned span; unmatched starts remain visibly incomplete.
    SpanEnd,
}

/// Caller-reported process and clock information, not independently verified identity.
///
/// The enclosing canonical event supplies recorder time and session identity. A process
/// instance UUID distinguishes PID reuse. Span identifiers are scoped to that instance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplicationEvent {
    /// Stable UUID for the emitting process instance.
    pub process_instance_id: Uuid,
    /// Caller-reported operating-system PID.
    pub process_id: u32,
    /// Caller-reported monotonic nanoseconds since instrumentation initialization.
    pub monotonic_ns: u64,
    /// Event or span lifecycle transition.
    pub kind: ApplicationEventKind,
    /// Diagnostic name, from 1 to 256 UTF-8 bytes.
    pub name: String,
    /// Span identity; required for span lifecycle transitions.
    pub span_id: Option<Uuid>,
    /// Optional parent span in the same process instance.
    pub parent_span_id: Option<Uuid>,
    /// Sensitive application data, excluded from metadata-only exports.
    pub attributes: BTreeMap<String, Value>,
}

impl ApplicationEvent {
    /// Validate the bounded wire record without imposing cross-event span completeness.
    ///
    /// # Errors
    /// Returns a stable description for an invalid name, identity, span, or payload size.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.name.trim().is_empty() || self.name.len() > 256 {
            return Err("application event name must contain 1 to 256 UTF-8 bytes");
        }

        if self.process_id == 0 || self.process_instance_id.is_nil() {
            return Err("application event requires a process identity");
        }

        if !matches!(self.kind, ApplicationEventKind::Event) && self.span_id.is_none() {
            return Err("application span transition requires a span ID");
        }

        if self.span_id.is_some_and(|id| return id.is_nil())
            || self.parent_span_id.is_some_and(|id| return id.is_nil())
            || (self.span_id.is_some() && self.span_id == self.parent_span_id)
        {
            return Err("application span IDs must be non-nil and distinct");
        }

        let bytes = serde_json::to_vec(self).map_err(encoding_failure)?;
        if bytes.len() > MAX_APPLICATION_EVENT_BYTES {
            return Err("application event exceeds 64 KiB");
        }
        return Ok(());
    }
}

/// Collapse an encoding failure into the stable validation description.
///
/// `validate` reports stable `&'static str` categories, and the attributes it would otherwise
/// echo are sensitive application data, so the serializer's detail is deliberately dropped.
fn encoding_failure(_: serde_json::Error) -> &'static str {
    return "invalid application JSON";
}
