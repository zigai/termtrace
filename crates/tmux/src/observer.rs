//! Server-lifetime observer processes: the pane guard, supervisor, and watchdog.
//!
//! The supervisor and the watchdog each hold a crash-released lock for one server socket.
//! The watchdog restarts a dead supervisor; the supervisor restarts a dead watchdog.
//! A lock file records its holder's PID, so a held lock names a live observer.

use crate::{
    artifact::{TmuxRecordingCoverage, TmuxServerIdentity},
    client::{valid_identifier, TmuxClient},
    error::{Error, Result},
    options::{SUPERVISOR_INTERVAL, UNRECORDED_GRACE},
    recording,
    scope::mark_decision,
    setup::PreparedTmuxSetup,
    status::{self, Integration, PaneCoverage},
};
use nix::unistd::geteuid;
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    os::unix::{ffi::OsStrExt as _, fs::MetadataExt as _},
    path::{Path, PathBuf},
    process::{self, Command},
    thread,
    time::{Duration, Instant},
};
use termtrace_process::ExclusiveLock;

const WATCHDOG_RESTART_BACKOFF: Duration = Duration::from_secs(2);

/// Observer process roles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObserverRole {
    /// Audits pane coverage and refreshes the aggregate.
    Supervisor,
    /// Restarts the supervisor.
    Watchdog,
}

/// PIDs of live observers.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Observers {
    /// Supervisor process, if one holds its lock.
    pub supervisor: Option<u32>,
    /// Watchdog process, if one holds its lock.
    pub watchdog: Option<u32>,
}

/// A held observer role, released when dropped or when the process dies.
#[derive(Debug)]
pub struct ObserverLock {
    _lock: ExclusiveLock,
}

impl ObserverRole {
    fn name(self) -> &'static str {
        return match self {
            Self::Supervisor => "supervisor",
            Self::Watchdog => "watchdog",
        };
    }

    fn lock_path(self, state_root: &Path, server: &TmuxServerIdentity) -> PathBuf {
        return state_root.join(format!(
            ".tmux-{}-{}-{}.lock",
            server.socket_device,
            server.socket_inode,
            self.name()
        ));
    }

    /// Claim this role for the server, or none when another process holds it.
    ///
    /// # Errors
    /// Returns a socket not owned by the current user or lock failures.
    pub fn claim(
        self,
        state_root: &Path,
        server: &TmuxServerIdentity,
    ) -> Result<Option<ObserverLock>> {
        let socket = Path::new(OsStr::from_bytes(server.socket_path.as_bytes()));
        let metadata = fs_err::symlink_metadata(socket)
            .map_err(|error| return Error::filesystem(socket, error))?;
        if metadata.uid() != geteuid().as_raw() {
            return Err(Error::invalid(
                "observer requires a tmux socket owned by the current user",
            ));
        }

        let path = self.lock_path(state_root, server);
        let Some(lock) = ExclusiveLock::try_acquire(&path)
            .map_err(|error| return Error::filesystem(&path, error))?
        else {
            return Ok(None);
        };

        let mut file = lock.file();
        file.set_len(0)
            .and_then(|()| return file.seek(SeekFrom::Start(0)).map(drop))
            .and_then(|()| return file.write_all(process::id().to_string().as_bytes()))
            .map_err(|error| return Error::filesystem(&path, error))?;

        return Ok(Some(ObserverLock { _lock: lock }));
    }

    /// PID of the process holding this role, if any.
    pub fn holder(self, state_root: &Path, server: &TmuxServerIdentity) -> Option<u32> {
        let path = self.lock_path(state_root, server);
        if !path.is_file() {
            return None;
        }

        match ExclusiveLock::try_acquire(&path) {
            Ok(None) => {}
            Ok(Some(_)) | Err(_) => return None,
        }

        let mut text = String::new();
        fs_err::File::open(&path)
            .ok()?
            .read_to_string(&mut text)
            .ok()?;

        return text.trim().parse().ok();
    }
}

/// Enforcement for one pane found by an audit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Enforcement {
    /// Cache that the pane's current process is out of scope.
    MarkExcluded {
        /// Pane ID.
        pane_id: String,
        /// Excluded process.
        pid: u32,
    },
    /// Report an unrecorded selected pane.
    Warn {
        /// Pane ID.
        pane_id: String,
    },
    /// Close an unrecorded selected pane under strict coverage.
    Kill {
        /// Pane ID.
        pane_id: String,
    },
}

/// Decide enforcement for one audit; `suspects` carries first-seen times between audits.
pub fn audit(
    panes: &[PaneCoverage],
    coverage: TmuxRecordingCoverage,
    suspects: &mut BTreeMap<String, Instant>,
    now: Instant,
) -> Vec<Enforcement> {
    let present = panes
        .iter()
        .map(|pane| return pane.row.snapshot.pane_id.clone())
        .collect::<BTreeSet<_>>();
    suspects.retain(|pane, _| return present.contains(pane));

    let mut enforcement = Vec::new();
    for pane in panes {
        let pane_id = &pane.row.snapshot.pane_id;
        let Some(pid) = pane.row.pid else {
            continue;
        };

        if pane.row.marks.grandfathered == Some(pid) {
            suspects.remove(pane_id);
            continue;
        }

        if !pane.selected {
            if pane.row.marks.excluded != Some(pid) {
                enforcement.push(Enforcement::MarkExcluded {
                    pane_id: pane_id.clone(),
                    pid,
                });
            }

            suspects.remove(pane_id);
            continue;
        }

        if pane.recorded {
            suspects.remove(pane_id);
            continue;
        }

        let first_seen = *suspects.entry(pane_id.clone()).or_insert(now);
        if now.saturating_duration_since(first_seen) < UNRECORDED_GRACE {
            continue;
        }

        suspects.remove(pane_id);
        enforcement.push(match coverage {
            TmuxRecordingCoverage::Strict => Enforcement::Kill {
                pane_id: pane_id.clone(),
            },
            TmuxRecordingCoverage::Warn => Enforcement::Warn {
                pane_id: pane_id.clone(),
            },
        });
    }

    return enforcement;
}

/// Whether the supervisor should restart a watchdog whose lock is free.
pub fn watchdog_restart_due(
    watchdog_alive: bool,
    last_restart: Option<Instant>,
    now: Instant,
) -> bool {
    return !watchdog_alive
        && last_restart.is_none_or(|last| {
            return now.saturating_duration_since(last) >= WATCHDOG_RESTART_BACKOFF;
        });
}

/// Enforce coverage for one newly created pane.
///
/// # Errors
/// Returns an invalid pane ID or tmux failures after the grace period.
pub fn guard(client: &TmuxClient, pane: &str) -> Result<()> {
    if !valid_identifier(pane, b'%') {
        return Err(Error::invalid(
            "tmux guard received an invalid pane identifier",
        ));
    }

    let Some(integration) = Integration::read(client)? else {
        return Ok(());
    };
    let Ok(row) = client.pane_row(pane) else {
        return Ok(());
    };
    let Some(pid) = row.pid else {
        return Ok(());
    };

    if !integration.scope.selects(&row) {
        return mark_decision(client, pane, pid, false);
    }

    let deadline = Instant::now() + UNRECORDED_GRACE;
    loop {
        match client.pane_number::<u32>(pane, "#{pane_pid}") {
            Err(_) => return Ok(()),
            Ok(pid) if status::is_pane_wrapper(pid, &integration.termtrace) => return Ok(()),
            Ok(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(_) => break,
        }
    }

    enforce(
        client,
        &match integration.coverage {
            TmuxRecordingCoverage::Strict => Enforcement::Kill {
                pane_id: pane.to_owned(),
            },
            TmuxRecordingCoverage::Warn => Enforcement::Warn {
                pane_id: pane.to_owned(),
            },
        },
        integration.coverage,
        "",
    );

    return Ok(());
}

fn enforce(
    client: &TmuxClient,
    enforcement: &Enforcement,
    coverage: TmuxRecordingCoverage,
    when: &str,
) {
    match enforcement {
        Enforcement::MarkExcluded { pane_id, pid } => {
            if let Err(error) = mark_decision(client, pane_id, *pid, false) {
                tracing::debug!(%error, pane = %pane_id, "could not cache excluded tmux pane process");
            }
        }
        Enforcement::Warn { pane_id } => {
            tracing::warn!(
                pane = %pane_id,
                "Termtrace found unrecorded pane {pane_id}{when} (coverage={})",
                coverage.as_str()
            );
        }
        Enforcement::Kill { pane_id } => {
            let message = format!(
                "Termtrace found unrecorded pane {pane_id}{when} (coverage={})",
                coverage.as_str()
            );
            let _display = client.run(["display-message", "-d", "5000", message.as_str()]);
            let _kill = client.run(["kill-pane", "-t", pane_id.as_str()]);
        }
    }
}

/// Supervise the inherited server until its integration is disabled.
///
/// Each tick lists panes once and uses that snapshot for coverage enforcement and for the
/// aggregate recording.
///
/// # Errors
/// Returns a failure to claim the role or to read settings.
pub fn supervise(client: &TmuxClient) -> Result<()> {
    let Some(integration) = Integration::read(client)? else {
        return Ok(());
    };
    let identity = client.server_identity()?;
    let Some(_lock) = ObserverRole::Supervisor.claim(&integration.output_root, &identity)? else {
        return Ok(());
    };

    if let Some(path) = &integration.aggregate_manifest {
        recording::observer_started(path, process::id())?;
    }

    let mut suspects = BTreeMap::new();
    let mut watchdog_restarted = None;

    loop {
        let Ok(Some(integration)) = Integration::read(client) else {
            return Ok(());
        };
        let Ok(rows) = client.list_panes() else {
            return Ok(());
        };
        let panes = status::classify(Some(&integration), &rows, |pid| {
            return status::is_pane_wrapper(pid, &integration.termtrace);
        });

        if let Some(path) = &integration.aggregate_manifest
            && let Err(error) = recording::refresh_from(path, client, &rows, &panes)
        {
            tracing::warn!(%error, "could not refresh aggregate tmux manifest");
        }

        for enforcement in audit(&panes, integration.coverage, &mut suspects, Instant::now()) {
            enforce(
                client,
                &enforcement,
                integration.coverage,
                " after its process changed",
            );
        }

        let now = Instant::now();
        let watchdog_alive = ObserverRole::Watchdog
            .holder(&integration.output_root, &identity)
            .is_some();
        if watchdog_restart_due(watchdog_alive, watchdog_restarted, now) {
            watchdog_restarted = Some(now);
            match PreparedTmuxSetup::from_options(client) {
                Ok(setup) => {
                    if let Err(error) = client.run_shell_background(&setup.watchdog_command()) {
                        tracing::warn!(%error, "could not restart tmux watchdog");
                    }
                }
                Err(error) => tracing::warn!(%error, "could not restart tmux watchdog"),
            }
        }

        thread::sleep(SUPERVISOR_INTERVAL);
    }
}

/// Restart the supervisor whenever its lock is free, until the integration is disabled.
///
/// # Errors
/// Returns a failure to claim the role or to read settings.
pub fn watchdog(client: &TmuxClient) -> Result<()> {
    let Some(integration) = Integration::read(client)? else {
        return Ok(());
    };
    let identity = client.server_identity()?;
    let Some(_lock) = ObserverRole::Watchdog.claim(&integration.output_root, &identity)? else {
        return Ok(());
    };

    while client.integration_enabled() {
        if ObserverRole::Supervisor
            .holder(&integration.output_root, &identity)
            .is_none()
        {
            // Waiting owns and reaps the child; both roles hold independent
            // crash-released locks.
            match Command::new(&integration.termtrace)
                .args(["tmux", "supervise", "-q"])
                .status()
            {
                Ok(status) => tracing::debug!(%status, "tmux observer exited"),
                Err(error) => tracing::warn!(%error, "could not start tmux observer"),
            }
        }

        if client.integration_enabled() {
            thread::sleep(SUPERVISOR_INTERVAL);
        }
    }

    return Ok(());
}

#[cfg(test)]
mod tests;
