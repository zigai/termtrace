//! Recorders that outlive their caller, registered under a reserved alias.

use super::RegistryService;
use crate::{
    client::{await_ready, Readiness},
    error::{Error, Result},
};
use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use termtrace_model::{
    execution::session::SessionCompletion,
    registry::{AliasReservation, DetachedSessionPaths, RegistryEntry},
};

/// Extra alias-lease time beyond the startup deadline, so a recorder that becomes ready at the
/// deadline can still promote its reservation.
const RESERVATION_GRACE: Duration = Duration::from_secs(5);
const LOG_TAIL_BYTES: u64 = 4096;

/// A detached recording to start under a reserved registry alias.
#[derive(Clone, Debug)]
pub struct DetachedStart {
    /// Registry alias reserved for the pending session and promoted once it is ready.
    pub alias: String,
    /// Canonical session path; a fresh managed path when `None`.
    pub session: Option<PathBuf>,
    /// How long the recorder may take to answer or complete.
    pub startup_timeout: Duration,
    /// Bytes written to the recorder's standard input, then closed; `/dev/null` when `None`.
    pub stdin: Option<Vec<u8>>,
}

/// A detached recording whose alias now names its canonical session.
#[derive(Debug)]
pub struct DetachedRecording {
    /// Promoted registry binding.
    pub entry: RegistryEntry,
    /// Canonical session path and the recorder's owner-only diagnostic logs.
    pub paths: DetachedSessionPaths,
    /// Validated completion when the recording finished before it was observed live.
    pub completion: Option<Box<SessionCompletion>>,
}

impl RegistryService {
    /// Reserve `request.alias`, spawn `binary` with the recorder arguments for the allocated
    /// session in its own process session, and promote the alias once the recorder answers or
    /// completes.
    ///
    /// The reservation is released when the recorder cannot start or exits without completion
    /// evidence; it stays recoverable when readiness times out or promotion fails.
    ///
    /// # Errors
    /// Returns reservation, spawn, readiness, or promotion failures.
    pub fn start_detached(
        &self,
        binary: &Path,
        request: DetachedStart,
        recorder_arguments: impl FnOnce(&Path) -> Vec<OsString>,
    ) -> Result<DetachedRecording> {
        let mut paths = self.registry.allocate_detached_paths()?;

        if let Some(session) = request.session {
            paths.session = session;
        }

        let reservation = self.registry.reserve(
            &request.alias,
            &paths.session,
            request.startup_timeout.saturating_add(RESERVATION_GRACE),
        )?;

        let spawned = spawn_detached(
            binary,
            recorder_arguments(&paths.session),
            &paths,
            request.stdin.as_deref(),
        );

        let mut recorder = match spawned {
            Ok(recorder) => recorder,
            Err(error) => {
                self.release_reservation(&reservation);

                return Err(error);
            }
        };

        let deadline = Instant::now() + request.startup_timeout;
        let readiness = await_ready(&paths.session, &mut recorder, deadline);
        // The CLI used to exit after this operation. Persistent SDK callers must
        // retain and eventually reap the same Child, including readiness failures.
        // This does not kill an orphan or change lease/promotion error semantics.
        thread::spawn(move || {
            let _status = recorder.wait();
        });

        let completion = match readiness? {
            Readiness::Live => None,
            Readiness::Completed(completion) => Some(completion),
            Readiness::Exited(status) => {
                self.release_reservation(&reservation);

                let mut reason = format!(
                    "detached recorder exited before publishing valid completion evidence with {status}"
                );

                if let Some(cause) = last_log_line(&paths.stderr_log) {
                    reason = format!("{reason}: {cause}");
                }

                return Err(startup_failure(&reason, &paths));
            }
            Readiness::TimedOut => {
                return Err(startup_failure(
                    &format!(
                        "detached recorder did not become ready within {} ms; its owned alias reservation and pending session remain recoverable",
                        request.startup_timeout.as_millis()
                    ),
                    &paths,
                ));
            }
        };

        let entry = self.registry.promote(&reservation).map_err(|error| {
            return startup_failure(
                &format!(
                    "recorder started but owned alias promotion failed; the reservation remains ({error})"
                ),
                &paths,
            );
        })?;

        return Ok(DetachedRecording {
            entry,
            paths,
            completion,
        });
    }

    pub(crate) fn reserve_session_alias(
        &self,
        alias: &str,
        session: &Path,
        lease: Duration,
    ) -> Result<AliasReservation> {
        return Ok(self.registry.reserve(alias, session, lease)?);
    }

    pub(crate) fn promote_session_alias(
        &self,
        reservation: &AliasReservation,
    ) -> Result<RegistryEntry> {
        return Ok(self.registry.promote(reservation)?);
    }

    pub(crate) fn release_session_alias(&self, reservation: &AliasReservation) {
        self.release_reservation(reservation);
    }

    fn release_reservation(&self, reservation: &AliasReservation) {
        if let Err(error) = self.registry.release(reservation) {
            tracing::warn!(%error, alias = %reservation.alias, "failed to release startup reservation");
        }
    }
}

/// Spawn `binary` with `arguments` in its own process session, logging to the owner-only
/// diagnostic logs of `paths` and writing `stdin` to its standard input.
///
/// # Errors
/// Returns log creation, spawn, or standard-input failures.
pub fn spawn_detached(
    binary: &Path,
    arguments: Vec<OsString>,
    paths: &DetachedSessionPaths,
    stdin: Option<&[u8]>,
) -> Result<Child> {
    let stdout = open_log(&paths.stdout_log)?;
    let stderr = open_log(&paths.stderr_log)?;
    let mut recorder = Command::new("setsid")
        .arg(binary)
        .args(arguments)
        .env_remove("TERMTRACE_RESPONSE")
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|source| {
            return Error::Filesystem {
                path: binary.to_path_buf(),
                source,
            };
        })?;

    if let Some(bytes) = stdin {
        let Some(mut pipe) = recorder.stdin.take() else {
            let _kill = recorder.kill();
            let _wait = recorder.wait();

            return Err(Error::RecorderStartup {
                reason: "detached recorder input pipe was not created".to_owned(),
            });
        };

        if let Err(source) = pipe.write_all(bytes) {
            let _kill = recorder.kill();
            let _wait = recorder.wait();

            return Err(Error::Filesystem {
                path: binary.to_path_buf(),
                source,
            });
        }
    }

    return Ok(recorder);
}

fn open_log(path: &Path) -> Result<File> {
    return OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        });
}

fn last_log_line(log: &Path) -> Option<String> {
    let mut file = File::open(log).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(LOG_TAIL_BYTES)))
        .ok()?;
    let mut tail = Vec::new();
    file.take(LOG_TAIL_BYTES).read_to_end(&mut tail).ok()?;
    let tail = String::from_utf8_lossy(&tail);
    let line = tail
        .lines()
        .rev()
        .find(|line| return !line.trim().is_empty())?;

    return Some(line.trim().trim_start_matches("termtrace: ").to_owned());
}

fn startup_failure(reason: &str, paths: &DetachedSessionPaths) -> Error {
    return Error::RecorderStartup {
        reason: format!(
            "{reason}; session candidate: {}; stdout log: {}; stderr log: {}",
            paths.session.display(),
            paths.stdout_log.display(),
            paths.stderr_log.display(),
        ),
    };
}
