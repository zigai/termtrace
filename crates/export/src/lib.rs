//! Policy-bound evidence export and bounded bug-report publication.

#![deny(unsafe_code)]

pub mod archive;
pub mod asciicast;
pub mod bug_report;
pub mod bundle;
mod error;
pub mod tmux;

pub use error::{Error, Result};
