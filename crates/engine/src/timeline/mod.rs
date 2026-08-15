//! Timeline construction, source selection, and time parsing.

mod build;
mod selection;
mod time;
pub use termtrace_model::terminal::replay::SourceRange;

pub use build::{build_timeline, build_timeline_page};
pub use selection::{resolve_source_range, resolve_source_selection};
pub use time::parse_time_offset_ns;
