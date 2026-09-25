//! Semantic and native media capture, playback, saving, and presentation.

#![deny(unsafe_code)]

pub mod capability;
mod error;
pub mod helper;
pub mod native_capture;
pub mod playback;
pub mod presentation;
pub mod save;
pub mod semantic_video;

pub use error::{Error, Result};
