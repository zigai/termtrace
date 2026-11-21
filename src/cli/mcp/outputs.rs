use rmcp::model::JsonObject;
use schemars::{generate::SchemaSettings, JsonSchema, SchemaGenerator};
use serde_json::{json, Value};
use std::sync::Arc;
use termtrace_model::{
    artifact::{bug_report::BugReportManifest, bundle::BundleManifest, bundle::BundleProfile},
    execution::{
        event_log::CanonicalBoundary,
        session::{SessionCompletion, SessionPhase, VerificationReport},
    },
    protocol::{
        control::{ClockSample, ControlErrorDetails, InputSequenceAtomOutcome},
        mutation::{MutationEffect, Retry},
    },
    registry::GarbageCollectionReport,
    registry::RegistryListEntry,
    terminal::{
        analysis::{DiagnoseReport, ExplainReport, LastActionReport},
        diff::{RowMove, ScreenChanges, SemanticDiff},
        observation::{ActorBoundaries, CaptureQuality, Observation},
        screen::ScreenFrame,
        timeline::TimelineEntry,
    },
    value::{pagination::OutputPage, path_bytes::PathValue},
};

// Each schema lists the shapes a command prints, so a client can validate any result,
// including failed calls that still carry structured content.

pub(super) fn start() -> Arc<JsonObject> {
    let mut schemas = Schemas::new();
    let started = object(
        &[
            "kind",
            "alias",
            "canonical_session",
            "session_id",
            "ready",
            "completion",
        ],
        json!({
            "kind": {"const": "started"},
            "alias": {"type": "string"},
            "canonical_session": schemas.typed::<PathValue>(),

            "session_id": {"type": "string", "format": "uuid"},
            "ready": {"type": "boolean"},
            "completion": schemas.typed::<Option<SessionCompletion>>(),
            "postcondition": start_postcondition(),
        }),
    );
    let failed = object(
        &[
            "kind",
            "startup_state",
            "alias",
            "canonical_session",
            "session_id",
            "recorder_pid",
            "recorder_process_start_ticks",
            "stdout_log",
            "stderr_log",
            "ready",
            "completion",
        ],
        json!({
            "kind": {"const": "started"},
            "startup_state": {"enum": ["live", "completed_success", "completed_failure"]},
            "alias": {"type": "string"},
            "canonical_session": schemas.typed::<PathValue>(),

            "session_id": {"type": "string", "format": "uuid"},
            "recorder_pid": {"type": ["integer", "null"]},
            "recorder_process_start_ticks": {"type": ["integer", "null"]},
            "stdout_log": schemas.typed::<PathValue>(),
            "stderr_log": schemas.typed::<PathValue>(),
            "ready": {"type": "boolean"},
            "completion": schemas.typed::<Option<SessionCompletion>>(),
            "postcondition": start_postcondition(),
        }),
    );
    return schemas.finish(vec![started, failed]);
}

fn start_postcondition() -> Value {
    return object(
        &["matched"],
        json!({
            "matched": {"type": "boolean"},
            "ended": {"type": "boolean"},
            "kind": {"enum": ["idle", "screen", "screen_absent", "screen_regex", "output"]},
            "outcome": {"type": "string"},
            "error": {"type": "string"},
            "state": {"type": "string"},
            "state_hash": {"type": "string"},
            "event_seq": {"type": "integer"},
            "output_end_offset": {"type": "integer"},
            "monotonic_ns": {"type": "integer"},
            "size": size(),
            "screen": {"type": "string"},
        }),
    );
}

pub(super) fn restart() -> Arc<JsonObject> {
    let mut schemas = Schemas::new();
    let mut session = || {
        return object(
            &["canonical_session", "session_id"],
            json!({
                "canonical_session": schemas.typed::<PathValue>(),
                "session_id": {"type": "string", "format": "uuid"},
            }),
        );
    };
    let predecessor = session();
    let successor = session();
    let restarted = object(
        &["alias", "predecessor", "successor", "ready"],
        json!({
            "alias": {"type": "string"},
            "predecessor": predecessor,
            "successor": successor,
            "ready": {"type": "boolean"},
        }),
    );
    return schemas.finish(vec![restarted]);
}

pub(super) fn gc() -> Arc<JsonObject> {
    let mut schemas = Schemas::new();
    let report = schemas.typed::<GarbageCollectionReport>();
    return schemas.finish(vec![report]);
}

pub(super) fn send() -> Arc<JsonObject> {
    let mut schemas = Schemas::new();
    let postcondition = object(
        &["kind", "matched", "outcome"],
        json!({
            "kind": {"enum": ["none", "output", "idle", "screen"]},
            "matched": {"type": "boolean"},
            "outcome": {"type": "string"},
            "matched_row": {"type": "integer", "minimum": 0},
            "matched_text": {"type": "string"},
        }),
    );
    let applied = object(
        &[
            "v",
            "ok",
            "type",
            "request_id",
            "input",
            "postcondition",
            "observation",
            "ended",
        ],
        json!({
            "v": {"const": 1},
            "ok": {"type": "boolean"},
            "type": {"const": "action"},
            "request_id": {"type": "string", "format": "uuid"},
            "input": object(
                &["effect", "atoms", "bytes", "pace_ns"],
                json!({
                    "effect": schemas.typed::<MutationEffect>(),
                    "atoms": {"type": "integer"},
                    "bytes": {"type": "integer"},
                    "pace_ns": {"type": "integer"},
                    "outcomes": {"type": "array", "items": schemas.typed::<InputSequenceAtomOutcome>()},
                }),
            ),
            "postcondition": postcondition,
            "observation": object(
                &[],
                json!({
                    "state": {"type": "string"},
                }),
            ),
            "ended": {"type": "boolean"},
            "boundaries": schemas.typed::<ActorBoundaries>(),
            "code": {"const": "postcondition_failed"},
            "changes": schemas.typed::<ScreenChanges>(),
            "changing_rows": rows(),
            "screen": {"type": "string"},
            "size": size(),
        }),
    );
    let rejected = object(
        &[
            "v",
            "ok",
            "type",
            "code",
            "message",
            "request_id",
            "input",
            "observation",
            "ended",
        ],
        json!({
            "v": {"const": 1},
            "ok": {"const": false},
            "type": {"const": "action"},
            "code": {"type": "string"},
            "message": {"type": "string"},
            "request_id": {"type": "string", "format": "uuid"},
            "input": object(
                &["effect", "atoms", "bytes", "retry"],
                json!({
                    "effect": schemas.typed::<MutationEffect>(),
                    "atoms": {"type": "integer"},
                    "bytes": {"type": "integer"},
                    "prefix": {"type": "object"},
                    "retry": schemas.typed::<Retry>(),
                }),
            ),
            "observation": {
                "anyOf": [
                    {"type": "null"},
                    object(
                        &["state", "scope"],
                        json!({"state": {"type": "string"}, "scope": {"type": "string"}}),
                    ),
                ],
            },
            "ended": {"type": "boolean"},
            "boundaries": schemas.typed::<Option<ActorBoundaries>>(),
        }),
    );
    let command = object(
        &["v", "ok", "type", "request_id", "postcondition"],
        json!({
            "v": {"const": 1},
            "ok": {"type": "boolean"},
            "type": {"enum": ["pointer", "activate", "resize", "signal", "mark", "wait"]},
            "request_id": {"type": "string", "format": "uuid"},
            "postcondition": postcondition,
            "observation": object(
                &[],
                json!({
                    "state": {"type": "string"},
                }),
            ),
            "changes": schemas.typed::<ScreenChanges>(),
            "changing_rows": rows(),
            "screen": {"type": "string"},
            "size": size(),
            "code": {"const": "postcondition_failed"},
        }),
    );

    let wait_atom_failed = object(
        &[
            "v",
            "ok",
            "type",
            "code",
            "request_id",
            "atom",
            "atom_index",
            "outcome",
            "state",
            "size",
            "screen",
        ],
        json!({
            "v": {"const": 1},
            "ok": {"const": false},
            "type": {"const": "action"},
            "code": {"const": "wait_atom_failed"},
            "request_id": {"type": "string", "format": "uuid"},
            "atom": {"type": "string"},
            "atom_index": {"type": "integer", "minimum": 0},
            "outcome": {"enum": ["deadline", "child_completed"]},
            "state": {"type": "string"},
            "size": size(),
            "screen": {"type": "string"},
        }),
    );

    let lifecycle = lifecycle(&mut schemas);

    return schemas.finish(vec![
        applied,
        rejected,
        command,
        wait_atom_failed,
        lifecycle,
    ]);
}

pub(super) fn screen() -> Arc<JsonObject> {
    let mut schemas = Schemas::new();
    let rows = json!({
        "type": "array",
        "items": {
            "type": "array",
            "prefixItems": [{"type": "integer"}, {"type": "string"}],
            "minItems": 2,
            "maxItems": 2,
        },
    });
    let current = object(
        &[
            "v",
            "ok",
            "type",
            "session",
            "lifecycle",
            "state",
            "quality",
            "size",
            "ended",
        ],
        json!({
            "v": {"const": 1},
            "ok": {"const": true},
            "type": {"const": "screen"},
            "session": {"type": "string"},
            "lifecycle": schemas.typed::<SessionPhase>(),
            "state": {"type": "string"},
            "quality": schemas.typed::<CaptureQuality>(),
            "size": size(),
            "ended": {"type": "boolean"},
            "unchanged": {"const": true},
            "screen": {
                "anyOf": [
                    {"type": "string"},
                    object(
                        &["encoding", "rows", "blank_rows_omitted"],
                        json!({
                            "encoding": {"const": "rows"},
                            "rows": rows,
                            "blank_rows_omitted": {"type": "integer"},
                        }),
                    ),
                    object(
                        &["encoding", "since", "moves", "rows"],
                        json!({
                            "encoding": {"const": "diff_rows"},
                            "since": {"type": "string"},
                            "moves": {"type": "array", "items": schemas.typed::<RowMove>()},
                            "rows": rows,
                        }),
                    ),
                ],
            },
        }),
    );
    let earlier = schemas.typed::<Observation>();
    let lifecycle = lifecycle(&mut schemas);

    return schemas.finish(vec![current, earlier, lifecycle]);
}

pub(super) fn wait() -> Arc<JsonObject> {
    let mut schemas = Schemas::new();
    let report = object(
        &["matched", "outcome", "predicate", "state", "size"],
        json!({
            "matched": {"type": "boolean"},
            "outcome": {"type": "string"},
            "predicate": {"type": "object"},
            "state": {"type": "string"},
            "size": size(),
            "screen": {"type": "string"},
            "changing_rows": rows(),
            "frame_hash": {"type": "string"},
            "frame": schemas.typed::<ScreenFrame>(),
            "first_match_monotonic_ns": {"type": ["integer", "null"]},
            "observed_monotonic_ns": {"type": "integer"},
            "stable_for_ns": {"type": "integer"},
            "first_observed_monotonic_ns": {"type": "integer"},
            "event_seq": {"type": "integer"},
            "output_end_offset": {"type": "integer"},
            "monotonic_ns": {"type": "integer"},
            "semantic_state_zstd": {"type": "string"},
        }),
    );
    let lifecycle = lifecycle(&mut schemas);

    return schemas.finish(vec![report, lifecycle]);
}

pub(super) fn status() -> Arc<JsonObject> {
    let mut schemas = Schemas::new();
    let sessions = object(
        &["sessions"],
        json!({
            "sessions": {"type": "array", "items": schemas.typed::<RegistryListEntry>()},
        }),
    );
    let completed = completed(&mut schemas, &["session", "lifecycle"]);
    let live = object(
        &[
            "kind",
            "session",
            "lifecycle",
            "session_state",
            "pid",
            "boundary",
            "clock_sample",
        ],
        json!({
            "kind": {"const": "status"},
            "session": {"type": "string"},
            "lifecycle": {"const": "live"},
            "session_state": {"const": "live"},
            "pid": {"type": ["integer", "null"]},
            "boundary": boundary(&mut schemas),
            "clock_sample": schemas.typed::<ClockSample>(),
        }),
    );
    let lifecycle = lifecycle(&mut schemas);

    return schemas.finish(vec![sessions, completed, live, lifecycle]);
}

pub(super) fn stop() -> Arc<JsonObject> {
    let mut schemas = Schemas::new();
    let rejected = object(
        &[
            "kind",
            "operation",
            "code",
            "message",
            "details",
            "effect",
            "retry",
        ],
        json!({
            "kind": {"const": "control_rejected"},
            "operation": {"const": "stop"},
            "code": {"type": "string"},
            "message": {"type": "string"},
            "details": schemas.typed::<Option<ControlErrorDetails>>(),
            "effect": schemas.typed::<MutationEffect>(),
            "retry": schemas.typed::<Retry>(),
        }),
    );
    let acknowledged = object(
        &["kind", "boundary"],
        json!({
            "kind": {"const": "acknowledged"},
            "boundary": boundary(&mut schemas),
        }),
    );
    let stopped = object(
        &["kind", "session", "session_id", "exit_code"],
        json!({
            "kind": {"const": "stopped"},
            "alias": {"type": "string"},
            "session": schemas.typed::<PathValue>(),

            "session_id": {"type": "string", "format": "uuid"},
            "exit_code": {"type": ["integer", "null"]},
            "signal": {"type": "string"},
        }),
    );
    let lifecycle = lifecycle(&mut schemas);

    return schemas.finish(vec![rejected, acknowledged, stopped, lifecycle]);
}

pub(super) fn analyze() -> Arc<JsonObject> {
    let mut schemas = Schemas::new();
    let timeline = object(
        &["records"],
        json!({
            "records": {
                "type": "array",
                "items": {
                    "anyOf": [
                        schemas.typed::<TimelineEntry>(),
                        object(
                            &["kind", "page"],
                            json!({
                                "kind": {"const": "output_page"},
                                "page": schemas.typed::<OutputPage>(),
                            }),
                        ),
                    ],
                },
            },
        }),
    );

    let text = object(&["text"], json!({"text": {"type": "string"}}));
    let variants = vec![
        schemas.typed::<LastActionReport>(),
        timeline,
        schemas.typed::<SemanticDiff>(),
        schemas.typed::<ExplainReport>(),
        schemas.typed::<DiagnoseReport>(),
        schemas.typed::<VerificationReport>(),
        text,
        lifecycle(&mut schemas),
    ];
    return schemas.finish(variants);
}

pub(super) fn screenshot() -> Arc<JsonObject> {
    let mut schemas = Schemas::new();
    let semantic = object(
        &[
            "kind",
            "truth_layer",
            "directory",
            "image",
            "manifest",
            "state_hash",
        ],
        json!({
            "kind": {"const": "semantic_screenshot"},
            "truth_layer": {"const": "semantic_rendering"},
            "directory": schemas.typed::<PathValue>(),

            "image": schemas.typed::<PathValue>(),

            "manifest": schemas.typed::<PathValue>(),

            "state_hash": {"type": "string"},
        }),
    );
    let native = object(
        &[
            "kind",
            "truth_layer",
            "directory",
            "image",
            "manifest",
            "endpoint",
            "presentation_evidence",
        ],
        json!({
            "kind": {"const": "native_screenshot"},
            "truth_layer": {"const": "native"},
            "directory": schemas.typed::<PathValue>(),
            "image": schemas.typed::<PathValue>(),
            "manifest": schemas.typed::<PathValue>(),
            "endpoint": schemas.typed::<PathValue>(),
            "presentation_evidence": {"const": "post_endpoint_stable_capture"},
        }),
    );

    return schemas.finish(vec![semantic, native]);
}

pub(super) fn bundle() -> Arc<JsonObject> {
    let mut schemas = Schemas::new();
    let archive = object(
        &[
            "kind",
            "profile",
            "archive",
            "member_count",
            "includes_canonical_evidence",
        ],
        json!({
            "kind": {"const": "bundle_archive"},
            "profile": schemas.typed::<BundleProfile>(),
            "archive": {"type": "string"},
            "member_count": {"type": "integer"},
            "includes_canonical_evidence": {"type": "boolean"},
        }),
    );
    let debug = object(
        &["kind", "profile", "directory", "manifest"],
        json!({
            "kind": {"const": "bundle"},
            "profile": {"const": "debug"},
            "directory": schemas.typed::<PathValue>(),
            "manifest": schemas.typed::<BugReportManifest>(),
        }),
    );
    let directory = object(
        &["kind", "directory", "manifest"],
        json!({
            "kind": {"const": "bundle"},
            "directory": schemas.typed::<PathValue>(),
            "manifest": schemas.typed::<BundleManifest>(),
        }),
    );

    return schemas.finish(vec![archive, debug, directory]);
}

struct Schemas {
    generator: SchemaGenerator,
}

impl Schemas {
    fn new() -> Self {
        return Self {
            generator: SchemaSettings::draft2020_12().into_generator(),
        };
    }

    fn typed<T: JsonSchema>(&mut self) -> Value {
        return self.generator.subschema_for::<T>().to_value();
    }

    fn finish(mut self, variants: Vec<Value>) -> Arc<JsonObject> {
        let mut root = JsonObject::new();
        root.insert("type".to_owned(), json!("object"));

        // Every failure that carries no CLI JSON still arrives as structured content.
        let mut variants = variants;
        variants.push(json!({
            "type": "object",
            "required": ["ok", "error"],
            "properties": {
                "ok": {"const": false},
                "error": {"type": "string"},
            },
        }));
        root.insert("anyOf".to_owned(), Value::Array(variants));

        let definitions = self.generator.take_definitions(true);
        if !definitions.is_empty() {
            root.insert("$defs".to_owned(), Value::Object(definitions));
        }

        return Arc::new(root);
    }
}

fn object(required: &[&str], properties: Value) -> Value {
    return json!({"type": "object", "required": required, "properties": properties});
}

fn rows() -> Value {
    return json!({
        "type": "array",
        "items": {"type": "integer", "minimum": 0},
    });
}

fn size() -> Value {
    return json!({
        "type": "array",
        "items": {"type": "integer"},
        "minItems": 2,
        "maxItems": 2,
    });
}

fn lifecycle(schemas: &mut Schemas) -> Value {
    return object(
        &["kind", "operation", "lifecycle", "session"],
        json!({
            "kind": {"const": "session_lifecycle"},
            "operation": {"type": "string"},
            "lifecycle": schemas.typed::<SessionPhase>(),
            "session": schemas.typed::<PathValue>(),

            "diagnostic": {"type": ["string", "null"]},
            "exit_code": {"type": ["integer", "null"]},
            "signal": {"type": "string"},
            "screen": {"type": "string"},
            "size": size(),
        }),
    );
}

fn boundary(schemas: &mut Schemas) -> Value {
    return json!({
        "anyOf": [
            object(
                &["durable", "next_event_seq", "event_hash", "output_bytes"],
                json!({
                    "durable": {"const": true},
                    "next_event_seq": {"type": "integer"},
                    "event_hash": {"type": "string"},
                    "output_bytes": {"type": "integer"},
                }),
            ),
            object(
                &["durable", "durable_boundary", "observed_boundary"],
                json!({
                    "durable": {"const": false},
                    "durable_boundary": schemas.typed::<CanonicalBoundary>(),
                    "observed_boundary": schemas.typed::<CanonicalBoundary>(),
                }),
            ),
        ],
    });
}

fn completed(schemas: &mut Schemas, also_required: &[&str]) -> Value {
    let mut required = vec!["kind", "completion"];
    required.extend_from_slice(also_required);

    return object(
        &required,
        json!({
            "kind": {"const": "completed"},
            "session": {"type": "string"},
            "lifecycle": {"const": "completed"},
            "completion": schemas.typed::<SessionCompletion>(),
        }),
    );
}
