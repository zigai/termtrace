//! Timeline construction, source selection, and time parsing.

mod build;
mod selection;
pub(crate) mod stream;
mod time;
pub use termtrace_model::terminal::replay::SourceRange;

pub use build::{build_timeline, build_timeline_page};
pub use selection::{
    resolve_session_prefix_source_selection, resolve_session_source_selection,
    resolve_source_range, resolve_source_selection,
};
pub use stream::{
    build_session_timeline_page, visit_session_timeline_while, visit_snapshot_timeline_while,
};
pub use time::parse_time_offset_ns;
