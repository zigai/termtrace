//! Reaching a live Termtrace session: control transport and session-phase observation.
//!
//! This crate is the only place that decides whether a session is live, completed, or
//! neither. It links no recorder, replay engine, or renderer.

mod error;
mod phase;
mod transport;

pub use error::{Error, Result};
pub use phase::{
    is_completed, observe_phase, read_completed, recorder_identity, request_or_completed,
    send_or_completed, PhaseObservation, Reached, RecorderIdentity,
};
pub use transport::{
    control_endpoint_is_reachable, exchange, request, send_request, send_request_response,
    wait_for_screen, ExchangeTiming,
};
