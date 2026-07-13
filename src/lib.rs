//! Core types and services for recording and inspecting terminal sessions.

mod artifact;

pub mod control;
pub mod error;
pub mod event;
pub mod event_log;
pub mod ghostty_vt;
pub mod playback;
pub mod record;
pub mod replay;
pub mod visual;

pub use error::{Error, Result};
