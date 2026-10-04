//! Control transport, mutation history, command execution, and screen waits.

mod commands;
mod history;
mod screen;
mod server;
mod transport;

pub(in crate::recording) use commands::{apply_signal, PendingInputSequence};
pub(in crate::recording) use history::MutationHistory;
pub(in crate::recording) use screen::ScreenWaiter;
pub(in crate::recording) use server::{spawn, ControlWorker};
pub use transport::control_socket_name;
