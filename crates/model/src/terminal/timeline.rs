//! Stable ordered timeline contracts.

use crate::{execution::event::Event, terminal::state::DirtyState, terminal::vt::VtToken};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One ordered, source-bound timeline item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelineEntry {
    /// Canonical session offset.
    pub monotonic_ns: u64,
    /// Responsible canonical event, absent for the initial semantic state.
    pub event_seq: Option<u64>,
    /// Timeline item payload.
    pub item: TimelineItem,
}

/// Concise canonical or semantic item in a diagnostic timeline.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimelineItem {
    /// A distinct Ghostty semantic state transition.
    SemanticState {
        /// Dense semantic state identifier.
        state_id: u64,
        /// Byte offset within the responsible output event.
        event_byte_offset: Option<u64>,
        /// Complete normalized semantic state hash.
        state_hash: String,
        /// Whether the state was eligible for presentation.
        presented: bool,
        /// Ghostty's dirty classification.
        ghostty_dirty: DirtyState,
        /// Rows whose semantic cells changed.
        changed_rows: Vec<u16>,
        /// Number of complete semantic cell values changed.
        changed_cells: u64,
        /// Whether cursor state changed.
        cursor_changed: bool,
        /// Whether terminal modes changed.
        modes_changed: bool,
        /// Whether terminal metadata changed.
        metadata_changed: bool,
    },
    /// A non-output canonical event retained verbatim.
    CanonicalEvent {
        /// Exact typed canonical event.
        event: Event,
    },
    /// One lossless explanatory VT frame.
    VtToken {
        /// Exact token and canonical source segments.
        token: VtToken,
    },
}

/// One bounded page of a concise diagnostic timeline.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimelinePage {
    /// Entries selected by the requested offset and limit.
    pub entries: Vec<TimelineEntry>,
    /// Offset for the next page, absent when this page reaches closure.
    pub next_offset: Option<u64>,
}
