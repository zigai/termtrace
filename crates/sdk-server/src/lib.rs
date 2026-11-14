//! JSON-RPC server that exposes the Termtrace SDK to clients in other languages.
//!
//! Requests and responses are JSON-RPC 2.0 objects, one per line. The first request must be
//! `initialize`. Each later request runs on its own thread, so a slow wait never delays other
//! calls, and responses are written in completion order. Values use the plain serde form of the
//! SDK model types; [`catalog`] describes every method and type for client generators.

pub mod catalog;
mod failure;
mod methods;
mod protocol;
mod server;

use std::io::{self, BufRead, Write};

/// Serve requests from `input` until it ends, writing responses to `output`.
///
/// Recordings the client still holds are released when `input` ends, which leaves their
/// recorders running exactly as dropping the Rust values would.
///
/// # Errors
///
/// Fails when `input` cannot be read or `output` cannot be written.
pub fn serve(input: impl BufRead, output: impl Write + Send + 'static) -> io::Result<()> {
    return server::Server::run(input, output);
}
