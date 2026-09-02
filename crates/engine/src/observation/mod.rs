//! Stable, truth-labeled terminal observation and replay caches.

pub use termtrace_model::terminal::observation::{
    CaptureQuality, FrameEnvelope, Observation, ObservationFrame, ObservationIntegrity,
    ObservationResolution, ObservationSourceKind, ReplayCachePruneReport, ReplayCacheStatus,
    StatePoint, TruthLayer,
};
pub use termtrace_terminal::project_screen_frame;
pub use termtrace_terminal::screen::{
    normalize_screen_frame_cells, normalize_screen_frame_spans, normalize_screen_frame_text,
};

mod ansi;
mod cache;
mod model;
mod session;

pub use ansi::{format_ansi_stream, observe_ansi_stream, semantic_state_from_ansi};
pub(crate) use cache::publish_endpoint_checkpoint;
use cache::{CachedSemanticState, load_checkpoint, publish_checkpoint};
pub use cache::{prune_replay_cache, replay_cache_status};
use model::observation_error;
pub use model::state_text;
use model::{frame_from_state, frame_hash};
pub use session::{
    observation_from_live_snapshot, observe_completed_with_formats, observe_one_off_session,
    observe_session, observe_session_with_options, observe_validated_prefix,
    resolved_screen_state_for_session, semantic_state_for_session,
    semantic_state_from_live_snapshot, semantic_state_with_hash_for_session,
};
