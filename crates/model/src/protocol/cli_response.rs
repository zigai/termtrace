//! Compact, versioned CLI response protocol contracts.

use std::{borrow::Cow, collections::BTreeMap};

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Current compact CLI response protocol version.
pub const CLI_RESPONSE_VERSION: u32 = 1;

/// Mutation application disposition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Applied {
    /// No mutation was applied.
    None,
    /// The requested mutation was applied in full.
    All,
    /// Only part of the requested mutation was applied.
    Partial,
    /// The mutation outcome could not be determined.
    Unknown,
}

/// Authoritative retry guidance.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Retry {
    /// The command does not need to be retried.
    NotNeeded,
    /// A new request may be submitted safely.
    SafeNewRequest,
    /// A retry is safe only when it reuses the same request identifier.
    SameRequestIdOnly,
    /// Inspect current state before deciding whether to retry.
    InspectBeforeRetry,
    /// Retrying may repeat or conflict with the requested operation.
    Unsafe,
}

/// Session lifecycle known at response time.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// The session is active.
    Live,
    /// The session has ended.
    Ended,
    /// The session completed successfully.
    Completed,
    /// The session lifecycle transition is still pending.
    Pending,
    /// The session lifecycle could not be determined.
    Unknown,
}

/// Compact response envelope with command-specific fields flattened at the top level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CliResponseEnvelope {
    /// Protocol version.
    #[serde(rename = "v")]
    pub version: u32,
    /// Whether the command succeeded.
    pub ok: bool,
    /// Stable command or response type.
    #[serde(rename = "type")]
    pub response_type: String,
    /// Stable failure code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Mutation application disposition.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub applied: Option<Applied>,
    /// Retry guidance.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
    /// Known session lifecycle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_state: Option<SessionState>,
    /// String or integer request identifier copied from the command.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<Value>,
    /// Human-readable diagnostic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Whether the full response exceeded its byte limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,
    /// Original serialized response size when truncated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_bytes: Option<u64>,
    /// Command-specific response fields.
    #[serde(flatten)]
    pub fields: BTreeMap<String, Value>,
}

impl JsonSchema for CliResponseEnvelope {
    fn schema_name() -> Cow<'static, str> {
        return Cow::Borrowed("CliResponseEnvelope");
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        return json_schema!({
            "title": "Termtrace CLI response",
            "description": "Version 1 compact response emitted by --response agent.",
            "type": "object",
            "required": ["v", "ok", "type"],
            "properties": {
                "v": { "const": CLI_RESPONSE_VERSION },
                "ok": { "type": "boolean" },
                "type": { "type": "string", "minLength": 1 },
                "code": { "type": "string", "minLength": 1 },
                "applied": generator.subschema_for::<Applied>(),
                "retry": generator.subschema_for::<Retry>(),
                "session_state": generator.subschema_for::<SessionState>(),
                "request_id": { "type": ["string", "integer"] },
                "message": { "type": "string" },
                "truncated": { "type": "boolean" },
                "response_bytes": { "type": "integer", "minimum": 0 }
            },
            "allOf": [{
                "if": { "properties": { "ok": { "const": false } }, "required": ["ok"] },
                "then": { "required": ["code", "applied", "retry", "session_state"] }
            }],
            "additionalProperties": true
        });
    }
}
