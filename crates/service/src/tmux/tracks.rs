//! Reusable canonical track effects; the CLI and SDK use the same adapter.
use crate::{
    client::{Session, StopRequest},
    defaults,
    registry::{DetachedStart, RegistryService},
};
use std::{
    env,
    ffi::OsString,
    fs, io,
    os::unix::{ffi::OsStrExt as _, fs::PermissionsExt as _},
    path::{Path, PathBuf},
    time::Duration,
};
use termtrace_model::value::ByteString;
use termtrace_tmux::{artifact::TmuxSessionLink, recording::Tracks, Error, Result};

/// Effects backed by the managed registry and the original service session operations.
pub struct ServiceTracks {
    registry: RegistryService,
    binary: PathBuf,
}

impl ServiceTracks {
    /// Open an adapter using a caller-selected recorder and explicit/default state root.
    /// # Errors
    /// Propagates state-root security and filesystem errors.
    pub fn open(binary: impl Into<PathBuf>, root: Option<&Path>) -> crate::Result<Self> {
        return Ok(Self {
            registry: root.map_or_else(RegistryService::open_default, RegistryService::open)?,
            binary: binary.into(),
        });
    }

    /// Resolve the selected recorder, including a plain program name on PATH.
    /// Offline aggregate reads do not need this and remain available without a recorder.
    /// # Errors
    /// Returns an unavailable selected executable; never substitutes the host interpreter.
    pub fn recorder_executable(&self) -> crate::Result<PathBuf> {
        let candidate = if self.binary.as_os_str().as_bytes().contains(&b'/') {
            Some(self.binary.clone())
        } else {
            env::var_os("PATH").and_then(|paths| {
                return env::split_paths(&paths)
                    .map(|root| return root.join(&self.binary))
                    .find(|path| {
                        return path.metadata().is_ok_and(|metadata| {
                            return metadata.is_file()
                                && metadata.permissions().mode() & 0o111 != 0;
                        });
                    });
            })
        };

        let path = candidate.ok_or_else(|| {
            return crate::Error::Filesystem {
                path: self.binary.clone(),
                source: io::Error::new(
                    io::ErrorKind::NotFound,
                    "selected Termtrace executable is not available on PATH",
                ),
            };
        })?;

        return fs::canonicalize(&path)
            .map_err(|source| return crate::Error::Filesystem { path, source });
    }
}

fn failed(operation: &'static str, error: crate::Error) -> Error {
    return Error::TrackSource {
        operation,
        source: Box::new(error),
    };
}

impl Tracks for ServiceTracks {
    fn state_root(&self) -> &Path {
        return self.registry.state_root();
    }

    fn reserve(&self, alias: &str) -> Result<()> {
        return self
            .registry
            .ensure_available(alias)
            .map_err(|error| return failed("composite alias reservation", error));
    }

    fn start(&self, alias: &str, command: &[OsString]) -> Result<TmuxSessionLink> {
        if command.is_empty() || command.first().is_some_and(|value| return value.is_empty()) {
            return Err(Error::Track {
                operation: "composite tmux client track start",
                message: "command must not be empty".into(),
            });
        }

        let started = self
            .registry
            .start_detached(
                &self.binary,
                DetachedStart {
                    alias: alias.into(),
                    session: None,
                    startup_timeout: defaults::STARTUP_TIMEOUT,
                    stdin: None,
                },
                |session| {
                    let mut args = vec![
                        OsString::from("record"),
                        "--no-input".into(),
                        "--out".into(),
                        session.as_os_str().into(),
                        "--session-alias".into(),
                        alias.into(),
                        "--".into(),
                    ];
                    args.extend(command.iter().cloned());

                    return args;
                },
            )
            .map_err(|error| return failed("composite tmux client track start", error))?;

        return Ok(TmuxSessionLink {
            session_id: started.entry.session_id,
            session: ByteString::from(started.entry.session_path().as_os_str().as_bytes()),
        });
    }

    fn resolve(&self, alias: &str) -> Result<PathBuf> {
        return self
            .registry
            .resolve(alias)
            .map(|entry| return entry.session_path())
            .map_err(|error| return failed("composite alias resolution", error));
    }

    fn stop(&self, session: &Path, wait: bool, timeout: Duration) -> Result<()> {
        return Session::attach(session, &self.binary)
            .and_then(|mut session| {
                return session.stop(StopRequest::default(), wait.then_some(timeout));
            })
            .map(drop)
            .map_err(|error| return failed("composite tmux client track stop", error));
    }

    fn wait_completed(&self, session: &Path, timeout: Duration) -> Result<()> {
        return Session::attach(session, &self.binary)
            .and_then(|mut session| return session.complete(timeout))
            .map(drop)
            .map_err(|error| return failed("tmux track completion", error));
    }
}
