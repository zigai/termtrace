//! Control transport, mutation history, command execution, and screen waits.

mod commands;
mod history;
mod screen;
mod server;
mod transport;

pub(in crate::recording) use commands::{PendingInputSequence, apply_signal};
pub(in crate::recording) use history::MutationHistory;
pub(in crate::recording) use screen::ScreenWaiter;
pub use screen::{CompiledScreenPredicate, compile_screen_predicate, evaluate_screen_predicate};
pub(in crate::recording) use server::{ControlWorker, spawn_control_server};
pub use transport::control_socket_name;
