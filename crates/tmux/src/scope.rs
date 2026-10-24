//! Which pane processes an integration records.

use crate::{
    client::{valid_identifier, PaneRow, TmuxClient},
    error::{Error, Result},
    options,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    ffi::{OsStr, OsString},
    os::unix::ffi::OsStrExt as _,
    str,
};
use termtrace_model::value::ByteString;

const SCOPE_VERSION: u32 = 1;

/// Recording scope mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TmuxScopeMode {
    /// Record every future pane process.
    All,
    /// Record only processes launched explicitly through `tmux run`.
    Explicit,
    /// Record processes in the listed sessions, windows, or panes.
    Selected,
}

/// Versioned recording scope stored in a global tmux option.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TmuxScopePolicy {
    /// Policy shape version.
    pub version: u32,
    /// Scope mode.
    pub mode: TmuxScopeMode,
    /// Selected stable session IDs.
    pub session_ids: BTreeSet<String>,
    /// Selected session names.
    pub session_names: Vec<ByteString>,
    /// Selected stable window IDs.
    pub window_ids: BTreeSet<String>,
    /// Selected window names.
    pub window_names: Vec<ByteString>,
    /// Selected stable pane IDs.
    pub pane_ids: BTreeSet<String>,
}

/// Requested scope targets, before resolution against a live server.
#[derive(Clone, Debug, Default)]
pub struct ScopeTargets {
    /// Session targets.
    pub sessions: Vec<OsString>,
    /// Window targets.
    pub windows: Vec<OsString>,
    /// Pane targets.
    pub panes: Vec<OsString>,
}

impl ScopeTargets {
    fn is_empty(&self) -> bool {
        return self.sessions.is_empty() && self.windows.is_empty() && self.panes.is_empty();
    }
}

impl TmuxScopePolicy {
    /// Record every future pane process.
    pub fn all() -> Self {
        return Self {
            version: SCOPE_VERSION,
            mode: TmuxScopeMode::All,
            session_ids: BTreeSet::new(),
            session_names: Vec::new(),
            window_ids: BTreeSet::new(),
            window_names: Vec::new(),
            pane_ids: BTreeSet::new(),
        };
    }

    /// Record only explicitly launched processes.
    pub fn explicit() -> Self {
        return Self {
            mode: TmuxScopeMode::Explicit,
            ..Self::all()
        };
    }

    fn selected() -> Self {
        return Self {
            mode: TmuxScopeMode::Selected,
            ..Self::all()
        };
    }

    /// Stable mode spelling.
    pub fn mode_name(&self) -> &'static str {
        return match self.mode {
            TmuxScopeMode::All => "all",
            TmuxScopeMode::Explicit => "explicit",
            TmuxScopeMode::Selected => "selected",
        };
    }

    /// Scope configured before the server exists: names and stable pane IDs.
    ///
    /// # Errors
    /// Returns empty names or pane selectors that are not stable IDs.
    pub fn initial(explicit_only: bool, targets: &ScopeTargets) -> Result<Self> {
        if explicit_only {
            return Ok(Self::explicit());
        }

        if targets.is_empty() {
            return Ok(Self::all());
        }

        let mut policy = Self::selected();
        for (names, kind, into) in [
            (&targets.sessions, "session", &mut policy.session_names),
            (&targets.windows, "window", &mut policy.window_names),
        ] {
            for name in names {
                if name.as_bytes().is_empty() {
                    return Err(Error::invalid(format!(
                        "tmux {kind} names must be nonempty"
                    )));
                }

                into.push(ByteString::from(name.as_bytes()));
            }
        }

        for pane in &targets.panes {
            let pane = str::from_utf8(pane.as_bytes()).map_err(|error| {
                return Error::invalid(format!("tmux pane IDs must be UTF-8: {error}"));
            })?;
            if !valid_identifier(pane, b'%') {
                return Err(Error::invalid(format!(
                    "tmux pane selector `{pane}` is not a stable pane ID such as %3"
                )));
            }

            policy.pane_ids.insert(pane.to_owned());
        }

        policy.validate()?;

        return Ok(policy);
    }

    /// Scope requested against a running server; targets resolve to stable IDs.
    ///
    /// Returns none when nothing was requested.
    ///
    /// # Errors
    /// Returns unavailable targets.
    pub fn requested(
        client: &TmuxClient,
        all: bool,
        explicit_only: bool,
        targets: &ScopeTargets,
    ) -> Result<Option<Self>> {
        if all {
            return Ok(Some(Self::all()));
        }

        if explicit_only {
            return Ok(Some(Self::explicit()));
        }

        if targets.is_empty() {
            return Ok(None);
        }

        let mut policy = Self::selected();
        for target in &targets.sessions {
            policy.session_ids.insert(client.resolve_target(
                target,
                "#{session_id}",
                b'$',
                "session",
            )?);
        }

        for target in &targets.windows {
            policy.window_ids.insert(client.resolve_target(
                target,
                "#{window_id}",
                b'@',
                "window",
            )?);
        }

        for target in &targets.panes {
            policy
                .pane_ids
                .insert(client.resolve_target(target, "#{pane_id}", b'%', "pane")?);
        }

        policy.validate()?;

        return Ok(Some(policy));
    }

    /// Resolve every name and ID against a running server.
    ///
    /// # Errors
    /// Returns unavailable targets.
    pub fn resolve_live(&self, client: &TmuxClient) -> Result<Self> {
        if self.mode != TmuxScopeMode::Selected {
            return Ok(self.clone());
        }

        let mut resolved = Self::selected();
        for target in self
            .session_ids
            .iter()
            .map(|id| return OsStr::new(id.as_str()))
            .chain(
                self.session_names
                    .iter()
                    .map(|name| return OsStr::from_bytes(name.as_bytes())),
            )
        {
            resolved.session_ids.insert(client.resolve_target(
                target,
                "#{session_id}",
                b'$',
                "session",
            )?);
        }

        for target in self
            .window_ids
            .iter()
            .map(|id| return OsStr::new(id.as_str()))
            .chain(
                self.window_names
                    .iter()
                    .map(|name| return OsStr::from_bytes(name.as_bytes())),
            )
        {
            resolved.window_ids.insert(client.resolve_target(
                target,
                "#{window_id}",
                b'@',
                "window",
            )?);
        }

        for pane in &self.pane_ids {
            resolved.pane_ids.insert(client.resolve_target(
                OsStr::new(pane),
                "#{pane_id}",
                b'%',
                "pane",
            )?);
        }

        resolved.validate()?;

        return Ok(resolved);
    }

    /// Reject malformed policies.
    ///
    /// # Errors
    /// Returns the violated rule.
    pub fn validate(&self) -> Result<()> {
        if self.version != SCOPE_VERSION {
            return Err(Error::invalid(format!(
                "unsupported tmux recording scope version {}",
                self.version
            )));
        }

        if self
            .session_ids
            .iter()
            .any(|identifier| return !valid_identifier(identifier, b'$'))
            || self
                .window_ids
                .iter()
                .any(|identifier| return !valid_identifier(identifier, b'@'))
            || self
                .pane_ids
                .iter()
                .any(|identifier| return !valid_identifier(identifier, b'%'))
        {
            return Err(Error::invalid(
                "configured tmux recording scope contains an invalid stable object ID",
            ));
        }

        if self
            .session_names
            .iter()
            .chain(self.window_names.iter())
            .any(|name| return name.as_bytes().is_empty())
        {
            return Err(Error::invalid(
                "configured tmux recording scope contains an empty name",
            ));
        }

        let has_targets = !self.session_ids.is_empty()
            || !self.session_names.is_empty()
            || !self.window_ids.is_empty()
            || !self.window_names.is_empty()
            || !self.pane_ids.is_empty();

        return match (self.mode, has_targets) {
            (TmuxScopeMode::Selected, false) => Err(Error::invalid(
                "selected tmux recording scope has no targets",
            )),
            (TmuxScopeMode::All | TmuxScopeMode::Explicit, true) => Err(Error::invalid(format!(
                "tmux recording scope mode {} cannot contain targets",
                self.mode_name()
            ))),
            _ => Ok(()),
        };
    }

    /// Validated option encoding.
    ///
    /// # Errors
    /// Returns malformed policies.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        self.validate()?;

        return serde_json::to_vec(self).map_err(|error| return Error::json(options::SCOPE, error));
    }

    /// Read the configured policy.
    ///
    /// # Errors
    /// Returns an empty or malformed option.
    pub fn from_options(client: &TmuxClient) -> Result<Self> {
        let bytes = client.option(options::SCOPE)?;
        if bytes.is_empty() {
            return Err(Error::invalid("configured tmux recording scope is empty"));
        }

        let policy = serde_json::from_slice::<Self>(&bytes).map_err(|error| {
            return Error::invalid(format!(
                "configured tmux recording scope is invalid: {error}"
            ));
        })?;

        policy.validate()?;

        return Ok(policy);
    }

    /// Store this policy in the server's options and wake the supervisor.
    ///
    /// # Errors
    /// Returns malformed policies or tmux failures.
    pub fn store(&self, client: &TmuxClient) -> Result<()> {
        client.set_option(options::SCOPE, OsStr::from_bytes(&self.to_bytes()?))?;

        return client.signal_observer();
    }

    /// Whether the pane matches the policy's targets, ignoring cached process marks.
    pub fn matches(&self, row: &PaneRow) -> bool {
        return match self.mode {
            TmuxScopeMode::All => true,
            TmuxScopeMode::Explicit => false,
            TmuxScopeMode::Selected => {
                let pane = &row.snapshot;

                self.pane_ids.contains(&pane.pane_id)
                    || self.session_ids.contains(&pane.session_id)
                    || self.window_ids.contains(&pane.window_id)
                    || self
                        .session_names
                        .iter()
                        .any(|name| return name.as_bytes() == pane.session_name.as_bytes())
                    || self.window_names.contains(&row.window_name)
            }
        };
    }

    /// Whether the pane's current process is selected, honoring cached process marks.
    pub fn selects(&self, row: &PaneRow) -> bool {
        if row.pid.is_some() && row.marks.selected == row.pid {
            return true;
        }

        if row.pid.is_some() && row.marks.excluded == row.pid {
            return false;
        }
        return self.matches(row);
    }
}

/// Cache a scope decision for the pane's current process.
///
/// # Errors
/// Returns tmux failures.
pub fn mark_decision(client: &TmuxClient, pane: &str, pid: u32, selected: bool) -> Result<()> {
    let (set, unset) = if selected {
        (options::SELECTED_PID, options::EXCLUDED_PID)
    } else {
        (options::EXCLUDED_PID, options::SELECTED_PID)
    };

    client.set_pane_option(pane, set, OsStr::new(&pid.to_string()))?;
    client.unset_pane_option(pane, unset)?;

    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{artifact::TmuxPaneSnapshot, client::PaneMarks};

    fn row(pid: u32, marks: PaneMarks) -> PaneRow {
        return PaneRow {
            dead: false,
            pid: Some(pid),
            marks,
            window_name: ByteString::from(b"editor".as_slice()),
            current_command: ByteString::default(),
            snapshot: TmuxPaneSnapshot {
                session_id: "$1".to_owned(),
                session_name: "work".to_owned(),
                window_id: "@2".to_owned(),
                window_index: 0,
                layout: String::new(),
                window_active: true,
                zoomed: false,
                pane_id: "%3".to_owned(),
                pane_active: true,
                copy_mode: false,
                pane_mode: String::new(),
                recording_session: None,
            },
        };
    }

    #[test]
    fn marks_for_the_current_process_override_targets() {
        let names = TmuxScopePolicy::initial(
            false,
            &ScopeTargets {
                windows: vec![OsString::from("editor")],
                ..ScopeTargets::default()
            },
        )
        .expect("window names");
        assert!(names.selects(&row(7, PaneMarks::default())));
        assert!(!names.selects(&row(
            7,
            PaneMarks {
                excluded: Some(7),
                ..PaneMarks::default()
            }
        )));
        assert!(names.selects(&row(
            7,
            PaneMarks {
                excluded: Some(6),
                ..PaneMarks::default()
            }
        )));
        assert!(TmuxScopePolicy::explicit().selects(&row(
            7,
            PaneMarks {
                selected: Some(7),
                ..PaneMarks::default()
            }
        )));
        assert!(!TmuxScopePolicy::explicit().matches(&row(7, PaneMarks::default())));
    }
}
