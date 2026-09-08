//! Linux abstract-socket framing and actor response deadlines.

use std::{
    os::{linux::net::SocketAddrExt as _, unix::net::SocketAddr},
    time::Duration,
};

use termtrace_model::protocol::control::{
    ActionPostcondition, ControlCommand, InputSequenceAtom, MAX_CONTROL_MESSAGE_BYTES,
};
use uuid::Uuid;

use crate::{Error, Result};

const CONTROL_SOCKET_PREFIX: &str = "termtrace-control-";
const DEFAULT_CONTROL_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_INPUT_SEQUENCE_PACING: Duration = Duration::from_secs(60);

/// Derive the Linux abstract control-socket name for a recording session.
pub fn control_socket_name(session_id: Uuid) -> String {
    return format!("{CONTROL_SOCKET_PREFIX}{session_id}");
}

pub(super) fn control_socket_address(name: &str) -> Result<SocketAddr> {
    return SocketAddr::from_abstract_name(name.as_bytes()).map_err(|error| {
        return Error::InvalidControlProtocol {
            reason: format!("invalid abstract control socket name: {error}"),
        };
    });
}

pub(super) fn control_response_timeout(command: &ControlCommand, wait_slack: Duration) -> Duration {
    return match command {
        ControlCommand::WaitScreen { timeout_ns, .. }
        | ControlCommand::CaptureStable { timeout_ns, .. } => {
            Duration::from_nanos(*timeout_ns).saturating_add(wait_slack)
        }
        ControlCommand::InputSequence { atoms, pace_ns, .. } => {
            input_sequence_response_timeout(atoms, *pace_ns)
        }
        ControlCommand::Act {
            atoms,
            pace_ns,
            postcondition,
            ..
        } => {
            let postcondition_timeout = match postcondition {
                ActionPostcondition::None => Duration::ZERO,
                ActionPostcondition::Idle { timeout_ns, .. }
                | ActionPostcondition::Screen { timeout_ns, .. } => {
                    Duration::from_nanos(*timeout_ns)
                }
            };

            input_sequence_response_timeout(atoms, *pace_ns)
                .saturating_add(postcondition_timeout)
                .saturating_add(wait_slack)
        }
        _ => DEFAULT_CONTROL_TIMEOUT,
    };
}

pub(super) fn validate_frame(encoded: &[u8]) -> Result<()> {
    if encoded.is_empty()
        || encoded.last() != Some(&b'\n')
        || u64::try_from(encoded.len()).unwrap_or(u64::MAX) > MAX_CONTROL_MESSAGE_BYTES
    {
        return Err(Error::InvalidControlProtocol {
            reason: "message must be one newline-terminated JSON object no larger than 1 MiB"
                .to_owned(),
        });
    }

    return Ok(());
}

fn input_sequence_response_timeout(atoms: &[InputSequenceAtom], pace_ns: u64) -> Duration {
    let gaps = u64::try_from(atoms.len().saturating_sub(1)).unwrap_or(u64::MAX);
    let pacing = Duration::from_nanos(pace_ns.saturating_mul(gaps)).min(MAX_INPUT_SEQUENCE_PACING);

    return DEFAULT_CONTROL_TIMEOUT.saturating_add(pacing);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use termtrace_model::{
        execution::event::InputKind,
        protocol::control::{ControlCommand, InputSequenceAtom},
        value::ByteString,
    };

    #[test]
    fn rejects_unterminated_and_oversized_frames() {
        assert!(validate_frame(br#"{"version":1}"#).is_err());
        let mut oversized = vec![b'x'; usize::try_from(MAX_CONTROL_MESSAGE_BYTES).unwrap()];
        oversized.push(b'\n');
        assert!(validate_frame(&oversized).is_err());
    }

    #[test]
    fn paced_sequences_extend_both_sides_of_the_control_deadline() {
        let atoms = vec![
            InputSequenceAtom::Bytes {
                operation_id: Uuid::now_v7(),
                data: ByteString::new(vec![b'a']),
                input_kind: InputKind::Text,
                caller_intent: None,
            },
            InputSequenceAtom::Bytes {
                operation_id: Uuid::now_v7(),
                data: ByteString::new(vec![b'b']),
                input_kind: InputKind::Text,
                caller_intent: None,
            },
        ];

        let command = ControlCommand::InputSequence {
            atoms,
            pace_ns: 11_000_000_000,
            include_atom_outcomes: false,
        };

        assert_eq!(
            control_response_timeout(&command, Duration::from_secs(2)),
            Duration::from_secs(21)
        );
    }
}
