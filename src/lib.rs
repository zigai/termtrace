//! Core types and services for recording and inspecting terminal sessions.

pub mod control;
pub mod error;
pub mod event;
pub mod event_log;
pub mod ghostty_vt;
pub mod record;
pub mod replay;

pub use error::{Error, Result};
