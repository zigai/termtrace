//! Timed canonical-event playback inside the native Ghostty capture process.

mod runtime;
mod terminal;
pub(crate) mod worker;

pub use crate::{Error, Result};
pub use runtime::{PlaybackOptions, playback};
pub use termtrace_model::artifact::playback::{PlaybackFailure, PlaybackTiming};
pub use worker::playback_worker;
