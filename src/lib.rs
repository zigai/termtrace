//! Core types and services for recording and inspecting terminal sessions.

pub mod analysis;
pub mod artifact;
pub mod bundle;
pub mod capability;
pub mod client;

pub mod control;
pub mod diff;
pub mod driver;
pub mod error;
pub mod event;
pub mod event_log;
pub mod geometry;
pub mod ghostty_vt;
pub mod inspection;
pub mod interop;
pub mod observation;
pub mod playback;
pub mod profile;
pub mod provenance;
pub mod receipt;
pub mod record;
pub mod replay;
pub mod request_journal;
pub mod save;
pub mod scenario;
pub mod schema;
pub mod semantic_image;
pub mod session;
pub mod session_registry;
mod terminal_mode;
pub mod timeline;
pub mod visual;
pub mod vt_framer;

pub use error::{Error, Result};
