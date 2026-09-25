//! Aggregate tmux recording.
//!
//! A tmux recording is one composite outer-client track plus one canonical track per
//! pane process, supervised for the life of a tmux server. This crate owns the whole
//! domain: typed lifecycle events ([`artifact::TmuxEvent`]), the lifecycle state machine
//! ([`lifecycle`]), a pure reducer ([`reducer::reduce`]), the recording's storage
//! ([`store`], [`journal`], [`catalog`]), tmux command construction and parsing
//! ([`client`]), scope selection ([`scope`]), integration setup ([`setup`]), coverage
//! classification ([`status`]), the observer processes ([`observer`]), pane launching
//! ([`pane`]), and the aggregate operations ([`recording`]).
//!
//! Canonical session effects (start, stop, await completion) are supplied by the caller
//! through [`recording::Tracks`].

pub mod artifact;
pub mod catalog;
pub mod client;
mod error;
pub mod journal;
pub mod lifecycle;
pub mod observer;
pub mod options;
pub mod pane;
pub mod recording;
pub mod reducer;
pub mod scope;
pub mod setup;
pub mod status;
pub mod store;

pub use error::{Error, Result};
