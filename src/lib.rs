//! Core types and services for recording and inspecting terminal sessions.

mod artifact;

pub mod control;
pub mod error;
pub mod event;
pub mod event_log;
pub mod ghostty_vt;
pub mod inspection;
pub mod interop;
pub mod playback;
pub mod profile;
pub mod record;
pub mod replay;
pub mod schema;
pub mod session;
pub mod visual;

pub use error::{Error, Result};
