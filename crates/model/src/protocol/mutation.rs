//! What a failed request did to the session, and the one retry rule derived from it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Effect a request had on the child, its PTY, or canonical evidence.
///
/// The recording actor states this fact in every rejection. Clients only
/// synthesize it for transport outcomes the actor could not report.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MutationEffect {
    /// Nothing was applied.
    NotApplied,
    /// A known prefix of input atoms was applied completely, and nothing after it.
    Partial {
        /// Number of leading atoms applied completely.
        completed_atoms: u32,
    },
    /// Every requested mutation was applied.
    Applied,
    /// The request may or may not have been applied.
    Unknown,
}

/// What a caller may safely do after a failed request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Retry {
    /// Nothing happened; re-evaluate and submit a new request.
    NewRequest,
    /// Resend the identical request and identifier; the actor applies it at most once.
    SameRequestId,
    /// Part of the request applied; inspect current state before continuing.
    InspectFirst,
    /// The request applied; retrying would repeat it.
    DoNotRetry,
}

impl MutationEffect {
    /// Effect of an input sequence that stopped after `completed_atoms` whole atoms.
    ///
    /// Bytes written for the failed atom make its delivery indeterminate.
    #[must_use]
    pub const fn stopped_input_sequence(
        completed_atoms: u32,
        failed_atom_bytes_written: u64,
    ) -> Self {
        if failed_atom_bytes_written != 0 {
            return Self::Unknown;
        }

        if completed_atoms == 0 {
            return Self::NotApplied;
        }
        return Self::Partial { completed_atoms };
    }

    /// The only retry rule: derived from the effect alone.
    #[must_use]
    pub const fn retry(self) -> Retry {
        return match self {
            Self::NotApplied => Retry::NewRequest,
            Self::Unknown => Retry::SameRequestId,
            Self::Partial { .. } => Retry::InspectFirst,
            Self::Applied => Retry::DoNotRetry,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_is_derived_from_every_effect() {
        assert_eq!(MutationEffect::NotApplied.retry(), Retry::NewRequest);
        assert_eq!(MutationEffect::Unknown.retry(), Retry::SameRequestId);
        assert_eq!(
            MutationEffect::Partial { completed_atoms: 1 }.retry(),
            Retry::InspectFirst
        );
        assert_eq!(
            MutationEffect::Partial {
                completed_atoms: u32::MAX
            }
            .retry(),
            Retry::InspectFirst
        );
        assert_eq!(MutationEffect::Applied.retry(), Retry::DoNotRetry);
    }

    #[test]
    fn stopped_sequences_report_whole_atoms_or_unknown_delivery() {
        assert_eq!(
            MutationEffect::stopped_input_sequence(0, 0),
            MutationEffect::NotApplied
        );
        assert_eq!(
            MutationEffect::stopped_input_sequence(3, 0),
            MutationEffect::Partial { completed_atoms: 3 }
        );
        assert_eq!(
            MutationEffect::stopped_input_sequence(0, 1),
            MutationEffect::Unknown
        );
        assert_eq!(
            MutationEffect::stopped_input_sequence(3, 7),
            MutationEffect::Unknown
        );
    }

    #[test]
    fn effects_and_retries_serialize_as_stable_snake_case() {
        let encoded = serde_json::to_value([
            MutationEffect::NotApplied,
            MutationEffect::Partial { completed_atoms: 2 },
            MutationEffect::Applied,
            MutationEffect::Unknown,
        ])
        .expect("effects serialize");

        assert_eq!(
            encoded,
            serde_json::json!([
                {"kind": "not_applied"},
                {"kind": "partial", "completed_atoms": 2},
                {"kind": "applied"},
                {"kind": "unknown"},
            ])
        );

        let encoded = serde_json::to_value([
            Retry::NewRequest,
            Retry::SameRequestId,
            Retry::InspectFirst,
            Retry::DoNotRetry,
        ])
        .expect("retries serialize");

        assert_eq!(
            encoded,
            serde_json::json!([
                "new_request",
                "same_request_id",
                "inspect_first",
                "do_not_retry"
            ])
        );
    }
}
