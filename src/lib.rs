//! Core types and services for recording and inspecting terminal sessions.

pub mod error;
pub mod event;
pub mod event_log;
pub mod record;

pub use error::{Error, Result};
