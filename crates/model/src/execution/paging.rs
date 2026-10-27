//! Source-bound, snapshot-relative SDK paging contracts. These are not persisted log formats.

use crate::{
    execution::{event::EventProjection, session::SessionCompletion},
    protocol::control::LiveEventCursor,
    value::ByteString,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Identity of the canonical file opened for a page. Display paths are not authoritative.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageSource {
    /// Exact absolute canonical session directory bytes.
    pub session_path: ByteString,
    /// Non-authoritative display only; never used to select or authenticate a source.
    pub session_path_display: String,
    /// Immutable canonical session identity.
    pub session_id: Uuid,
    /// Device of the canonical root descriptor.
    pub device: u64,
    /// Inode of the canonical root descriptor.
    pub inode: u64,
    /// Hash of the exact canonical header bytes.
    pub header_hash: String,
}

/// A checked chain endpoint, not permission to skip validation of an imported cursor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EventPageCursor {
    /// File and session identity.
    pub source: PageSource,
    /// Next scanned event, including events excluded by the filter.
    pub next_event_seq: u64,
    /// Hash immediately before `next_event_seq`, or header hash at zero.
    pub event_hash: String,
}

/// Bounded projected canonical events. `has_more` concerns this finite snapshot only.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectedEventPage {
    /// Ordered matching projections with hashes, timestamps, and optional byte summaries.
    pub events: Vec<EventProjection>,
    /// Next scanned position. Filters may advance an empty page.
    pub cursor: EventPageCursor,
    /// A matching complete record remains in the snapshot.
    pub has_more: bool,
    /// An incomplete trailing record was excluded from this live snapshot.
    pub incomplete_tail: bool,
}

/// A byte position anchored to the chain endpoint enclosing that output prefix.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputPageCursor {
    /// Chain endpoint through the event containing the last returned byte.
    pub events: EventPageCursor,
    /// Exact offset in the concatenated PTY output stream.
    pub byte_offset: u64,
}

/// Bounded exact PTY bytes; this never promises no future output will arrive.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ByteOutputPage {
    /// Exact PTY bytes, including NUL and invalid UTF-8.
    pub bytes: ByteString,
    /// Checked resume position.
    pub cursor: OutputPageCursor,
    /// At least one further output byte is present in this snapshot.
    pub has_more: bool,
    /// An incomplete trailing record was excluded from this live snapshot.
    pub incomplete_tail: bool,
}

/// Actor-owned durable follow position or independently validated completion.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventFollowPosition {
    /// Position authenticated by the live actor, not imported file metadata.
    Live {
        /// Writer descriptor device.
        device: u64,
        /// Writer descriptor inode.
        inode: u64,
        /// Exact canonical header hash.
        header_hash: String,
        /// Requested position, when retained and durable.
        start: Option<LiveEventCursor>,
        /// Latest synchronized position.
        durable: LiveEventCursor,
    },
    /// Validated terminal completion, including its source-bound final event hash.
    Completed {
        /// Full completion; this is not a live-prefix claim.
        completion: Box<SessionCompletion>,
    },
}
