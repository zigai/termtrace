//! Application-reported diagnostics received independently of terminal bytes.

use crate::terminal::observation::ActorBoundaries;
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
    #[serde(with = "crate::value::json")]
    #[schemars(with = "BTreeMap<String, Value>")]
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

/// Successful acknowledged SDK instrumentation emission. This is not a persisted event envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplicationAcknowledgement {
    /// True only after the actor's positive acknowledgement.
    pub acknowledged: bool,
    /// Emitting process-instance identity.
    pub process_instance_id: Uuid,
    /// Span identity for start/end, absent for a discrete event.
    pub span_id: Option<Uuid>,
    /// Actor-observed and durable positions from the actual acknowledgement.
    pub boundaries: ActorBoundaries,
}

/// Shared bounded SDK JSON domain. This does not reinterpret or invalidate persisted JSON records.
/// # Errors
/// Rejects unsupported unsafe integral numbers instead of rounding them for a host language.
pub fn validate_sdk_json(value: &Value) -> Result<(), &'static str> {
    let mut remaining = MAX_APPLICATION_EVENT_BYTES;

    return validate_sdk_json_inner(value, 0, &mut remaining);
}

/// SDK input recursion bound, matching the JSON parser's default maximum.
pub const MAX_SDK_JSON_DEPTH: usize = 128;

fn validate_sdk_json_inner(
    value: &Value,
    depth: usize,
    remaining: &mut usize,
) -> Result<(), &'static str> {
    if depth > MAX_SDK_JSON_DEPTH {
        return Err("SDK JSON nesting exceeds the bounded domain");
    }

    let cost = match value {
        Value::String(text) => text.len().saturating_add(1),
        _ => 1,
    };

    *remaining = remaining
        .checked_sub(cost)
        .ok_or("SDK JSON exceeds attribute budget")?;
    match value {
        Value::Number(number) => {
            if number
                .as_u64()
                .is_some_and(|n| return n > 9_007_199_254_740_991)
                || number
                    .as_i64()
                    .is_some_and(|n| return n.unsigned_abs() > 9_007_199_254_740_991)
                || number
                    .as_f64()
                    .is_some_and(|n| return n.fract() == 0.0 && n.abs() > 9_007_199_254_740_991.0)
            {
                return Err("SDK JSON integer exceeds the shared safe-integer domain");
            }
        }
        Value::Array(items) => {
            for item in items {
                validate_sdk_json_inner(item, depth + 1, remaining)?;
            }
        }
        Value::Object(fields) => {
            for (key, item) in fields {
                *remaining = remaining
                    .checked_sub(key.len())
                    .ok_or("SDK JSON exceeds attribute budget")?;
                validate_sdk_json_inner(item, depth + 1, remaining)?;
            }
        }
        _ => {}
    }

    return Ok(());
}

#[cfg(test)]
mod sdk_json_tests {
    use super::{validate_sdk_json, MAX_APPLICATION_EVENT_BYTES, MAX_SDK_JSON_DEPTH};
    use serde_json::{json, Value};

    #[test]
    fn sdk_json_bounds_depth_and_work_without_reinterpreting_canonical_values() {
        let mut deep = Value::Null;
        for _ in 0..=MAX_SDK_JSON_DEPTH {
            deep = Value::Array(vec![deep]);
        }

        assert!(validate_sdk_json(&deep).is_err());
        assert!(
            validate_sdk_json(&Value::String("x".repeat(MAX_APPLICATION_EVENT_BYTES))).is_err()
        );
        assert!(validate_sdk_json(&Value::Array(vec![
            Value::Null;
            MAX_APPLICATION_EVENT_BYTES
        ]))
        .is_err());
        assert!(
            validate_sdk_json(&json!({"data": [null, true, 1.25], "monotonic_ns": 42})).is_ok()
        );
        // A lower bound on encoded cost must not exclude valid compact arrays.
        assert!(validate_sdk_json(&json!(vec![0; 30_000])).is_ok());
    }
}
