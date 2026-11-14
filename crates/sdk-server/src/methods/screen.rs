use super::session::SessionRef;
use super::{method, millis, Registry};
use crate::failure::Failure;
use crate::server::Call;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::time::Duration;
use termtrace_model::protocol::control::ScreenPredicate;
use termtrace_model::terminal::observation::{Observation, StatePoint};
use termtrace_model::terminal::screen::{ScreenFrame, ScreenProjectionRequest};
use termtrace_sdk::contracts::control::{ProjectionSpec, ReactionSpec};
use termtrace_sdk::defaults;
use termtrace_sdk::input::Reaction;
use termtrace_sdk::screen::{
    frame_text, ScreenWaitResult, StableCaptureOptions, StableScreenCapture,
};

/// Model fields plus the exact frame they describe.
///
/// The schema is the inner type's own, which marks itself as carrying a frame.
pub(crate) struct Captured<T> {
    details: T,
    frame: ScreenFrame,
}

impl<T> Captured<T> {
    pub(crate) const fn new(details: T, frame: ScreenFrame) -> Self {
        return Self { details, frame };
    }
}

impl<T: Serialize> Serialize for Captured<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Flat<'value, T> {
            #[serde(flatten)]
            details: &'value T,
            frame: &'value ScreenFrame,
        }

        return Flat {
            details: &self.details,
            frame: &self.frame,
        }
        .serialize(serializer);
    }
}

impl<T: JsonSchema> JsonSchema for Captured<T> {
    fn inline_schema() -> bool {
        return T::inline_schema();
    }

    fn schema_name() -> Cow<'static, str> {
        return T::schema_name();
    }

    fn schema_id() -> Cow<'static, str> {
        return T::schema_id();
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        return T::json_schema(generator);
    }
}

/// A presented state reached by a wait.
#[derive(Serialize, JsonSchema)]
pub(crate) struct ResolvedState {
    /// Semantic state hash.
    hash: String,
    /// Plain text of the frame.
    text: String,
    /// Zero-based cursor row.
    cursor_row: u16,
    /// Zero-based cursor column.
    cursor_col: u16,
    /// The frame.
    frame: ScreenFrame,
}

impl ResolvedState {
    pub(crate) fn of(frame: &ScreenFrame) -> Self {
        return Self {
            hash: frame.state_hash.clone(),
            text: frame_text(frame),
            cursor_row: frame.cursor.row.unwrap_or(0),
            cursor_col: frame.cursor.column.unwrap_or(0),
            frame: frame.clone(),
        };
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FrameParams {
    session: SessionRef,
    /// Detail and region; spans of the whole viewport by default.
    #[serde(default)]
    projection: ProjectionSpec,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FrameAtParams {
    session: SessionRef,
    /// State point; a live session supports only the latest presented state.
    point: StatePoint,
    /// Whether presentation-ineligible states may be selected.
    #[serde(default)]
    include_hidden: bool,
    /// Detail and region; spans of the whole viewport by default.
    #[serde(default)]
    projection: ProjectionSpec,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ObserveParams {
    session: SessionRef,
    /// State point; the latest presented state by default.
    #[serde(default)]
    point: Option<StatePoint>,
    /// Whether presentation-ineligible states may be selected.
    #[serde(default)]
    include_hidden: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CaptureStableParams {
    session: SessionRef,
    /// Required unchanged interval; the default idle interval when absent.
    #[serde(default)]
    stable_for_ms: Option<u64>,
    /// Actor-side bound; the default wait timeout when absent.
    #[serde(default)]
    timeout_ms: Option<u64>,
    /// Detail and region; spans of the whole viewport by default.
    #[serde(default)]
    projection: ProjectionSpec,
    /// Return an unsettled capture instead of failing.
    #[serde(default)]
    allow_incomplete: bool,
    /// Reaction required before the stable interval starts.
    #[serde(default)]
    reaction: Option<ReactionSpec>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WaitForParams {
    session: SessionRef,
    /// Predicate on the presented screen.
    predicate: ScreenPredicate,
    /// Required stable interval; none by default.
    #[serde(default)]
    stable_for_ms: Option<u64>,
    /// Actor-side bound; the default wait timeout when absent.
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TextParams {
    session: SessionRef,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WaitForTextParams {
    session: SessionRef,
    /// Text the presented screen must contain.
    pattern: String,
    /// Bound on the wait; the default wait timeout when absent.
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WaitForIdleParams {
    session: SessionRef,
    /// Bound on the wait; the default wait timeout when absent.
    #[serde(default)]
    timeout_ms: Option<u64>,
}

pub(super) fn register(registry: &mut Registry) {
    method(
        registry,
        "screen.frame",
        "Return the latest presented frame with the requested detail and region.",
        frame,
    );
    method(
        registry,
        "screen.frame_at",
        "Return the frame at a state point of a completed session, or the latest presented \
         frame of a live one.",
        frame_at,
    );
    method(
        registry,
        "screen.observe",
        "Return the complete observation of a state point.",
        observe,
    );
    method(
        registry,
        "screen.capture_stable",
        "Capture one exact frame once it stays unchanged, optionally after a reaction.",
        capture_stable,
    );
    method(
        registry,
        "screen.wait_for",
        "Wait on the actor for a screen predicate; an unmatched outcome is a result, not an \
         error.",
        wait_for,
    );
    method(
        registry,
        "screen.text",
        "Return the plain text of the latest presented frame.",
        text,
    );
    method(
        registry,
        "screen.wait_for_text",
        "Wait until the presented screen contains the text.",
        wait_for_text,
    );
    method(
        registry,
        "screen.wait_for_idle",
        "Wait until the presented screen stays unchanged for the default idle interval.",
        wait_for_idle,
    );
}

fn frame(call: &Call<'_>, params: FrameParams) -> Result<ScreenFrame, Failure> {
    return params
        .session
        .attach(call)?
        .frame(params.projection.into())
        .map_err(|error| return Failure::sdk(&error));
}

fn frame_at(call: &Call<'_>, params: FrameAtParams) -> Result<ScreenFrame, Failure> {
    return params
        .session
        .attach(call)?
        .frame_at(
            params.point,
            params.include_hidden,
            params.projection.into(),
        )
        .map_err(|error| return Failure::sdk(&error));
}

fn observe(call: &Call<'_>, params: ObserveParams) -> Result<Observation, Failure> {
    let point = params.point.unwrap_or(StatePoint::LatestPresented);

    return params
        .session
        .attach(call)?
        .observe(point, params.include_hidden)
        .map_err(|error| return Failure::sdk(&error));
}

fn capture_stable(
    call: &Call<'_>,
    params: CaptureStableParams,
) -> Result<Captured<StableScreenCapture>, Failure> {
    let reaction = params.reaction.map(|spec| {
        let mut reaction = Reaction::new();
        reaction.changed_from = spec.changed_from;
        reaction.timeout = millis(spec.timeout_ms, reaction.timeout);
        return reaction;
    });

    let options = StableCaptureOptions {
        stable_for: millis(params.stable_for_ms, defaults::IDLE_STABLE_FOR),
        timeout: millis(params.timeout_ms, defaults::WAIT_TIMEOUT),
        projection: params.projection.into(),
        allow_incomplete: params.allow_incomplete,
        reaction,
    };
    let capture = params
        .session
        .attach(call)?
        .capture_stable(options)
        .map_err(|error| return Failure::sdk(&error))?;

    return Ok(Captured::new(capture.clone(), capture.frame));
}

fn wait_for(call: &Call<'_>, params: WaitForParams) -> Result<Captured<ScreenWaitResult>, Failure> {
    let outcome = params
        .session
        .attach(call)?
        .wait_screen(
            params.predicate,
            millis(params.stable_for_ms, Duration::ZERO),
            millis(params.timeout_ms, defaults::WAIT_TIMEOUT),
        )
        .map_err(|error| return Failure::sdk(&error))?;

    return Ok(Captured::new(
        ScreenWaitResult::from(&outcome),
        outcome.frame,
    ));
}

fn text(call: &Call<'_>, params: TextParams) -> Result<String, Failure> {
    return params
        .session
        .attach(call)?
        .frame(ScreenProjectionRequest::default())
        .map(|frame| return frame_text(&frame))
        .map_err(|error| return Failure::sdk(&error));
}

fn wait_for_text(call: &Call<'_>, params: WaitForTextParams) -> Result<ResolvedState, Failure> {
    let predicate = ScreenPredicate::Contains {
        text: params.pattern.clone(),
    };
    let outcome = params
        .session
        .attach(call)?
        .wait_screen(
            predicate,
            Duration::ZERO,
            millis(params.timeout_ms, defaults::WAIT_TIMEOUT),
        )
        .map_err(|error| return Failure::sdk(&error))?;

    if !outcome.is_satisfied() {
        return Err(Failure::unsatisfied_wait(format!(
            "timed out waiting for pattern: '{}'",
            params.pattern
        )));
    }
    return Ok(ResolvedState::of(&outcome.frame));
}

fn wait_for_idle(call: &Call<'_>, params: WaitForIdleParams) -> Result<ResolvedState, Failure> {
    let options = StableCaptureOptions {
        timeout: millis(params.timeout_ms, defaults::WAIT_TIMEOUT),
        ..StableCaptureOptions::default()
    };

    return params
        .session
        .attach(call)?
        .capture_stable(options)
        .map(|capture| return ResolvedState::of(&capture.frame))
        .map_err(|error| return Failure::sdk(&error));
}
