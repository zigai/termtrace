//! Native Node.js and TypeScript bindings for Termtrace.

#![allow(
    unsafe_code,
    clippy::single_char_lifetime_names,
    clippy::implicit_return,
    clippy::string_slice,
    clippy::absolute_paths,
    clippy::redundant_clone,
    clippy::option_if_let_else,
    clippy::map_unwrap_or,
    reason = "NAPI-RS derive and function macros generate FFI code, single-char lifetimes, and implicit returns"
)]

mod actions;
mod control;
mod diagnostics;
mod evidence;
mod host;
mod locator;
mod native;
mod original;
pub mod scenario;
pub mod testing;
pub mod video;

use actions::{mutation_err, JsActionBuilder};
use control::{JsFocusOptions, JsMutationOptions, JsPointer, JsPointerOptions, JsScrollOptions};
pub use host::NativeValue;
use host::{Arg, Model};
use locator::{query_error, JsLocator, JsLocatorSpec, JsQueryResult};
use napi_derive::napi;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use termtrace_model::artifact::bundle::{BundleProfile, BundleVerificationPolicy};
use termtrace_model::artifact::save::SaveFormat;
use termtrace_model::protocol::control::ScreenPredicate;
use termtrace_model::protocol::mutation::MutationEffect;
use termtrace_model::terminal::observation::StatePoint;
use termtrace_model::terminal::screen::{
    FrameDetail, ScreenFrame, ScreenProjectionRequest, ScreenRegion,
};
use termtrace_model::terminal::state::KeyChord;
use termtrace_model::terminal::{
    observation::{CaptureQuality, Observation},
    screen::{ScreenFrameContent as ScreenFrameData, ScreenFrameSource, ScreenGeometry},
    state::{ColorState, CursorState, ModeState},
};
use termtrace_model::value::ExactPath;
use termtrace_sdk::analysis::AnalysisService;
use termtrace_sdk::artifacts::ArtifactService;
use termtrace_sdk::client::{Session, Terminal};
use termtrace_sdk::contracts::LaunchOptions;
use termtrace_sdk::contracts::StopReport;
use termtrace_sdk::defaults;
use termtrace_sdk::events::{EventCursor, EventsService};
use termtrace_sdk::input::{MutationGuard, Reaction};
use termtrace_sdk::integrity::IntegrityService;
use termtrace_sdk::locator::{LocatorSpec, ScreenFrameQuery, WaitCancellation};
use termtrace_sdk::output::{OutputCursor, OutputService};
use termtrace_sdk::registry::RegistryService;
use termtrace_sdk::screen::{frame_text, StableCaptureOptions};
use termtrace_sdk::session::{
    LaunchSpec, SessionTarget, StopOutcome, StopRequest, VisualProfile, VisualProfileReport,
};
use termtrace_sdk::system::SystemService;
use termtrace_sdk::tmux::TmuxService;
use termtrace_sdk::{ByteString, EnvironmentSpec, Uuid};

#[derive(serde::Serialize)]
struct Empty {}

/// Translate a Termtrace error; `index.js` lifts the typed `cause` into `kind`, `code`,
/// `effect`, and `retry` properties on the thrown error.
fn to_napi_err(err: termtrace_sdk::Error) -> napi::Error {
    #[derive(serde::Serialize)]
    struct EditDetails<'a> {
        details: &'a termtrace_sdk::artifacts::EditFailure,
    }

    let report = termtrace_sdk::contracts::ErrorReport::from(&err);

    if let Some(details) = err.edit_failure() {
        return typed_error_context(
            err.to_string(),
            &report.kind,
            &report.code,
            err.failure_context(),
            EditDetails { details },
        );
    }
    return typed_error_context(
        err.to_string(),
        &report.kind,
        &report.code,
        err.failure_context(),
        Empty {},
    );
}

/// A wait that ended without its condition holding; nothing was applied.
fn unsatisfied_wait(message: String) -> napi::Error {
    return typed_error(
        message,
        "wait_unsatisfied",
        "wait_unsatisfied",
        MutationEffect::NotApplied,
    );
}

fn typed_error(message: String, kind: &str, code: &str, effect: MutationEffect) -> napi::Error {
    return typed_error_with(message, kind, code, effect, Empty {});
}

/// [`typed_error`] with extra typed fields that `index.js` lifts onto the thrown error.
fn typed_error_with<T: serde::Serialize>(
    message: String,
    kind: &str,
    code: &str,
    effect: MutationEffect,
    extra: T,
) -> napi::Error {
    return typed_error_context(
        message,
        kind,
        code,
        termtrace_model::protocol::management::FailureContext::actor(effect),
        extra,
    );
}

fn typed_error_context<T: serde::Serialize>(
    message: String,
    kind: &str,
    code: &str,
    context: termtrace_model::protocol::management::FailureContext,
    extra: T,
) -> napi::Error {
    #[derive(serde::Serialize)]
    struct Failure<'a, T> {
        kind: &'a str,
        code: &'a str,
        #[serde(flatten)]
        context: termtrace_model::protocol::management::FailureContext,
        #[serde(flatten)]
        extra: T,
    }

    let typed = match host::encode(&Failure {
        kind,
        code,
        context,
        extra,
    }) {
        Ok(typed) => typed,
        Err(error) => return error,
    };

    let mut error = napi::Error::new(napi::Status::GenericFailure, message);
    error.cause = Some(Box::new(napi::Error::new(
        napi::Status::GenericFailure,
        format!("termtrace:{typed}"),
    )));

    return error;
}

fn blocking_failure(error: tokio::task::JoinError) -> napi::Error {
    return napi::Error::from_reason(error.to_string());
}

fn resolved_state(frame: &ScreenFrame) -> JsResolvedState {
    return JsResolvedState {
        hash: frame.state_hash.clone(),
        text: frame_text(frame),
        cursor_row: u32::from(frame.cursor.row.unwrap_or(0)),
        cursor_col: u32::from(frame.cursor.column.unwrap_or(0)),
        frame: Arc::new(frame.clone()),
    };
}

fn timeout_or_default(timeout_ms: Option<u32>) -> Duration {
    return timeout_ms.map_or(defaults::WAIT_TIMEOUT, |millis| {
        return Duration::from_millis(u64::from(millis));
    });
}

fn default_termtrace_bin() -> PathBuf {
    if let Ok(bin) = std::env::var("TERMTRACE_BIN") {
        return PathBuf::from(bin);
    }

    if let Ok(exe) = std::env::current_exe() {
        let candidate = exe.with_file_name("termtrace");
        if candidate.is_file() {
            return candidate;
        }
    }

    return PathBuf::from("termtrace");
}

fn default_evidence_root() -> PathBuf {
    if let Ok(root) = std::env::var("TERMTRACE_ROOT") {
        return PathBuf::from(root);
    }
    return std::env::temp_dir().join("termtrace");
}

/// Parse one key chord with the shared grammar, e.g. `Enter`, `ctrl+c`, `ctrl+shift+F5`.
fn parse_key(key: &str) -> napi::Result<KeyChord> {
    return KeyChord::parse(key)
        .map_err(|error| return napi::Error::from_reason(error.to_string()))?
        .ok_or_else(|| {
            return napi::Error::from_reason(format!(
                "unknown key name: '{key}'; send plain characters as text"
            ));
        });
}

/// Decode the typed payload `index.js` lifts out of a native error message.
///
/// # Errors
///
/// Raises `TypeError` when the payload is not valid encoded error data.
#[napi(skip_typescript)]
pub fn decode_error_data(json: String) -> napi::Result<NativeValue> {
    return serde_json::from_str(&json).map_err(|e| return host::type_error(e.to_string()));
}

/// Returns the Termtrace core version.
#[napi]
pub fn version() -> String {
    return "0.1.0".to_string();
}

/// Rich screen frame rendered against terminal geometry.
#[napi(js_name = "ScreenFrame")]
#[derive(Clone)]
pub struct JsFrame {
    /// Viewport rows.
    pub rows: u32,
    /// Viewport columns.
    pub cols: u32,
    /// Plain text of all rendered rows.
    pub text: String,
    pub(crate) frame: Arc<ScreenFrame>,
}

impl JsFrame {
    /// Wrap one exact frame.
    pub fn new(frame: Arc<ScreenFrame>) -> Self {
        return Self {
            rows: u32::from(frame.geometry.rows),
            cols: u32::from(frame.geometry.cols),
            text: frame_text(&frame),
            frame,
        };
    }
}

#[napi]
impl JsFrame {
    /// Projection format version.
    #[napi(getter)]
    pub fn version(&self) -> u32 {
        return self.frame.version;
    }

    /// Complete semantic state identity.
    #[napi(getter)]
    pub fn state_hash(&self) -> String {
        return self.frame.state_hash.clone();
    }

    /// Projection identity of this exact frame.
    #[napi(getter)]
    pub fn frame_hash(&self) -> String {
        return self.frame.frame_hash.clone();
    }

    /// Whether this state was presentation eligible.
    #[napi(getter)]
    pub fn presented(&self) -> bool {
        return self.frame.presented;
    }

    /// Settlement label.
    ///
    /// # Errors
    ///
    /// Fails when the value cannot be converted.
    #[napi(getter)]
    pub fn capture_quality(&self) -> Model<CaptureQuality> {
        return Model(self.frame.capture_quality);
    }

    /// Exact source occurrence, tagged by `source`.
    ///
    /// # Errors
    ///
    /// Fails when the value cannot be converted.
    #[napi(getter)]
    pub fn source(&self) -> Model<ScreenFrameSource> {
        return Model(self.frame.source.clone());
    }

    /// Full viewport geometry.
    ///
    /// # Errors
    ///
    /// Fails when the value cannot be converted.
    #[napi(getter)]
    pub fn geometry(&self) -> Model<ScreenGeometry> {
        return Model(self.frame.geometry);
    }

    /// Validated requested half-open region.
    ///
    /// # Errors
    ///
    /// Fails when the value cannot be converted.
    #[napi(getter)]
    pub fn requested_region(&self) -> Model<ScreenRegion> {
        return Model(self.frame.requested_region);
    }

    /// Actually returned half-open region.
    ///
    /// # Errors
    ///
    /// Fails when the value cannot be converted.
    #[napi(getter)]
    pub fn returned_region(&self) -> Model<ScreenRegion> {
        return Model(self.frame.returned_region);
    }

    /// Complete cursor state; `row`, `column`, and `wide_tail` are `null` when unknown.
    ///
    /// # Errors
    ///
    /// Fails when the value cannot be converted.
    #[napi(getter)]
    pub fn cursor(&self) -> Model<CursorState> {
        return Model(self.frame.cursor.clone());
    }

    /// Terminal modes, including the complete ANSI and DEC mode inventory.
    ///
    /// # Errors
    ///
    /// Fails when the value cannot be converted.
    #[napi(getter)]
    pub fn modes(&self) -> Model<ModeState> {
        return Model(self.frame.modes.clone());
    }

    /// Default colors, explicit cursor color, and the active 256-color palette.
    ///
    /// # Errors
    ///
    /// Fails when the value cannot be converted.
    #[napi(getter)]
    pub fn colors(&self) -> Model<ColorState> {
        return Model(self.frame.colors.clone());
    }

    /// Structured rows tagged by `detail`.
    ///
    /// # Errors
    ///
    /// Fails when the value cannot be converted.
    #[napi(getter)]
    pub fn content(&self) -> Model<ScreenFrameData> {
        return Model(self.frame.data.clone());
    }

    /// Evaluate a locator specification against this exact frame without capturing.
    ///
    /// # Errors
    ///
    /// Fails with a query kind when the specification does not fit this frame.
    #[napi]
    pub fn query(&self, spec: &JsLocatorSpec) -> napi::Result<JsQueryResult> {
        return self
            .frame
            .query(spec.spec.clone())
            .map(JsQueryResult::new)
            .map_err(|error| return query_error(&error));
    }

    /// Canonical model JSON of this frame; wide integers are plain JSON numbers.
    ///
    /// # Errors
    ///
    /// Fails when the frame cannot be serialized.
    #[napi]
    pub fn to_json(&self) -> napi::Result<String> {
        return serde_json::to_string(self.frame.as_ref())
            .map_err(|error| return napi::Error::from_reason(error.to_string()));
    }

    /// Decode canonical model JSON exactly, rejecting unknown or missing fields.
    ///
    /// # Errors
    ///
    /// Fails when `json` is not a valid screen frame.
    #[napi(factory)]
    pub fn from_json(json: String) -> napi::Result<Self> {
        return serde_json::from_str::<ScreenFrame>(&json)
            .map(|frame| return Self::new(Arc::new(frame)))
            .map_err(|error| return host::type_error(error.to_string()));
    }
}

/// Immutable snapshot of a resolved presented terminal state.
#[napi(js_name = "ResolvedState")]
#[derive(Clone)]
pub struct JsResolvedState {
    /// Content-addressed semantic state hash.
    pub hash: String,
    /// Plain text of the viewport.
    pub text: String,
    /// Zero-indexed cursor row, or 0 when the frame has no cursor row; see `cursor`.
    pub cursor_row: u32,
    /// Zero-indexed cursor column, or 0 when the frame has no cursor column; see `cursor`.
    pub cursor_col: u32,
    pub(crate) frame: Arc<ScreenFrame>,
}

#[napi]
impl JsResolvedState {
    /// Complete cursor state; `row`, `column`, and `wide_tail` are `null` when unknown.
    ///
    /// # Errors
    ///
    /// Fails when the value cannot be converted.
    #[napi(getter)]
    pub fn cursor(&self) -> Model<CursorState> {
        return Model(self.frame.cursor.clone());
    }

    /// The exact rich frame this state summarizes.
    #[napi(getter)]
    pub fn frame(&self) -> JsFrame {
        return JsFrame::new(Arc::clone(&self.frame));
    }
}

/// Detail and region of a projected frame.
#[napi(object, object_to_js = false, js_name = "FrameOptions")]
pub struct JsFrameOptions {
    /// `none`, `text`, `spans` (default), or `cells`, which requires `region`.
    pub detail: Option<Arg<FrameDetail>>,
    /// Half-open region; the whole viewport when omitted.
    pub region: Option<Arg<ScreenRegion>>,
}

/// Point, hidden-state selection, and projection of a resolved frame.
#[napi(object, object_to_js = false, js_name = "FrameAtOptions")]
pub struct JsFrameAtOptions {
    /// Whether presentation-ineligible states may be selected.
    pub include_hidden: Option<bool>,
    /// `none`, `text`, `spans` (default), or `cells`, which requires `region`.
    pub detail: Option<Arg<FrameDetail>>,
    /// Half-open region; the whole viewport when omitted.
    pub region: Option<Arg<ScreenRegion>>,
}

/// Point and hidden-state selection of an observation.
#[napi(object, object_to_js = false, js_name = "ObserveOptions")]
pub struct JsObserveOptions {
    /// State point; the latest presented state when omitted.
    pub point: Option<Arg<StatePoint>>,
    /// Whether presentation-ineligible states may be selected.
    pub include_hidden: Option<bool>,
}

/// Options of one stable capture.
#[napi(object, object_to_js = false, js_name = "CaptureStableOptions")]
pub struct JsCaptureStableOptions {
    /// Required unchanged interval in whole milliseconds.
    pub stable_for_ms: Option<f64>,
    /// Actor-side bound in whole milliseconds.
    pub timeout_ms: Option<f64>,
    /// `none`, `text`, `spans` (default), or `cells`, which requires `region`.
    pub detail: Option<Arg<FrameDetail>>,
    /// Half-open region; the whole viewport when omitted.
    pub region: Option<Arg<ScreenRegion>>,
    /// Return an unsettled capture instead of failing.
    pub allow_incomplete: Option<bool>,
    /// Reaction required before the stable interval starts.
    pub reaction: Option<Arg<termtrace_sdk::contracts::ReactionSpec>>,
}

/// Bounds of an actor-side screen wait.
#[napi(object, object_to_js = false, js_name = "ScreenWaitOptions")]
pub struct JsScreenWaitOptions {
    /// Required stable interval in whole milliseconds.
    pub stable_for_ms: Option<f64>,
    /// Actor-side bound in whole milliseconds.
    pub timeout_ms: Option<f64>,
}

/// Literal-text locator options.
#[napi(object, object_to_js = false, js_name = "TextLocatorOptions")]
pub struct JsTextLocatorOptions {
    /// Compare the complete row or region text instead of searching substrings.
    pub exact: Option<bool>,
}

fn projection(
    detail: Option<Arg<FrameDetail>>,
    region: Option<Arg<ScreenRegion>>,
) -> ScreenProjectionRequest {
    return ScreenProjectionRequest {
        detail: detail.map(|v| v.0).unwrap_or_default(),
        region: region.map(|v| v.0),
    };
}

fn with_frame<T>(details: T, frame: ScreenFrame) -> JsCaptured<T> {
    return JsCaptured {
        details: Model(details),
        frame: JsFrame::new(Arc::new(frame)),
    };
}

/// Model fields plus the exact frame they describe, returned as one object.
pub struct JsCaptured<T> {
    details: Model<T>,
    frame: JsFrame,
}

impl<T: serde::Serialize> napi::bindgen_prelude::ToNapiValue for JsCaptured<T> {
    unsafe fn to_napi_value(
        env: napi::sys::napi_env,
        val: Self,
    ) -> napi::Result<napi::sys::napi_value> {
        // SAFETY: napi supplies a live env; both values are created in it during this call.
        unsafe {
            let details = Model::to_napi_value(env, val.details)?;
            let mut object = napi::bindgen_prelude::Object::from_raw(env, details);
            object.set("frame", val.frame)?;

            return napi::bindgen_prelude::Object::to_napi_value(env, object);
        }
    }
}

/// Result of reading a page of raw child PTY output bytes.
#[napi(object, object_from_js = false)]
pub struct JsOutputPage {
    /// Raw bytes produced by the child process.
    pub bytes: Model<ByteString>,
    /// Byte offset for fetching the subsequent page.
    pub next_offset: Model<u64>,
    /// Whether additional bytes remain beyond this page.
    pub has_more: Model<bool>,
}

/// Screen inspection capability attached to a live or completed session.
#[napi(js_name = "Screen")]
#[derive(Clone)]
pub struct JsScreen {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: PathBuf,
}

impl JsScreen {
    fn bind(&self, spec: LocatorSpec) -> JsLocator {
        return JsLocator::new(self.terminal.clone(), self.session_path.clone(), spec);
    }
}

#[napi]
impl JsScreen {
    /// Return the latest presented screen frame with the requested detail and region.
    ///
    /// # Errors
    ///
    /// Fails when `options` is malformed, the session cannot be attached, the frame cannot be
    /// projected, or the background worker panics.
    #[napi]
    pub async fn frame(&self, options: Option<JsFrameOptions>) -> napi::Result<JsFrame> {
        let projection = match options {
            Some(options) => projection(options.detail, options.region),
            None => ScreenProjectionRequest::default(),
        };
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();

        return tokio::task::spawn_blocking(move || {
            return terminal
                .attach(path)
                .and_then(|session| return session.frame(projection))
                .map(|frame| return JsFrame::new(Arc::new(frame)))
                .map_err(to_napi_err);
        })
        .await
        .map_err(blocking_failure)?;
    }

    /// Return a frame at any state point of a completed session, or the latest presented
    /// state of a live one.
    ///
    /// # Errors
    ///
    /// Fails when an argument is malformed, a live session is asked for another point or hidden
    /// states, resolution fails, or the background worker panics.
    #[napi]
    pub async fn frame_at(
        &self,
        point: Arg<StatePoint>,
        options: Option<JsFrameAtOptions>,
    ) -> napi::Result<JsFrame> {
        let point = point.0;
        let (include_hidden, projection) = match options {
            Some(options) => (
                options.include_hidden.unwrap_or(false),
                projection(options.detail, options.region),
            ),
            None => (false, ScreenProjectionRequest::default()),
        };
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();

        return tokio::task::spawn_blocking(move || {
            return terminal
                .attach(path)
                .and_then(|session| return session.frame_at(point, include_hidden, projection))
                .map(|frame| return JsFrame::new(Arc::new(frame)))
                .map_err(to_napi_err);
        })
        .await
        .map_err(blocking_failure)?;
    }

    /// Return the complete observation of a state point (latest presented by default).
    ///
    /// # Errors
    ///
    /// Fails when `point` is malformed, a live session is asked for another point, observation
    /// fails, or the background worker panics.
    #[napi]
    pub async fn observe(
        &self,
        options: Option<JsObserveOptions>,
    ) -> napi::Result<Model<Observation>> {
        let (point, include_hidden) = match options {
            Some(options) => (
                options.point.map(|point| point.0),
                options.include_hidden.unwrap_or(false),
            ),
            None => (None, false),
        };
        let point = point.unwrap_or(StatePoint::LatestPresented);
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();

        return tokio::task::spawn_blocking(move || {
            let observation = terminal
                .attach(path)
                .and_then(|session| return session.observe(point, include_hidden))
                .map_err(to_napi_err)?;

            return Ok(Model(observation));
        })
        .await
        .map_err(blocking_failure)?;
    }

    /// Capture one exact frame once it stays unchanged, optionally after a reaction.
    ///
    /// # Errors
    ///
    /// Fails when an option is malformed, the capture fails or, unless `allowIncomplete`, does
    /// not settle, or the background worker panics.
    #[napi]
    pub async fn capture_stable(
        &self,
        options: Option<JsCaptureStableOptions>,
    ) -> napi::Result<JsCaptured<termtrace_sdk::screen::StableScreenCapture>> {
        let options = options.unwrap_or(JsCaptureStableOptions {
            stable_for_ms: None,
            timeout_ms: None,
            detail: None,
            region: None,
            allow_incomplete: None,
            reaction: None,
        });
        let reaction = options
            .reaction
            .map(|reaction| {
                let reaction = reaction.0;
                let mut wire = Reaction::new();
                wire.changed_from = reaction.changed_from;
                let milliseconds = reaction
                    .timeout_ms
                    .map(|ms| {
                        return u32::try_from(ms)
                            .map(f64::from)
                            .map_err(|e| host::range_error(e.to_string()));
                    })
                    .transpose()?;
                wire.timeout = host::millis(milliseconds, wire.timeout)?;

                return Ok::<_, napi::Error>(wire);
            })
            .transpose()?;
        let capture_options = StableCaptureOptions {
            stable_for: host::millis(options.stable_for_ms, defaults::IDLE_STABLE_FOR)?,
            timeout: host::millis(options.timeout_ms, defaults::WAIT_TIMEOUT)?,
            projection: projection(options.detail, options.region),
            allow_incomplete: options.allow_incomplete.unwrap_or(false),
            reaction,
        };
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();

        return tokio::task::spawn_blocking(move || {
            let capture = terminal
                .attach(path)
                .and_then(|session| return session.capture_stable(capture_options))
                .map_err(to_napi_err)?;

            return Ok(with_frame(capture.clone(), capture.frame));
        })
        .await
        .map_err(blocking_failure)?;
    }

    /// Wait on the actor for a screen predicate and resolve with the outcome even when it is
    /// unmatched; `quality` tells a deadline from child completion or output closure.
    ///
    /// # Errors
    ///
    /// Fails when an argument is malformed, the actor rejects the predicate, the wait fails, or
    /// the background worker panics.
    #[napi]
    pub async fn wait_for(
        &self,
        predicate: Arg<ScreenPredicate>,
        options: Option<JsScreenWaitOptions>,
    ) -> napi::Result<JsCaptured<termtrace_sdk::screen::ScreenWaitResult>> {
        let predicate = predicate.0;
        let (stable_for, timeout) = match options {
            Some(options) => (
                host::millis(options.stable_for_ms, Duration::ZERO)?,
                host::millis(options.timeout_ms, defaults::WAIT_TIMEOUT)?,
            ),
            None => (Duration::ZERO, defaults::WAIT_TIMEOUT),
        };
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();

        return tokio::task::spawn_blocking(move || {
            let outcome = terminal
                .attach(path)
                .and_then(|session| return session.wait_screen(predicate, stable_for, timeout))
                .map_err(to_napi_err)?;

            return Ok(with_frame(
                termtrace_sdk::screen::ScreenWaitResult::from(&outcome),
                outcome.frame,
            ));
        })
        .await
        .map_err(blocking_failure)?;
    }

    /// Bind a locator specification without capturing; the default matches every cell.
    #[napi]
    pub fn locator(&self, spec: Option<&JsLocatorSpec>) -> JsLocator {
        return self.bind(
            spec.map(|spec| return spec.spec.clone())
                .unwrap_or_default(),
        );
    }

    /// Bind a nonempty literal text locator.
    ///
    /// # Errors
    ///
    /// Fails with kind `invalid_query` when `text` is empty.
    #[napi]
    pub fn get_by_text(
        &self,
        text: String,
        options: Option<JsTextLocatorOptions>,
    ) -> napi::Result<JsLocator> {
        let exact = options
            .and_then(|options| return options.exact)
            .unwrap_or(false);
        let spec = if exact { LocatorSpec::exact(text) } else { LocatorSpec::text(text) };

        return spec
            .map(|spec| return self.bind(spec))
            .map_err(|error| return query_error(&error));
    }

    /// Bind a Rust regex syntax locator.
    ///
    /// # Errors
    ///
    /// Fails for anything but a string (including `RegExp`), and with kind `invalid_query` when
    /// the pattern is invalid or can match zero-width text.
    #[napi]
    pub fn get_by_regex(&self, pattern: Arg<String>) -> napi::Result<JsLocator> {
        return locator::regex_spec(pattern).map(|spec| return self.bind(spec));
    }

    /// Bind one absolute row.
    ///
    /// # Errors
    ///
    /// Fails when `row` is not a whole number from 0 through 65535.
    #[napi]
    pub fn row(&self, row: f64) -> napi::Result<JsLocator> {
        let row = host::cell_coordinate(row, "row")?;

        return Ok(self.bind(LocatorSpec::default().in_row(row)));
    }

    /// Bind one nonempty absolute half-open region.
    ///
    /// # Errors
    ///
    /// Fails when `region` is malformed, and with kind `invalid_query` when it is empty.
    #[napi]
    pub fn region(&self, region: Arg<ScreenRegion>) -> napi::Result<JsLocator> {
        let region = region.0;

        return LocatorSpec::default()
            .in_region(region)
            .map(|spec| return self.bind(spec))
            .map_err(|error| return query_error(&error));
    }

    /// Bind a cell appearance filter.
    ///
    /// # Errors
    ///
    /// Fails with kind `invalid_query` when `style` is malformed.
    #[napi]
    pub fn filter(
        &self,
        style: Arg<termtrace_sdk::locator::StyleFilter>,
    ) -> napi::Result<JsLocator> {
        let style = style.0;

        return Ok(self.bind(LocatorSpec::default().with_style(style)));
    }

    /// Return the plain text of the latest presented frame.
    ///
    /// # Errors
    ///
    /// Fails when the session cannot be attached, the frame cannot be captured, or the background
    /// worker panics.
    #[napi]
    pub async fn text(&self) -> napi::Result<String> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();

        return tokio::task::spawn_blocking(move || {
            return terminal
                .attach(path)
                .and_then(|session| return session.frame(ScreenProjectionRequest::default()))
                .map(|frame| return frame_text(&frame))
                .map_err(to_napi_err);
        })
        .await
        .map_err(blocking_failure)?;
    }

    /// Wait until the presented screen contains the specified text pattern.
    ///
    /// # Errors
    ///
    /// Fails when the session cannot be attached, the text is not presented before the timeout, or
    /// the background worker panics.
    #[napi]
    pub async fn wait_for_text(
        &self,
        pattern: String,
        timeout_ms: Option<u32>,
    ) -> napi::Result<JsResolvedState> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();
        let timeout = timeout_or_default(timeout_ms);

        return tokio::task::spawn_blocking(move || {
            let predicate = ScreenPredicate::Contains {
                text: pattern.clone(),
            };
            let outcome = terminal
                .attach(path)
                .and_then(|session| {
                    return session.wait_screen(predicate, Duration::ZERO, timeout);
                })
                .map_err(to_napi_err)?;

            if !outcome.is_satisfied() {
                return Err(unsatisfied_wait(format!(
                    "timed out waiting for pattern: '{pattern}'"
                )));
            }
            return Ok(resolved_state(&outcome.frame));
        })
        .await
        .map_err(blocking_failure)?;
    }

    /// Wait until the presented screen remains unchanged for the default idle interval.
    ///
    /// # Errors
    ///
    /// Fails when the session cannot be attached, the screen does not settle before the timeout, or
    /// the background worker panics.
    #[napi]
    pub async fn wait_for_idle(&self, timeout_ms: Option<u32>) -> napi::Result<JsResolvedState> {
        let terminal = self.terminal.clone();
        let path = self.session_path.clone();
        let timeout = timeout_or_default(timeout_ms);

        return tokio::task::spawn_blocking(move || {
            return terminal
                .attach(path)
                .and_then(|session| {
                    return session.capture_stable(StableCaptureOptions {
                        timeout,
                        ..StableCaptureOptions::default()
                    });
                })
                .map(|capture| return resolved_state(&capture.frame))
                .map_err(to_napi_err);
        })
        .await
        .map_err(blocking_failure)?;
    }
}

/// A logical Termtrace session reference.
#[napi(js_name = "Session")]
#[derive(Clone)]
pub struct JsSession {
    pub(crate) terminal: Terminal,
    pub(crate) session_path: PathBuf,
    pub(crate) session_id: String,
}

#[napi]
impl JsSession {
    /// Canonical session identity string.
    #[napi(getter)]
    pub fn session_id(&self) -> String {
        return self.session_id.clone();
    }

    /// Filesystem path to the session directory, for display when it is not UTF-8.
    #[napi(getter)]
    pub fn path(&self) -> String {
        return self.session_path.display().to_string();
    }

    /// Exact session directory bytes.
    #[napi(getter)]
    pub fn path_bytes(&self) -> napi::bindgen_prelude::Uint8Array {
        return napi::bindgen_prelude::Uint8Array::from(self.session_path.as_os_str().as_bytes());
    }

    /// Explicit application instrumentation with one process instance per emitter.
    #[napi]
    pub fn instrumentation(&self) -> diagnostics::JsInstrumentation {
        return diagnostics::JsInstrumentation::new(Arg(ExactPath(self.session_path.clone())));
    }

    /// Typed analysis operations through the shared service.
    #[napi]
    pub fn analysis(&self) -> diagnostics::JsDiagnostics {
        return diagnostics::JsDiagnostics {
            path: self.session_path.clone(),
        };
    }

    /// Typed evidence operations through the shared service.
    #[napi]
    pub fn evidence(&self) -> diagnostics::JsEvidence {
        return diagnostics::JsEvidence {
            path: self.session_path.clone(),
        };
    }

    /// Native replay operations through the shared service.
    #[napi]
    pub fn native(&self) -> native::JsNative {
        return native::JsNative {
            terminal: self.terminal.clone(),
            path: self.session_path.clone(),
        };
    }

    /// Typed integrity operations through the shared service.
    #[napi]
    pub fn integrity(&self) -> diagnostics::JsEvidence {
        return diagnostics::JsEvidence {
            path: self.session_path.clone(),
        };
    }

    /// Source-bound exact PTY output pages.
    #[napi]
    pub fn output(&self) -> evidence::JsOutput {
        return evidence::JsOutput {
            path: self.session_path.clone(),
        };
    }

    /// Source-bound filtered canonical event projections.
    #[napi]
    pub fn events(&self) -> evidence::JsEvents {
        return evidence::JsEvents {
            path: self.session_path.clone(),
        };
    }

    /// Access screen inspection capabilities.
    #[napi]
    pub fn screen(&self) -> JsScreen {
        return JsScreen {
            terminal: self.terminal.clone(),
            session_path: self.session_path.clone(),
        };
    }

    /// Create an action builder for input automation.
    #[napi]
    pub fn actions(&self) -> JsActionBuilder {
        return JsActionBuilder::new(self.terminal.clone(), self.session_path.clone());
    }

    /// Mode-aware pointer, focus, and text-activation operations with typed results.
    #[napi]
    pub fn pointer(&self) -> JsPointer {
        return JsPointer {
            terminal: self.terminal.clone(),
            session_path: self.session_path.clone(),
        };
    }

    /// Legacy adapter: click a mouse button at a terminal cell through `pointer().click`,
    /// discarding the typed result.
    ///
    /// # Errors
    ///
    /// Fails as `Pointer.click`.
    #[napi]
    pub async fn pointer_click(
        &self,
        column: u32,
        row: u32,
        button: Option<Arg<termtrace_sdk::MouseButton>>,
        strict: Option<bool>,
    ) -> napi::Result<()> {
        let options = JsPointerOptions {
            button,
            strict,
            request_id: None,
            if_state: None,
        };
        let _result = self
            .pointer()
            .click(f64::from(column), f64::from(row), Some(options))
            .await?;

        return Ok(());
    }

    /// Legacy adapter: scroll the mouse wheel at a terminal cell through `pointer().scroll`,
    /// discarding the typed result.
    ///
    /// # Errors
    ///
    /// Fails as `Pointer.scroll`.
    #[napi]
    pub async fn pointer_scroll(
        &self,
        column: u32,
        row: u32,
        lines: Option<i32>,
        strict: Option<bool>,
    ) -> napi::Result<()> {
        let options = JsScrollOptions {
            lines,
            strict,
            request_id: None,
            if_state: None,
        };
        let _result = self
            .pointer()
            .scroll(f64::from(column), f64::from(row), Some(options))
            .await?;

        return Ok(());
    }

    /// Legacy adapter: report terminal focus through `pointer().focus`, discarding the typed
    /// result.
    ///
    /// # Errors
    ///
    /// Fails as `Pointer.focus`.
    #[napi]
    pub async fn pointer_focus(&self, focused: bool, strict: Option<bool>) -> napi::Result<()> {
        let options = JsFocusOptions {
            strict,
            request_id: None,
            if_state: None,
        };
        let _result = self.pointer().focus(focused, Some(options)).await?;

        return Ok(());
    }

    /// Resize the child PTY.
    ///
    /// # Errors
    ///
    /// Fails for an invalid geometry or request identity, and rejects with `request_id` when the
    /// recorder rejects it.
    #[napi]
    pub async fn resize(
        &self,
        cols: f64,
        rows: f64,
        options: Option<JsMutationOptions>,
    ) -> napi::Result<Model<termtrace_sdk::ControlAcknowledgement>> {
        return control::resize(
            self.terminal.clone(),
            self.session_path.clone(),
            cols,
            rows,
            options,
        )
        .await;
    }

    /// Send one allow-listed signal to the child process group.
    ///
    /// # Errors
    ///
    /// Fails for an unknown signal or invalid request identity, and rejects with `request_id`
    /// when the recorder rejects it.
    #[napi]
    pub async fn signal(
        &self,
        signal: Arg<termtrace_sdk::ProcessSignal>,
        options: Option<JsMutationOptions>,
    ) -> napi::Result<Model<termtrace_sdk::ControlAcknowledgement>> {
        return control::signal(
            self.terminal.clone(),
            self.session_path.clone(),
            signal,
            options,
        )
        .await;
    }

    /// Append a canonical marker.
    ///
    /// # Errors
    ///
    /// Fails for an invalid request identity, and rejects with `request_id` when the recorder
    /// rejects it.
    #[napi]
    pub async fn mark(
        &self,
        label: String,
        options: Option<JsMutationOptions>,
    ) -> napi::Result<Model<termtrace_sdk::MarkOutcome>> {
        return control::mark(
            self.terminal.clone(),
            self.session_path.clone(),
            label,
            options,
        )
        .await;
    }

    /// Apply ordered steps, each a separate mutation with an identity derived from the send's.
    ///
    /// # Errors
    ///
    /// Fails for malformed steps or options, and on a failed step rejects with the step's error
    /// carrying `step`, its derived `request_id`, and `send_request_id`; earlier steps were
    /// applied.
    #[napi]
    pub async fn send(
        &self,
        spec: Arg<termtrace_sdk::contracts::SendSpec>,
    ) -> napi::Result<control::SendResult> {
        return control::send(self.terminal.clone(), self.session_path.clone(), spec).await;
    }

    /// Live status `{ kind: 'live', ... }` or completion `{ kind: 'completed', ... }`.
    ///
    /// # Errors
    ///
    /// Rejects when the session cannot be attached or read.
    #[napi]
    pub async fn status(&self) -> napi::Result<Model<termtrace_sdk::SessionStatus>> {
        return control::status(self.terminal.clone(), self.session_path.clone()).await;
    }

    /// Atomically publish visual representations (PNG, SVG, HTML, TXT, JSON) of a state point.
    ///
    /// # Errors
    ///
    /// Fails when a format name is unknown, the state cannot be resolved or rendered, the
    /// destination cannot be published, or the background worker panics.
    #[napi]
    pub async fn save(
        &self,
        destination: String,
        formats: Option<Arg<Vec<SaveFormat>>>,
        state_hash: Option<String>,
        include_hidden: Option<bool>,
    ) -> napi::Result<Model<termtrace_model::artifact::save::SaveManifest>> {
        let session_path = self.session_path.clone();
        let dest = PathBuf::from(destination);
        let save_formats = formats.map(|formats| formats.0).unwrap_or_else(|| {
            return vec![
                SaveFormat::Png,
                SaveFormat::Svg,
                SaveFormat::Txt,
                SaveFormat::Json,
            ];
        });
        let point = state_hash.map_or(StatePoint::LatestPresented, |h| {
            return StatePoint::StateHash { state_hash: h };
        });
        let hidden = include_hidden.unwrap_or(false);

        return tokio::task::spawn_blocking(move || {
            let outcome = ArtifactService::save(&session_path, &dest, point, &save_formats, hidden)
                .map_err(to_napi_err)?;

            return Ok(Model(outcome.manifest));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Export session terminal playback as an asciicast v2 file.
    ///
    /// # Errors
    ///
    /// Fails when the event log cannot be read or the cast cannot be written, or the background
    /// worker panics.
    #[napi]
    pub async fn export_asciicast(&self, output_path: String) -> napi::Result<()> {
        let session_path = self.session_path.clone();
        let target = PathBuf::from(output_path);

        return tokio::task::spawn_blocking(move || {
            ArtifactService::export_asciicast(&session_path, &target).map_err(to_napi_err)?;

            return Ok(());
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Create a compressed evidence bundle (.tar.zst) for archiving or forensics.
    ///
    /// # Errors
    ///
    /// Fails when the bundle cannot be created or published, or the background worker panics.
    #[napi]
    pub async fn create_bundle(&self, output_path: String) -> napi::Result<()> {
        let session_path = self.session_path.clone();
        let target = PathBuf::from(output_path);

        return tokio::task::spawn_blocking(move || {
            ArtifactService::create_bundle(&session_path, &target, BundleProfile::Forensic)
                .map_err(to_napi_err)?;

            return Ok(());
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Compute semantic difference between two presented states and return JSON.
    ///
    /// # Errors
    ///
    /// Fails when either state hash cannot be resolved, the session's evidence cannot be validated
    /// or replayed, the report cannot be serialized, or the background worker panics.
    #[napi]
    pub async fn diff(
        &self,
        from_hash: String,
        to_hash: String,
    ) -> napi::Result<Model<termtrace_model::terminal::diff::SemanticDiff>> {
        let session_path = self.session_path.clone();

        return tokio::task::spawn_blocking(move || {
            let from = StatePoint::StateHash {
                state_hash: from_hash,
            };
            let to = StatePoint::StateHash {
                state_hash: to_hash,
            };
            let (diff, _, _) =
                AnalysisService::diff(&session_path, from, to).map_err(to_napi_err)?;

            return Ok(Model(diff));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Read raw child PTY output bytes using an optional byte offset cursor.
    ///
    /// # Errors
    ///
    /// Fails when `offset` is negative or beyond the JavaScript safe-integer range, the output page
    /// cannot be read, or the background worker panics.
    #[napi]
    pub async fn read_output(
        &self,
        offset: Option<Arg<u64>>,
        limit: Option<u32>,
    ) -> napi::Result<JsOutputPage> {
        let offset = offset.map(|value| value.0);
        let session_path = self.session_path.clone();
        let cursor = offset
            .map(|off| {
                return Ok::<_, napi::Error>(OutputCursor {
                    session_path: session_path.clone(),
                    byte_offset: off,
                    event_seq: None,
                });
            })
            .transpose()?;
        let limit_val = usize::try_from(limit.unwrap_or(65536)).unwrap_or(65536);

        return tokio::task::spawn_blocking(move || {
            let page =
                OutputService::read_page(&session_path, cursor, limit_val).map_err(to_napi_err)?;

            return Ok(JsOutputPage {
                bytes: Model(ByteString::new(page.bytes)),
                next_offset: Model(page.cursor.byte_offset),
                has_more: Model(page.has_more),
            });
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Wait for exact child output bytes to appear in the canonical stream.
    ///
    /// # Errors
    ///
    /// Fails when `pattern` is empty, the event log is invalid, the child exits or the session
    /// completes before the bytes appear, the timeout elapses, or the background worker panics.
    #[napi]
    pub async fn wait_for_output(
        &self,
        pattern: String,
        timeout_ms: Option<u32>,
    ) -> napi::Result<bool> {
        let session_path = self.session_path.clone();
        let timeout = timeout_or_default(timeout_ms);

        return tokio::task::spawn_blocking(move || {
            let _outcome =
                OutputService::wait_for_bytes(&session_path, pattern.as_bytes(), timeout)
                    .map_err(to_napi_err)?;

            return Ok(true);
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Read canonical event records starting at an optional event sequence.
    ///
    /// # Errors
    ///
    /// Fails when `cursor` is negative or beyond the JavaScript safe-integer range, the event page
    /// cannot be read or serialized, or the background worker panics.
    #[napi]
    pub async fn read_events(
        &self,
        cursor: Option<Arg<u64>>,
        limit: Option<u32>,
    ) -> napi::Result<Model<Vec<termtrace_model::execution::event::Event>>> {
        let cursor = cursor.map(|value| value.0);
        let session_path = self.session_path.clone();
        let event_cursor = cursor
            .map(|seq| {
                return Ok::<_, napi::Error>(EventCursor {
                    session_path: session_path.clone(),
                    next_event_seq: seq,
                });
            })
            .transpose()?;
        let limit_val = usize::try_from(limit.unwrap_or(100)).unwrap_or(100);

        return tokio::task::spawn_blocking(move || {
            let page = EventsService::read_page(&session_path, event_cursor, limit_val)
                .map_err(to_napi_err)?;

            return Ok(Model(page.events));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Explain VT and semantic evidence around a target state point.
    ///
    /// # Errors
    ///
    /// Fails when the target state cannot be resolved, the session's evidence cannot be validated
    /// or replayed, the report cannot be serialized, or the background worker panics.
    #[napi]
    pub async fn explain(
        &self,
        target_hash: Option<String>,
        context: Option<u32>,
    ) -> napi::Result<Model<termtrace_model::terminal::analysis::ExplainReport>> {
        let session_path = self.session_path.clone();
        let point = target_hash.map_or(StatePoint::LatestPresented, |h| {
            return StatePoint::StateHash { state_hash: h };
        });
        let ctx = u64::from(context.unwrap_or(1));

        return tokio::task::spawn_blocking(move || {
            let report =
                AnalysisService::explain(&session_path, point, ctx).map_err(to_napi_err)?;

            return Ok(Model(report));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Run conservative evidence-backed rendering diagnostics on the session.
    ///
    /// # Errors
    ///
    /// Fails when visual or canonical evidence fails validation or replay, the report cannot be
    /// serialized, or the background worker panics.
    #[napi]
    pub async fn diagnose(
        &self,
        broad_change_percent: Option<u32>,
    ) -> napi::Result<Model<termtrace_model::terminal::analysis::DiagnoseReport>> {
        let session_path = self.session_path.clone();
        let pct = u8::try_from(broad_change_percent.unwrap_or(25)).unwrap_or(25);

        return tokio::task::spawn_blocking(move || {
            let report =
                AnalysisService::diagnose(&session_path, Vec::new(), pct).map_err(to_napi_err)?;

            return Ok(Model(report));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Perform full triage analysis on the most recent canonical action.
    ///
    /// # Errors
    ///
    /// Fails when the most recent action cannot be analyzed from validated evidence, the report
    /// cannot be serialized, or the background worker panics.
    #[napi]
    pub async fn last_action(
        &self,
        broad_change_percent: Option<u32>,
    ) -> napi::Result<Model<termtrace_model::terminal::analysis::LastActionAnalysis>> {
        let session_path = self.session_path.clone();
        let pct = u8::try_from(broad_change_percent.unwrap_or(25)).unwrap_or(25);

        return tokio::task::spawn_blocking(move || {
            let analysis = AnalysisService::last_action(&session_path, pct).map_err(to_napi_err)?;

            return Ok(Model(analysis));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Build a bounded page of timeline entries.
    ///
    /// # Errors
    ///
    /// Fails when the session's evidence cannot be read, validated, or replayed, the page cannot be
    /// serialized, or the background worker panics.
    #[napi]
    pub async fn timeline_page(
        &self,
        offset: Option<u32>,
        limit: Option<u32>,
        include_hidden: Option<bool>,
        include_controls: Option<bool>,
    ) -> napi::Result<Model<termtrace_model::terminal::timeline::TimelinePage>> {
        let session_path = self.session_path.clone();
        let off = u64::from(offset.unwrap_or(0));
        let lim = u16::try_from(limit.unwrap_or(50)).unwrap_or(50);
        let hidden = include_hidden.unwrap_or(false);
        let controls = include_controls.unwrap_or(false);

        return tokio::task::spawn_blocking(move || {
            let page =
                AnalysisService::timeline_page(&session_path, hidden, controls, None, off, lim)
                    .map_err(to_napi_err)?;

            return Ok(Model(page));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }
}

/// An active recording session owning child processes and evidence buffers.
#[napi(js_name = "Recording")]
#[derive(Clone)]
pub struct JsRecording {
    pub(crate) terminal: Terminal,
    pub(crate) session: Arc<Mutex<Option<Session>>>,
    pub(crate) session_id: String,
    pub(crate) path: String,
    pub(crate) finalization: Arc<Mutex<Option<termtrace_sdk::testing::CachedFinalization>>>,
    pub(crate) lifecycle: Arc<Session>,
}

#[napi]
impl JsRecording {
    /// Unique session identifier.
    #[napi(getter)]
    pub fn session_id(&self) -> String {
        return self.session_id.clone();
    }

    /// Filesystem path to the canonical session directory.
    #[napi(getter)]
    pub fn path(&self) -> String {
        return self.path.clone();
    }

    /// Check if the recording session is still active.
    #[napi]
    pub fn is_active(&self) -> bool {
        let guard = self
            .session
            .lock()
            .unwrap_or_else(|e| return e.into_inner());
        return guard
            .as_ref()
            .is_some_and(|session| !session.lifecycle_finished())
            || (self.lifecycle.lifecycle_force_requested()
                && !self.lifecycle.lifecycle_finished());
    }

    /// Explicit application instrumentation with one process instance per emitter.
    #[napi]
    pub fn instrumentation(&self) -> diagnostics::JsInstrumentation {
        return diagnostics::JsInstrumentation::new(Arg(ExactPath(PathBuf::from(&self.path))));
    }

    /// Typed analysis operations through the shared service.
    #[napi]
    pub fn analysis(&self) -> diagnostics::JsDiagnostics {
        return diagnostics::JsDiagnostics {
            path: PathBuf::from(&self.path),
        };
    }

    /// Typed evidence operations through the shared service.
    #[napi]
    pub fn evidence(&self) -> diagnostics::JsEvidence {
        return diagnostics::JsEvidence {
            path: PathBuf::from(&self.path),
        };
    }

    /// Native replay operations through the shared service.
    #[napi]
    pub fn native(&self) -> native::JsNative {
        return native::JsNative {
            terminal: self.terminal.clone(),
            path: PathBuf::from(&self.path),
        };
    }

    /// Typed integrity operations through the shared service.
    #[napi]
    pub fn integrity(&self) -> diagnostics::JsEvidence {
        return diagnostics::JsEvidence {
            path: PathBuf::from(&self.path),
        };
    }

    /// Source-bound exact PTY output pages.
    #[napi]
    pub fn output(&self) -> evidence::JsOutput {
        return evidence::JsOutput {
            path: PathBuf::from(&self.path),
        };
    }

    /// Source-bound filtered canonical event projections.
    #[napi]
    pub fn events(&self) -> evidence::JsEvents {
        return evidence::JsEvents {
            path: PathBuf::from(&self.path),
        };
    }

    /// Access screen inspection capabilities.
    ///
    /// # Errors
    /// Fails when the recording has already been stopped or detached.
    #[napi]
    pub fn screen(&self) -> napi::Result<JsScreen> {
        if !self.is_active() {
            return Err(napi::Error::from_reason(
                "recording has already been stopped or detached",
            ));
        }

        return Ok(JsScreen {
            terminal: self.terminal.clone(),
            session_path: PathBuf::from(&self.path),
        });
    }

    /// Create an action builder for input automation.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached.
    #[napi]
    pub fn actions(&self) -> napi::Result<JsActionBuilder> {
        if !self.is_active() {
            return Err(napi::Error::from_reason(
                "recording has already been stopped or detached",
            ));
        }

        return Ok(JsActionBuilder::new(
            self.terminal.clone(),
            PathBuf::from(&self.path),
        ));
    }

    /// Get a logical session reference.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached.
    #[napi]
    pub fn session(&self) -> napi::Result<JsSession> {
        if !self.is_active() {
            return Err(napi::Error::from_reason(
                "recording has already been stopped or detached",
            ));
        }

        return Ok(JsSession {
            terminal: self.terminal.clone(),
            session_path: PathBuf::from(&self.path),
            session_id: self.session_id.clone(),
        });
    }

    /// Mode-aware pointer, focus, and text-activation operations with typed results.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached.
    #[napi]
    pub fn pointer(&self) -> napi::Result<JsPointer> {
        return Ok(self.session()?.pointer());
    }

    /// Legacy adapter: click a mouse button at a terminal cell.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached, and otherwise as
    /// `Pointer.click`.
    #[napi]
    pub async fn pointer_click(
        &self,
        column: u32,
        row: u32,
        button: Option<Arg<termtrace_sdk::MouseButton>>,
        strict: Option<bool>,
    ) -> napi::Result<()> {
        let sess = self.session()?;

        return sess.pointer_click(column, row, button, strict).await;
    }

    /// Legacy adapter: scroll the mouse wheel at a terminal cell.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached, and otherwise as
    /// `Pointer.scroll`.
    #[napi]
    pub async fn pointer_scroll(
        &self,
        column: u32,
        row: u32,
        lines: Option<i32>,
        strict: Option<bool>,
    ) -> napi::Result<()> {
        let sess = self.session()?;

        return sess.pointer_scroll(column, row, lines, strict).await;
    }

    /// Legacy adapter: report terminal focus state.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached, and otherwise as
    /// `Pointer.focus`.
    #[napi]
    pub async fn pointer_focus(&self, focused: bool, strict: Option<bool>) -> napi::Result<()> {
        let sess = self.session()?;

        return sess.pointer_focus(focused, strict).await;
    }

    /// Resize the child PTY.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached, and otherwise as
    /// `Session.resize`.
    #[napi]
    pub async fn resize(
        &self,
        cols: f64,
        rows: f64,
        options: Option<JsMutationOptions>,
    ) -> napi::Result<Model<termtrace_sdk::ControlAcknowledgement>> {
        let sess = self.session()?;

        return sess.resize(cols, rows, options).await;
    }

    /// Send one allow-listed signal to the child process group.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached, and otherwise as
    /// `Session.signal`.
    #[napi]
    pub async fn signal(
        &self,
        signal: Arg<termtrace_sdk::ProcessSignal>,
        options: Option<JsMutationOptions>,
    ) -> napi::Result<Model<termtrace_sdk::ControlAcknowledgement>> {
        let sess = self.session()?;

        return sess.signal(signal, options).await;
    }

    /// Append a canonical marker.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached, and otherwise as
    /// `Session.mark`.
    #[napi]
    pub async fn mark(
        &self,
        label: String,
        options: Option<JsMutationOptions>,
    ) -> napi::Result<Model<termtrace_sdk::MarkOutcome>> {
        let sess = self.session()?;

        return sess.mark(label, options).await;
    }

    /// Apply ordered steps, each a separate mutation with an identity derived from the send's.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached, and otherwise as
    /// `Session.send`.
    #[napi]
    pub async fn send(
        &self,
        spec: Arg<termtrace_sdk::contracts::SendSpec>,
    ) -> napi::Result<control::SendResult> {
        let sess = self.session()?;

        return sess.send(spec).await;
    }

    /// Live status `{ kind: 'live', ... }` or completion `{ kind: 'completed', ... }`, also after
    /// stop.
    ///
    /// # Errors
    ///
    /// Rejects when the session cannot be attached or read.
    #[napi]
    pub async fn status(&self) -> napi::Result<Model<termtrace_sdk::SessionStatus>> {
        return control::status(self.terminal.clone(), PathBuf::from(&self.path)).await;
    }

    /// Atomically publish visual representations (PNG, SVG, HTML, TXT, JSON) of a state point.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached, and otherwise as
    /// `JsSession::save`.
    #[napi]
    pub async fn save(
        &self,
        destination: String,
        formats: Option<Arg<Vec<SaveFormat>>>,
        state_hash: Option<String>,
        include_hidden: Option<bool>,
    ) -> napi::Result<Model<termtrace_model::artifact::save::SaveManifest>> {
        let sess = self.session()?;

        return sess
            .save(destination, formats, state_hash, include_hidden)
            .await;
    }

    /// Read raw child PTY output bytes using an optional byte offset cursor.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached, and otherwise as
    /// `JsSession::read_output`.
    #[napi]
    pub async fn read_output(
        &self,
        offset: Option<Arg<u64>>,
        limit: Option<u32>,
    ) -> napi::Result<JsOutputPage> {
        let sess = self.session()?;

        return sess.read_output(offset, limit).await;
    }

    /// Wait for exact child output bytes to appear in the canonical stream.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached, and otherwise as
    /// `JsSession::wait_for_output`.
    #[napi]
    pub async fn wait_for_output(
        &self,
        pattern: String,
        timeout_ms: Option<u32>,
    ) -> napi::Result<bool> {
        let sess = self.session()?;

        return sess.wait_for_output(pattern, timeout_ms).await;
    }

    /// Read canonical event records starting at an optional event sequence.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached, and otherwise as
    /// `JsSession::read_events`.
    #[napi]
    pub async fn read_events(
        &self,
        cursor: Option<Arg<u64>>,
        limit: Option<u32>,
    ) -> napi::Result<Model<Vec<termtrace_model::execution::event::Event>>> {
        let sess = self.session()?;

        return sess.read_events(cursor, limit).await;
    }

    /// Explain VT and semantic evidence around a target state point.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached, and otherwise as
    /// `JsSession::explain`.
    #[napi]
    pub async fn explain(
        &self,
        target_hash: Option<String>,
        context: Option<u32>,
    ) -> napi::Result<Model<termtrace_model::terminal::analysis::ExplainReport>> {
        let sess = self.session()?;

        return sess.explain(target_hash, context).await;
    }

    /// Run conservative evidence-backed rendering diagnostics on the session.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached, and otherwise as
    /// `JsSession::diagnose`.
    #[napi]
    pub async fn diagnose(
        &self,
        broad_change_percent: Option<u32>,
    ) -> napi::Result<Model<termtrace_model::terminal::analysis::DiagnoseReport>> {
        let sess = self.session()?;

        return sess.diagnose(broad_change_percent).await;
    }

    /// Perform full triage analysis on the most recent canonical action.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached, and otherwise as
    /// `JsSession::last_action`.
    #[napi]
    pub async fn last_action(
        &self,
        broad_change_percent: Option<u32>,
    ) -> napi::Result<Model<termtrace_model::terminal::analysis::LastActionAnalysis>> {
        let sess = self.session()?;

        return sess.last_action(broad_change_percent).await;
    }

    /// Legacy adapter: stop the recording session through `requestStop` and wait for validated
    /// completion evidence.
    ///
    /// # Errors
    ///
    /// Fails when the recorder does not stop with validated completion within the timeout, leaving
    /// the recording attached, or the background worker panics.
    #[napi]
    pub async fn stop(&self, timeout_ms: Option<u32>) -> napi::Result<()> {
        let timeout = timeout_ms.map_or(defaults::STOP_TIMEOUT, |millis| {
            return Duration::from_millis(u64::from(millis));
        });
        let _outcome = self.stop_request(Uuid::now_v7(), Some(timeout)).await?;

        return Ok(());
    }

    /// Ask the recording to end; with `waitMs`, wait that long for validated completion.
    ///
    /// Resolves with `{ kind: 'requested', ... }` or `{ kind: 'completed', ... }`. A failed request
    /// leaves the recording attached; pass `requestId` to retry it with the same identity.
    ///
    /// # Errors
    ///
    /// Fails when already stopped or detached or `requestId` is invalid, and rejects with
    /// `request_id` when the stop is rejected or completion is not published in time.
    #[napi]
    pub async fn request_stop(
        &self,
        options: Option<StopOptions>,
    ) -> napi::Result<Model<StopReport>> {
        let (request_id, wait_ms) = options.map_or((None, None), |options| {
            return (options.request_id, options.wait_ms);
        });
        let request_id =
            actions::request_id_arg(request_id.as_deref())?.unwrap_or_else(Uuid::now_v7);
        let wait = wait_ms.map(|millis| return Duration::from_millis(u64::from(millis)));
        let outcome = self.stop_request(request_id, wait).await?.ok_or_else(|| {
            return napi::Error::from_reason("recording has already been stopped or detached");
        })?;

        return Ok(Model(StopReport {
            request_id,
            outcome,
        }));
    }

    /// Wait for validated completion evidence and reap the recorder; resolves with the completion.
    ///
    /// # Errors
    ///
    /// Fails when already stopped or detached, and rejects when completion is not published in
    /// time, leaving the recording attached.
    #[napi]
    pub async fn wait_for_completion(
        &self,
        timeout_ms: Option<u32>,
    ) -> napi::Result<Model<termtrace_model::execution::session::SessionCompletion>> {
        let timeout = timeout_ms.map_or(defaults::STOP_TIMEOUT, |millis| {
            return Duration::from_millis(u64::from(millis));
        });
        let mut session = self.take().ok_or_else(|| {
            return napi::Error::from_reason("recording has already been stopped or detached");
        })?;

        let (session, result) = tokio::task::spawn_blocking(move || {
            let result = session.complete(timeout);
            return (session, result);
        })
        .await
        .map_err(blocking_failure)?;

        return match result {
            Ok(completion) => Ok(Model(completion)),
            Err(error) => {
                self.restore(session);
                Err(to_napi_err(error))
            }
        };
    }

    /// Delete this completed canonical session, reaping an owned recorder.
    ///
    /// # Errors
    ///
    /// Rejects when completion is missing or invalid or the session cannot be removed durably;
    /// an owned recording stays attached.
    #[napi]
    pub async fn discard_completed(&self) -> napi::Result<()> {
        let owned = self.take();
        let attached = owned.is_some();
        let terminal = self.terminal.clone();
        let path = self.path.clone();

        let (session, result) = tokio::task::spawn_blocking(move || {
            let mut session = match owned {
                Some(session) => session,
                None => match terminal.attach(path) {
                    Ok(session) => session,
                    Err(error) => return (None, Err(error)),
                },
            };
            let result = session.discard_completed();
            return (Some(session), result);
        })
        .await
        .map_err(blocking_failure)?;

        if let Err(error) = result {
            if attached && let Some(session) = session {
                self.restore(session);
            }
            return Err(to_napi_err(error));
        }

        return Ok(());
    }

    /// Forcefully terminate and reap the recorded process group and owned recorder.
    ///
    /// # Errors
    ///
    /// Fails when the process group or recorder cannot be terminated and reaped, leaving the
    /// recording attached.
    #[napi]
    pub fn force_close(&self) -> napi::Result<()> {
        let mut session = match self.take() {
            Some(session) => session,
            None if self.lifecycle.lifecycle_force_requested() => {
                self.lifecycle.lifecycle_observer()
            }
            None => return Ok(()),
        };

        if let Err(error) = session.force_close() {
            self.restore(session);
            return Err(to_napi_err(error));
        }

        return Ok(());
    }

    /// Explicit resource management disposal for `await using`.
    ///
    /// # Errors
    ///
    /// Fails as `JsRecording::stop` with the default timeout.
    #[napi]
    pub async fn async_dispose(&self) -> napi::Result<()> {
        return self.stop(None).await;
    }

    /// Detach the recording so it continues running in the background.
    ///
    /// # Errors
    ///
    /// Fails when the recording has already been stopped or detached.
    #[napi]
    pub fn detach(&self) -> napi::Result<()> {
        let session = self.take().ok_or_else(|| {
            return napi::Error::from_reason("recording has already been stopped or detached");
        })?;
        let _path = session.detach();

        return Ok(());
    }
}

impl JsRecording {
    async fn stop_request(
        &self,
        request_id: Uuid,
        wait: Option<Duration>,
    ) -> napi::Result<Option<StopOutcome>> {
        let Some(mut session) = self.take() else {
            return Ok(None);
        };

        let request = StopRequest {
            guard: MutationGuard {
                request_id: Some(request_id),
                if_state: None,
            },
        };

        let (session, result) = tokio::task::spawn_blocking(move || {
            let result = session.stop(request, wait);

            return (session, result);
        })
        .await
        .map_err(blocking_failure)?;

        return match result {
            Ok(StopOutcome::Completed { completion }) => {
                Ok(Some(StopOutcome::Completed { completion }))
            }
            Ok(requested @ StopOutcome::Requested { .. }) => {
                self.restore(session);

                Ok(Some(requested))
            }
            Err(error) => {
                self.restore(session);

                Err(mutation_err(error, request_id))
            }
        };
    }

    fn take(&self) -> Option<Session> {
        return self
            .session
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner())
            .take();
    }

    fn restore(&self, session: Session) {
        if session.lifecycle_finished() || session.lifecycle_force_requested() {
            return;
        }

        *self
            .session
            .lock()
            .unwrap_or_else(|poisoned| return poisoned.into_inner()) = Some(session);
    }
}

/// Identity and wait of one stop request.
#[napi(object, object_to_js = false, js_name = "StopOptions")]
pub struct StopOptions {
    /// Identity of this stop, to retry it safely.
    pub request_id: Option<String>,
    /// Wait this long for validated completion; without it, resolve once the stop is accepted.
    pub wait_ms: Option<u32>,
}

/// Primary entry point for launching and managing Termtrace recordings in Node.js.
#[napi(js_name = "Termtrace")]
#[derive(Clone)]
pub struct JsTermtrace {
    pub(crate) terminal: Terminal,
}

#[napi]
impl JsTermtrace {
    /// Typed system service, without starting probes on construction.
    #[napi]
    pub fn system(&self) -> diagnostics::JsSystem {
        return diagnostics::JsSystem::new();
    }
    /// Instrumentation bound to one live session.
    #[napi]
    pub fn instrumentation(&self, source: Arg<ExactPath>) -> diagnostics::JsInstrumentation {
        return diagnostics::JsInstrumentation::new(source);
    }

    /// Aggregate tmux recording with an explicit root, tmux executable, and socket.
    #[napi]
    pub fn tmux(
        &self,
        root: Option<Arg<ExactPath>>,
        tmux: Option<Arg<ExactPath>>,
        socket: Option<Arg<ExactPath>>,
        socket_name: Option<bool>,
    ) -> diagnostics::JsTmux {
        return diagnostics::JsTmux::new(
            root,
            Some(Arg(ExactPath(self.terminal.binary().to_path_buf()))),
            tmux,
            socket,
            socket_name,
        );
    }

    /// Registry operations at an explicit or the default state root.
    #[napi]
    pub fn registry(&self, root: Option<Arg<ExactPath>>) -> diagnostics::JsRegistry {
        return diagnostics::JsRegistry::new(
            root,
            Some(Arg(ExactPath(self.terminal.binary().to_path_buf()))),
        );
    }

    /// Replay analysis of one canonical session.
    #[napi]
    pub fn analysis(&self, source: Arg<ExactPath>) -> diagnostics::JsDiagnostics {
        return diagnostics::JsDiagnostics::new(source);
    }

    /// Evidence artifacts and integrity operations for one canonical session.
    #[napi]
    pub fn evidence(&self, source: Arg<ExactPath>) -> diagnostics::JsEvidence {
        return diagnostics::JsEvidence::new(source);
    }

    /// Run scenario documents as one suite with this client's binary. A document with a
    /// `matrix` expands to one labelled case per combination; with a shard, case `i` of the
    /// sorted expansion runs only when `i % shardCount == shardIndex`. Up to `jobs` cases run at
    /// once, each in a fresh session. A failed case runs again up to `reruns` times; a later
    /// pass makes it `flaky`, which fails CI only with `failOnFlaky`.
    ///
    /// # Errors
    ///
    /// Rejects when the options are invalid, a document cannot be read or expanded, or the
    /// shard is empty. Case failures stay in the report.
    #[napi]
    pub async fn run_scenarios(
        &self,
        scenarios: Vec<Arg<ExactPath>>,
        options: scenario::JsScenarioSuiteOptions,
    ) -> napi::Result<Model<termtrace_model::scenario::ScenarioSuiteReport>> {
        return scenario::run(&self.terminal, scenarios, options).await;
    }

    /// Native replay operations for one canonical session.
    #[napi]
    pub fn native(&self, source: Arg<ExactPath>) -> native::JsNative {
        return native::JsNative {
            terminal: self.terminal.clone(),
            path: source.0 .0,
        };
    }

    /// Create a new Termtrace instance with optional binary and evidence root paths.
    #[napi(constructor)]
    pub fn new(binary: Option<String>, evidence_root: Option<String>) -> Self {
        let bin_path = binary.map_or_else(default_termtrace_bin, PathBuf::from);
        let root_path = evidence_root.map_or_else(default_evidence_root, PathBuf::from);
        return Self {
            terminal: Terminal::new(bin_path, root_path),
        };
    }

    /// Launch a new recorded command session.
    ///
    /// # Errors
    ///
    /// Fails when `rows` or `cols` exceeds 65535, the command cannot be launched and recorded, or
    /// the background worker panics.
    #[napi]
    pub async fn record(
        &self,
        command: Vec<String>,
        rows: Option<u32>,
        cols: Option<u32>,
        env: Option<HashMap<String, String>>,
        cwd: Option<String>,
    ) -> napi::Result<JsRecording> {
        let geometry = |value: Option<u32>, name: &str| {
            return value
                .map(|value| {
                    return u16::try_from(value).map_err(|error| {
                        return napi::Error::from_reason(format!(
                            "{name} {value} exceeds 65535: {error}"
                        ));
                    });
                })
                .transpose();
        };
        let environment = EnvironmentSpec {
            set: env
                .unwrap_or_default()
                .into_iter()
                .map(|(name, value)| return (name, ByteString::new(value.into_bytes())))
                .collect(),
            ..EnvironmentSpec::default()
        };
        let options = LaunchOptions {
            rows: geometry(rows, "rows")?,
            cols: geometry(cols, "cols")?,
            cwd,
            environment: Some(environment),
            ..LaunchOptions::default()
        };

        return self
            .start(control::launch_spec_from(command, &options)?)
            .await;
    }

    /// Validate a visual profile and report the identity of everything a launch with it
    /// would use. A semantic-only profile runs no native program; a native section is
    /// resolved by Ghostty and the `cols` by `rows` grid is solved at the profile's DPI.
    /// Advisory only.
    ///
    /// # Errors
    ///
    /// Fails for a malformed profile, and rejects with the visual-profile failure code when it
    /// is invalid or does not resolve.
    #[napi]
    pub async fn preflight_visual_profile(
        &self,
        profile: Arg<VisualProfile>,
        cols: Option<u32>,
        rows: Option<u32>,
    ) -> napi::Result<Model<VisualProfileReport>> {
        let grid = |value: Option<u32>, name: &str, default: u16| {
            return value.map_or(Ok(default), |value| {
                return u16::try_from(value).map_err(|error| {
                    return napi::Error::from_reason(format!(
                        "{name} {value} exceeds 65535: {error}"
                    ));
                });
            });
        };
        let cols = grid(cols, "cols", defaults::COLS)?;
        let rows = grid(rows, "rows", defaults::ROWS)?;
        let terminal = self.terminal.clone();

        return tokio::task::spawn_blocking(move || {
            return terminal
                .preflight_visual_profile(&profile.0, cols, rows)
                .map(Model)
                .map_err(to_napi_err);
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Launch a recorded command with typed options: `rows`, `cols`, `cwd`, `environment`
    /// (`base`, `set` name to bytes, `copy`, `unset`, `color`), `startup_timeout_ms`,
    /// `output_byte_budget`, `max_duration_ms`, `idle_timeout_ms`, and `visual_profile`.
    ///
    /// Resolves once the recorder answers or the session already completed. A cleared
    /// environment still receives the reserved terminal variables.
    ///
    /// # Errors
    ///
    /// Fails for unknown or malformed options, and rejects when the options are invalid or the
    /// command cannot be launched and recorded.
    #[napi]
    pub async fn launch(
        &self,
        command: Vec<String>,
        options: Option<Arg<LaunchOptions>>,
    ) -> napi::Result<JsRecording> {
        return self.start(control::launch_spec(command, options)?).await;
    }

    /// Launch `command` as an original recording: it runs in a private Ghostty display whose
    /// pixels are captured from before its first byte.
    ///
    /// Options: `grid` or `viewport`, `canvas`, `fps` (24, 30 or 60), `cwd`, `environment`,
    /// `ghostty_config` or `visual_profile`, `alias`, `state_root`, `startup_timeout_ms`, `output_byte_budget`,
    /// `max_duration_ms` and `idle_timeout_ms`. Cancelling `cancel` tears startup down within
    /// a bounded deadline; the promise settles only after that.
    ///
    /// # Errors
    ///
    /// Fails for malformed options, and rejects with `original_capture_failed` when
    /// acquisition fails or is cancelled, or when native capture is unavailable.
    #[napi]
    pub async fn record_original(
        &self,
        command: Vec<String>,
        options: Option<Arg<termtrace_sdk::contracts::OriginalLaunchOptions>>,
        cancel: Option<&locator::JsCancelToken>,
    ) -> napi::Result<original::JsOriginalRecording> {
        let spec = original::original_launch_spec(command, options)?;

        return original::start_original(self.terminal.clone(), spec, cancel.cloned()).await;
    }

    /// Attach to an original recording without owning its capture supervisor.
    ///
    /// # Errors
    ///
    /// Rejects when the session or its capture status is missing or invalid.
    #[napi]
    pub async fn attach_original(
        &self,
        session_path: String,
    ) -> napi::Result<original::JsOriginalRecording> {
        let terminal = self.terminal.clone();

        return tokio::task::spawn_blocking(move || {
            let original = terminal
                .attach_original(&session_path)
                .map_err(to_napi_err)?;

            return Ok(original::JsOriginalRecording::new(terminal, original));
        })
        .await
        .map_err(blocking_failure)?;
    }

    /// Attach to an original recording named by path or registry alias, without owning its
    /// capture supervisor. Strings and bytes follow the `path:`/`name:` rules; aliases resolve
    /// under `stateRoot`, or the default registry.
    ///
    /// # Errors
    ///
    /// Rejects when the target does not resolve, or the session or its capture status is
    /// missing, invalid, not owner-only, or published for another session.
    #[napi]
    pub async fn attach_original_target(
        &self,
        target: original::PathInput,
        state_root: Option<original::PathInput>,
    ) -> napi::Result<original::JsOriginalRecording> {
        let target = original::session_target(&target)?;
        let state_root = original::state_root(state_root.as_ref());
        let terminal = self.terminal.clone();

        return tokio::task::spawn_blocking(move || {
            let original = terminal
                .attach_original_target(&target, state_root.as_deref())
                .map_err(to_napi_err)?;

            return Ok(original::JsOriginalRecording::new(terminal, original));
        })
        .await
        .map_err(blocking_failure)?;
    }

    /// Attach to a session named by path or registry alias; `path:` and `name:` prefixes are
    /// explicit.
    ///
    /// # Errors
    ///
    /// Rejects when the target does not resolve or cannot be attached.
    #[napi]
    pub async fn attach_target(&self, target: String) -> napi::Result<JsSession> {
        let terminal = self.terminal.clone();

        return tokio::task::spawn_blocking(move || {
            let target = SessionTarget::parse(OsStr::new(&target)).map_err(to_napi_err)?;
            let session = terminal.attach_target(&target).map_err(to_napi_err)?;

            return Ok(JsSession {
                session_id: session.session_id().to_string(),
                session_path: session.path().to_path_buf(),
                terminal,
            });
        })
        .await
        .map_err(blocking_failure)?;
    }
}

impl JsTermtrace {
    async fn start(&self, spec: LaunchSpec) -> napi::Result<JsRecording> {
        let terminal = self.terminal.clone();

        return tokio::task::spawn_blocking(move || {
            let session = terminal.start(spec).map_err(to_napi_err)?;
            let session_id = session.session_id().to_string();
            let path = session.path().display().to_string();
            let lifecycle = Arc::new(session.lifecycle_observer());

            return Ok(JsRecording {
                lifecycle,
                finalization: Arc::new(Mutex::new(None)),
                terminal: terminal.clone(),
                session: Arc::new(Mutex::new(Some(session))),
                session_id,
                path,
            });
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }
}

#[napi]
impl JsTermtrace {
    /// Attach to an existing session directory.
    ///
    /// # Errors
    ///
    /// Fails when `session_path` is not an attachable session or the background worker panics.
    #[napi]
    pub async fn attach(&self, session_path: String) -> napi::Result<JsSession> {
        let terminal = self.terminal.clone();
        let path = PathBuf::from(session_path);

        return tokio::task::spawn_blocking(move || {
            let session = terminal.attach(&path).map_err(to_napi_err)?;
            let session_id = session.session_id().to_string();
            let session_path = session.path().to_path_buf();

            return Ok(JsSession {
                terminal,
                session_path,
                session_id,
            });
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Verify an evidence bundle (.tar.zst or directory) and return whether it passed integrity verification.
    ///
    /// # Errors
    ///
    /// Fails only when the background worker panics; failed verification returns `false`.
    #[napi]
    pub async fn verify_bundle(
        &self,
        bundle_path: String,
        source_session: Option<String>,
    ) -> napi::Result<bool> {
        let path = PathBuf::from(bundle_path);
        let source = source_session.map_or_else(
            || {
                return path.join("canonical");
            },
            PathBuf::from,
        );

        return tokio::task::spawn_blocking(move || {
            let policy = BundleVerificationPolicy::new(&source, BundleProfile::Forensic);
            let res = ArtifactService::verify_bundle(&path, policy);

            return Ok(res.is_ok());
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Verify a completed session's structure and evidence bindings offline.
    ///
    /// # Errors
    ///
    /// Fails only when the report cannot be serialized or the background worker panics;
    /// verification findings are part of the returned report.
    #[napi]
    pub async fn verify_session(
        &self,
        session_path: String,
        level: Option<Arg<termtrace_model::execution::session::VerificationLevel>>,
    ) -> napi::Result<Model<termtrace_model::execution::session::VerificationReport>> {
        let path = PathBuf::from(session_path);
        let lvl = level
            .map(|v| v.0)
            .unwrap_or(termtrace_model::execution::session::VerificationLevel::Content);

        return tokio::task::spawn_blocking(move || {
            let report = IntegrityService::verify(&path, lvl, None);

            return Ok(Model(report));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Recover the longest integrity-checked event prefix from a damaged session.
    ///
    /// # Errors
    ///
    /// Fails when no integrity-checked prefix can be recovered and published to `destination`, the
    /// report cannot be serialized, or the background worker panics.
    #[napi]
    pub async fn recover_session(
        &self,
        session_path: String,
        destination: String,
    ) -> napi::Result<Model<termtrace_model::execution::event_log::RecoveryReport>> {
        let src = PathBuf::from(session_path);
        let dest = PathBuf::from(destination);

        return tokio::task::spawn_blocking(move || {
            let report = IntegrityService::recover_session(&src, &dest).map_err(to_napi_err)?;

            return Ok(Model(report));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// List registered sessions in the default user registry.
    ///
    /// # Errors
    ///
    /// Fails when the default registry cannot be opened or read, the entries cannot be serialized,
    /// or the background worker panics.
    #[napi]
    pub async fn registry_list(
        &self,
        include_all: Option<bool>,
    ) -> napi::Result<Model<Vec<termtrace_model::registry::RegistryListEntry>>> {
        let all = include_all.unwrap_or(false);

        return tokio::task::spawn_blocking(move || {
            let reg = RegistryService::open_default().map_err(to_napi_err)?;
            let entries = reg.list(all).map_err(to_napi_err)?;

            return Ok(Model(entries));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Resolve a friendly session name to its registered session path.
    ///
    /// # Errors
    ///
    /// Fails when the default registry cannot be opened or the background worker panics; an
    /// unresolvable name returns `null`.
    #[napi]
    pub async fn registry_resolve(&self, name: String) -> napi::Result<Option<String>> {
        return tokio::task::spawn_blocking(move || {
            let reg = RegistryService::open_default().map_err(to_napi_err)?;
            match reg.resolve_optional(&name).map_err(to_napi_err)? {
                Some(entry) => {
                    return Ok(Some(entry.session_path().display().to_string()));
                }
                None => {
                    return Ok(None);
                }
            }
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Register a friendly name for an existing canonical session directory.
    ///
    /// # Errors
    ///
    /// Fails when the default registry cannot be opened, `name` is invalid or already registered,
    /// `session_path` is not a registrable session, or the background worker panics.
    #[napi]
    pub async fn registry_register(
        &self,
        name: String,
        session_path: String,
    ) -> napi::Result<Model<termtrace_model::registry::RegistryEntry>> {
        let path = PathBuf::from(session_path);

        return tokio::task::spawn_blocking(move || {
            let reg = RegistryService::open_default().map_err(to_napi_err)?;
            let entry = reg.register(&name, &path).map_err(to_napi_err)?;

            return Ok(Model(entry));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Remove a friendly name binding.
    ///
    /// # Errors
    ///
    /// Fails when the default registry cannot be opened or the background worker panics; a failed
    /// removal returns `false`.
    #[napi]
    pub async fn registry_remove(&self, name: String) -> napi::Result<bool> {
        return tokio::task::spawn_blocking(move || {
            let reg = RegistryService::open_default().map_err(to_napi_err)?;
            let res = reg.remove(&name);

            return Ok(res.is_ok());
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Plan or execute garbage collection over completed sessions.
    ///
    /// # Errors
    ///
    /// Fails when the default registry cannot be opened, garbage collection cannot be planned or
    /// applied, the report cannot be serialized, or the background worker panics.
    #[napi]
    pub async fn registry_gc(
        &self,
        older_than_days: Option<u32>,
        keep_latest: Option<u32>,
        max_completed_bytes: Option<u32>,
        apply: Option<bool>,
    ) -> napi::Result<Model<termtrace_model::registry::GarbageCollectionReport>> {
        let days = older_than_days.map_or(defaults::GC_OLDER_THAN_DAYS, u64::from);
        let keep = keep_latest.map_or(defaults::GC_KEEP_LATEST, u64::from);
        let quota = max_completed_bytes.map(u64::from);
        let is_apply = apply.unwrap_or(false);

        return tokio::task::spawn_blocking(move || {
            let reg = RegistryService::open_default().map_err(to_napi_err)?;
            let report = reg.gc(days, keep, quota, is_apply).map_err(to_napi_err)?;

            return Ok(Model(report));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Bundle an aggregate tmux recording into an evidentiary streaming bundle.
    ///
    /// # Errors
    ///
    /// Fails when the aggregate tmux recording cannot be bundled or published, the report cannot be
    /// serialized, or the background worker panics.
    #[napi]
    pub async fn tmux_bundle(
        &self,
        manifest_path: String,
        destination: String,
    ) -> napi::Result<Model<termtrace_sdk::tmux::TmuxStreamingBundleManifest>> {
        let manifest = PathBuf::from(manifest_path);
        let dest = PathBuf::from(destination);

        return tokio::task::spawn_blocking(move || {
            let report = TmuxService::bundle(&manifest, &dest).map_err(to_napi_err)?;

            return Ok(Model(report));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Verify an aggregate tmux streaming bundle against its manifest.
    ///
    /// # Errors
    ///
    /// Fails only when the background worker panics; failed verification returns `false`.
    #[napi]
    pub async fn tmux_verify_bundle(
        &self,
        bundle_directory: String,
        manifest_path: String,
    ) -> napi::Result<bool> {
        let bundle_dir = PathBuf::from(bundle_directory);
        let manifest = PathBuf::from(manifest_path);

        return tokio::task::spawn_blocking(move || {
            let res = TmuxService::verify_bundle(&bundle_dir, &manifest);

            return Ok(res.is_ok());
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }

    /// Probe runtime system capabilities across engine, media, and evidence signers.
    ///
    /// Cancelling `cancel` stops the native capture probe early; it then reports blocked.
    ///
    /// # Errors
    ///
    /// Fails only when the report cannot be serialized or the background worker panics.
    #[napi]
    pub async fn probe_capabilities(
        &self,
        cancel: Option<&locator::JsCancelToken>,
    ) -> napi::Result<Model<termtrace_model::protocol::capability::CapabilityReport>> {
        let token = cancel.map(|token| return token.inner.clone());

        return tokio::task::spawn_blocking(move || {
            let report =
                WaitCancellation::forward(token.as_ref(), SystemService::probe_capabilities);

            return Ok(Model(report));
        })
        .await
        .map_err(|e| return napi::Error::from_reason(e.to_string()))?;
    }
}
