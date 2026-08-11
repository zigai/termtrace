//! One-shot PTY recording with byte-exact event persistence.

mod control;
mod input;
mod model;
pub(crate) mod presentation;
mod process;
mod runtime;
mod support;

pub use model::{RecordOptions, RecordOutcome};
pub use runtime::{record, record_with_output};
