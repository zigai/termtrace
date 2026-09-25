//! The aggregate lifecycle state machine.
//!
//! ```text
//! Starting --CompositeLinked--> Live --StopRequested--> Stopping --TracksSettled--> Completed
//!     \--------------------StopRequested-------------------^
//! ```
//!
//! `Stopping` also accepts a late `CompositeLinked` and a repeated `StopRequested` without
//! changing. `Completed` is terminal. A composite start failure is not a transition: the
//! recording is released and its name freed.

use crate::{
    artifact::TmuxRecordingLifecycle,
    error::{Error, Result},
};

/// Cause of a lifecycle change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleTrigger {
    /// The composite client track started.
    CompositeLinked,
    /// A stop was requested.
    StopRequested,
    /// The composite and every pane track published completion after a stop.
    TracksSettled,
}

impl TmuxRecordingLifecycle {
    /// Return the lifecycle after `trigger`, or reject an illegal transition.
    ///
    /// # Errors
    /// Returns [`Error::IllegalTransition`] for every transition not drawn in the module
    /// diagram.
    pub fn advance(self, trigger: LifecycleTrigger) -> Result<Self> {
        return match (self, trigger) {
            (Self::Starting, LifecycleTrigger::CompositeLinked) => Ok(Self::Live),
            (
                Self::Stopping,
                LifecycleTrigger::CompositeLinked | LifecycleTrigger::StopRequested,
            )
            | (Self::Starting | Self::Live, LifecycleTrigger::StopRequested) => Ok(Self::Stopping),
            (Self::Stopping, LifecycleTrigger::TracksSettled) => Ok(Self::Completed),
            (from, trigger) => Err(Error::IllegalTransition { from, trigger }),
        };
    }

    /// Whether history may still change; `Completed` recordings are sealed.
    pub fn is_open(self) -> bool {
        return self != Self::Completed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions_follow_the_diagram() {
        use LifecycleTrigger::{CompositeLinked, StopRequested, TracksSettled};
        use TmuxRecordingLifecycle::{Completed, Live, Starting, Stopping};

        let legal = [
            (Starting, CompositeLinked, Live),
            (Starting, StopRequested, Stopping),
            (Live, StopRequested, Stopping),
            (Stopping, StopRequested, Stopping),
            (Stopping, CompositeLinked, Stopping),
            (Stopping, TracksSettled, Completed),
        ];
        for (from, trigger, to) in legal {
            assert_eq!(from.advance(trigger).ok(), Some(to), "{from:?} {trigger:?}");
        }

        for (from, trigger) in [
            (Starting, TracksSettled),
            (Live, CompositeLinked),
            (Live, TracksSettled),
            (Completed, CompositeLinked),
            (Completed, StopRequested),
            (Completed, TracksSettled),
        ] {
            assert!(
                matches!(from.advance(trigger), Err(Error::IllegalTransition { .. })),
                "{from:?} {trigger:?} must be illegal"
            );
        }
    }
}
