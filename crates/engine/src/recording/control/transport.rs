//! Linux abstract-socket framing for the recording actor.

use crate::{Error, Result};
use std::os::{linux::net::SocketAddrExt as _, unix::net::SocketAddr};
pub use termtrace_model::protocol::control::control_socket_name;
use termtrace_model::protocol::control::is_control_frame;

pub(super) fn control_socket_address(name: &str) -> Result<SocketAddr> {
    return SocketAddr::from_abstract_name(name.as_bytes()).map_err(|error| {
        return Error::InvalidControlProtocol {
            reason: format!("invalid abstract control socket name: {error}"),
        };
    });
}

pub(super) fn validate_frame(encoded: &[u8]) -> Result<()> {
    if !is_control_frame(encoded) {
        return Err(Error::InvalidControlProtocol {
            reason: "message must be one newline-terminated JSON object no larger than 16 MiB"
                .to_owned(),
        });
    }
    return Ok(());
}
