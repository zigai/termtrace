//! Control transport, mutation history, command execution, and screen waits.

mod commands;
mod history;
mod screen;
mod server;

pub(in crate::record) use commands::{apply_signal, handle_control_request};
pub(in crate::record) use history::MutationHistory;
pub(in crate::record) use screen::{ScreenWaiter, evaluate_screen_waiters, fail_screen_waiters};
pub(in crate::record) use server::{ControlWorker, spawn_control_server};
