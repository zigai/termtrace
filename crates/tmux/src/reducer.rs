//! The pure aggregate reducer: `(state, input, now) -> (state, events)`.
//!
//! Every aggregate state change goes through [`reduce`]. It reads no clock, tmux server,
//! or filesystem; callers observe the world into an [`Input`] and commit the resulting
//! [`Step`] through [`crate::store`].

use crate::{
    artifact::{
        TmuxCoverageGap, TmuxEvent, TmuxGapKind, TmuxPaneSnapshot, TmuxPaneTrack,
        TmuxRecordingLifecycle, TmuxRecordingState, TmuxServerIdentity, TmuxSessionLink,
    },
    error::{Error, Result},
    lifecycle::LifecycleTrigger,
};
use std::collections::{BTreeMap, BTreeSet};
use termtrace_model::value::ByteString;
use uuid::Uuid;

/// One supervisor or status snapshot of a tmux server.
#[derive(Clone, Debug, Default)]
pub struct Observation {
    /// Identity of the server that answered.
    pub identity: Option<TmuxServerIdentity>,
    /// Every pane, in tmux order.
    pub panes: Vec<TmuxPaneSnapshot>,
    /// Pane IDs that are selected for recording but not run by a pane wrapper.
    pub uncovered: BTreeSet<String>,
    /// Resolved identities for the published recordings named by [`unlinked_recordings`].
    pub links: BTreeMap<String, TmuxPaneTrack>,
}

/// One fact the aggregate reacts to.
#[derive(Clone, Debug)]
pub enum Input {
    /// A server snapshot.
    Observe(Observation),
    /// A supervisor process started observing.
    ObserverStarted {
        /// Supervisor process ID.
        pid: u32,
    },
    /// The composite client track started.
    CompositeLinked(TmuxSessionLink),
    /// A stop was requested.
    StopRequested,
    /// A pane wrapper published its recording.
    TrackLinked(TmuxPaneTrack),
    /// Completion evidence observed after a stop.
    TracksSettled {
        /// Pane recordings newly observed complete, as `(pane ID, session ID)`.
        completed: Vec<(String, Uuid)>,
        /// Linked pane recordings still unfinished after `completed`.
        remaining: u64,
        /// Whether the composite client track published completion.
        composite_completed: bool,
    },
}

/// Result of one reduction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Step {
    /// State after the input.
    pub state: TmuxRecordingState,
    /// Events to append, in order.
    pub events: Vec<TmuxEvent>,
}

/// How an observed server relates to the bound identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityCheck {
    /// No identity is bound yet.
    Unbound,
    /// The observed server is the bound server.
    Matches,
}

/// Compare an observed server with the bound identity.
///
/// # Errors
/// Returns [`Error::ServerIdentityChanged`] when a different server answers.
pub fn check_identity(
    name: &str,
    bound: Option<&TmuxServerIdentity>,
    observed: &TmuxServerIdentity,
) -> Result<IdentityCheck> {
    return match bound {
        None => Ok(IdentityCheck::Unbound),
        Some(bound) if bound == observed => Ok(IdentityCheck::Matches),
        Some(bound) => Err(Error::ServerIdentityChanged {
            name: name.to_owned(),
            detail: format!(
                "expected pid {}, socket {:?} ({}:{}); observed pid {}, socket {:?} ({}:{})",
                bound.pid,
                String::from_utf8_lossy(bound.socket_path.as_bytes()),
                bound.socket_device,
                bound.socket_inode,
                observed.pid,
                String::from_utf8_lossy(observed.socket_path.as_bytes()),
                observed.socket_device,
                observed.socket_inode,
            ),
        }),
    };
}

/// Published pane recordings not yet linked, as `(pane ID, session path)`.
///
/// Callers resolve these to [`TmuxPaneTrack`] identities for [`Observation::links`].
pub fn unlinked_recordings(
    state: &TmuxRecordingState,
    panes: &[TmuxPaneSnapshot],
) -> Vec<(String, ByteString)> {
    return panes
        .iter()
        .filter_map(|pane| {
            let session = pane.recording_session.as_ref()?;

            return (state.pane_tracks.get(&pane.pane_id) != Some(session))
                .then(|| return (pane.pane_id.clone(), session.clone()));
        })
        .collect();
}

/// Coverage limitations that hold for the whole life of every aggregate.
pub fn permanent_gaps(now: u64) -> Vec<TmuxCoverageGap> {
    return [
        TmuxGapKind::PollingInterval,
        TmuxGapKind::ClientOverlayIndex,
    ]
    .into_iter()
    .map(|kind| return TmuxCoverageGap::open(None, kind, now))
    .collect();
}

/// Apply one input to `state` at wall-clock time `now`.
///
/// Completed recordings are sealed: observations and settlements produce nothing, and
/// links are rejected.
///
/// # Errors
/// Returns illegal lifecycle transitions, a changed server identity, a rebound composite
/// link, or links into a sealed recording. `state` is unchanged on error.
pub fn reduce(name: &str, state: &TmuxRecordingState, input: Input, now: u64) -> Result<Step> {
    let mut next = state.clone();
    let mut events = Vec::new();
    match input {
        Input::Observe(observation) => {
            if next.lifecycle.is_open() {
                observe(name, &mut next, &mut events, observation, now)?;
            }
        }
        Input::ObserverStarted { pid } => {
            if next.lifecycle.is_open() {
                let mut gap = TmuxCoverageGap::open(None, TmuxGapKind::ObserverStart, now);
                gap.first_observed_unix_ns = next.updated_unix_ns;
                events.push(TmuxEvent::CoverageGapOpened { gap: gap.clone() });
                gap.resolved_unix_ns = Some(now);
                events.push(TmuxEvent::CoverageGapResolved { gap });
                events.push(TmuxEvent::ObserverStarted { pid });
                next.updated_unix_ns = now;
            }
        }
        Input::CompositeLinked(link) => {
            next.lifecycle = next.lifecycle.advance(LifecycleTrigger::CompositeLinked)?;

            match &next.composite_track.link {
                Some(existing) if *existing == link => {}
                Some(_) => {
                    return Err(Error::invalid(
                        "aggregate composite track is already linked to another session",
                    ));
                }
                None => {
                    next.composite_track.link = Some(link.clone());
                    events.push(TmuxEvent::CompositeTrackLinked { link });
                }
            }

            next.updated_unix_ns = now;
        }
        Input::StopRequested => {
            if next.lifecycle.is_open() {
                let from = next.lifecycle;
                next.lifecycle = from.advance(LifecycleTrigger::StopRequested)?;

                if next.lifecycle != from {
                    events.push(TmuxEvent::RecordingStopping);
                    next.updated_unix_ns = now;
                }
            }
        }
        Input::TrackLinked(track) => {
            if !next.lifecycle.is_open() {
                return Err(Error::Sealed(name.to_owned()));
            }

            link_track(&mut next, &mut events, track);
            next.updated_unix_ns = now;
        }
        Input::TracksSettled {
            completed,
            remaining,
            composite_completed,
        } => {
            if next.lifecycle == TmuxRecordingLifecycle::Stopping {
                events.extend(completed.into_iter().map(|(pane_id, session_id)| {
                    return TmuxEvent::PaneTrackCompleted {
                        pane_id,
                        session_id,
                    };
                }));

                if composite_completed && remaining == 0 {
                    next.lifecycle = next.lifecycle.advance(LifecycleTrigger::TracksSettled)?;
                    events.push(TmuxEvent::RecordingCompleted);
                }

                next.updated_unix_ns = now;
            }
        }
    }

    return Ok(Step {
        state: next,
        events,
    });
}

fn observe(
    name: &str,
    state: &mut TmuxRecordingState,
    events: &mut Vec<TmuxEvent>,
    observation: Observation,
    now: u64,
) -> Result<()> {
    let Observation {
        identity,
        panes,
        uncovered,
        mut links,
    } = observation;

    if let Some(identity) = identity
        && check_identity(name, state.server.as_ref(), &identity)? == IdentityCheck::Unbound
    {
        state.server = Some(identity.clone());
        events.push(TmuxEvent::ServerIdentityBound { identity });
    }

    let previous = state
        .panes
        .iter()
        .map(|pane| return (pane.pane_id.as_str(), pane))
        .collect::<BTreeMap<_, _>>();

    let present = panes
        .iter()
        .map(|pane| return pane.pane_id.as_str())
        .collect::<BTreeSet<_>>();

    let mut linked = Vec::new();

    for pane in &panes {
        match previous.get(pane.pane_id.as_str()) {
            Some(before) => pane_transitions(events, before, pane),
            None => events.push(TmuxEvent::PaneCreated {
                pane: pane.address(),
            }),
        }

        if let Some(track) = links.remove(&pane.pane_id)
            && pane.recording_session.as_ref() == Some(&track.session)
        {
            linked.push(track);
        }
    }

    for pane in previous.values() {
        if !present.contains(pane.pane_id.as_str()) {
            events.push(TmuxEvent::PaneDestroyed {
                pane: pane.address(),
            });
        }
    }

    container_transitions(events, &state.panes, &panes);

    for track in linked {
        link_track(state, events, track);
    }

    let mut current = permanent_gaps(now);
    current.extend(
        uncovered
            .into_iter()
            .filter(|pane| return present.contains(pane.as_str()))
            .map(|pane| return TmuxCoverageGap::open(Some(pane), TmuxGapKind::UncoveredPane, now)),
    );
    merge_gaps(state, events, &current, now);

    state
        .pane_tracks
        .retain(|pane, _| return present.contains(pane.as_str()));

    state.panes = panes;
    state.updated_unix_ns = now;
    return Ok(());
}

fn link_track(state: &mut TmuxRecordingState, events: &mut Vec<TmuxEvent>, track: TmuxPaneTrack) {
    if state.pane_tracks.get(&track.pane_id) == Some(&track.session) {
        return;
    }

    state
        .pane_tracks
        .insert(track.pane_id.clone(), track.session.clone());

    events.push(TmuxEvent::PaneTrackLinked { track });
}

fn pane_transitions(
    events: &mut Vec<TmuxEvent>,
    previous: &TmuxPaneSnapshot,
    pane: &TmuxPaneSnapshot,
) {
    let address = pane.address();

    if previous.window_id != pane.window_id {
        events.push(TmuxEvent::PaneMoved {
            pane: address.clone(),
        });
    }

    if previous.layout != pane.layout {
        events.push(TmuxEvent::LayoutChanged {
            pane: address.clone(),
            layout: pane.layout.clone(),
        });
    }

    if previous.window_active != pane.window_active {
        events.push(TmuxEvent::ActiveWindowChanged {
            pane: address.clone(),
            active: pane.window_active,
        });
    }

    if previous.pane_active != pane.pane_active {
        events.push(TmuxEvent::ActivePaneChanged {
            pane: address.clone(),
            active: pane.pane_active,
        });
    }

    if previous.zoomed != pane.zoomed {
        events.push(TmuxEvent::ZoomChanged {
            pane: address.clone(),
            zoomed: pane.zoomed,
        });
    }

    if previous.copy_mode != pane.copy_mode || previous.pane_mode != pane.pane_mode {
        events.push(TmuxEvent::PaneModeChanged {
            pane: address,
            mode: pane.copy_mode.then(|| return pane.pane_mode.clone()),
        });
    }
}

fn container_transitions(
    events: &mut Vec<TmuxEvent>,
    previous: &[TmuxPaneSnapshot],
    current: &[TmuxPaneSnapshot],
) {
    let sessions = |panes: &[TmuxPaneSnapshot]| {
        return panes
            .iter()
            .map(|pane| return pane.session_id.clone())
            .collect::<BTreeSet<_>>();
    };

    let windows = |panes: &[TmuxPaneSnapshot]| {
        return panes
            .iter()
            .map(|pane| return pane.window_id.clone())
            .collect::<BTreeSet<_>>();
    };

    let (old_sessions, new_sessions) = (sessions(previous), sessions(current));
    let (old_windows, new_windows) = (windows(previous), windows(current));

    for session_id in new_sessions.difference(&old_sessions) {
        events.push(TmuxEvent::SessionCreated {
            session_id: session_id.clone(),
        });
    }

    for session_id in old_sessions.difference(&new_sessions) {
        events.push(TmuxEvent::SessionDestroyed {
            session_id: session_id.clone(),
        });
    }

    for window_id in new_windows.difference(&old_windows) {
        events.push(TmuxEvent::WindowCreated {
            window_id: window_id.clone(),
        });
    }

    for window_id in old_windows.difference(&new_windows) {
        events.push(TmuxEvent::WindowDestroyed {
            window_id: window_id.clone(),
        });
    }
}

fn merge_gaps(
    state: &mut TmuxRecordingState,
    events: &mut Vec<TmuxEvent>,
    current: &[TmuxCoverageGap],
    now: u64,
) {
    for gap in current {
        if let Some(existing) = state
            .coverage_gaps
            .iter_mut()
            .find(|existing| return existing.same_subject(gap))
        {
            existing.last_observed_unix_ns = now;
            continue;
        }

        events.push(TmuxEvent::CoverageGapOpened { gap: gap.clone() });
        state.coverage_gaps.push(gap.clone());
    }

    let (open, resolved): (Vec<_>, Vec<_>) = state.coverage_gaps.drain(..).partition(|gap| {
        return current
            .iter()
            .any(|candidate| return gap.same_subject(candidate));
    });
    state.coverage_gaps = open;

    for mut gap in resolved {
        gap.resolved_unix_ns = Some(now);
        events.push(TmuxEvent::CoverageGapResolved { gap });
    }
}

#[cfg(test)]
mod tests;
