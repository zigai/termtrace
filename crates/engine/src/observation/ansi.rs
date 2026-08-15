use termtrace_evidence::event_log::CanonicalBoundary;
use termtrace_model::{
    terminal::observation::{
        CaptureQuality, Observation, ObservationIntegrity, ObservationResolution,
        ObservationSourceKind, StatePoint, TruthLayer,
    },
    terminal::state::SemanticState,
};
use termtrace_terminal::{Format, GhosttyVt};

use super::{frame_from_state, frame_hash};
use crate::Result;
use crate::replay::semantic_state_hash;

/// Render an unchained ANSI/VT stream into a truth-labeled observation.
///
/// # Errors
///
/// Returns an error when geometry or semantic-state capture is invalid.
pub fn observe_ansi_stream(bytes: &[u8], cols: u16, rows: u16) -> Result<Observation> {
    let mut terminal = GhosttyVt::new(cols, rows, 0)?;
    terminal.write(bytes);
    let state = terminal.snapshot()?;
    let state_hash = semantic_state_hash(&state)?;
    let frame = frame_from_state(&state);
    let payload_hash = frame_hash(&frame)?;
    return Ok(Observation {
        source_kind: ObservationSourceKind::AnsiStream,
        session_id: None,
        canonical_session: None,
        source_completed: false,
        state_point: StatePoint::LatestPresented,
        state_id: None,
        state_hash,
        event_seq: None,
        monotonic_ns: None,
        presented: true,
        boundary: CanonicalBoundary {
            next_event_seq: 0,
            event_hash: blake3::hash(bytes).to_hex().to_string(),
            output_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        },
        capture_quality: CaptureQuality::Incomplete,
        truth_layer: TruthLayer::Semantic,
        integrity: ObservationIntegrity::Unchained,
        resolution: ObservationResolution::AnsiInterpretation,
        frame,
        payload_hash,
    });
}

/// Interpret an ANSI/VT stream into one complete semantic state.
///
/// # Errors
///
/// Returns an error when terminal geometry or semantic capture is invalid.
pub fn semantic_state_from_ansi(bytes: &[u8], cols: u16, rows: u16) -> Result<SemanticState> {
    let mut terminal = GhosttyVt::new(cols, rows, 0)?;
    terminal.write(bytes);
    return Ok(terminal.snapshot()?);
}

/// Format an unchained ANSI stream through the pinned Ghostty formatter.
///
/// # Errors
///
/// Returns an error when geometry or formatter output is invalid.
pub fn format_ansi_stream(bytes: &[u8], cols: u16, rows: u16, format: Format) -> Result<Vec<u8>> {
    let mut terminal = GhosttyVt::new(cols, rows, 0)?;
    terminal.write(bytes);
    return Ok(terminal.format(format)?);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_observation_uses_replay_semantic_hash_contract() {
        let bytes = b"\x1b[31msemantic";
        let observation = observe_ansi_stream(bytes, 20, 4).expect("ANSI observation");
        let state = semantic_state_from_ansi(bytes, 20, 4).expect("ANSI semantic state");

        assert_eq!(
            observation.state_hash,
            semantic_state_hash(&state).expect("replay semantic hash")
        );
    }
}
