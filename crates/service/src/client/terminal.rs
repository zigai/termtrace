use super::{await_ready, client_error, LaunchSpec, Readiness, Session, SessionTarget};
use crate::{Error, Result};
use std::{
    fs::OpenOptions,
    io::Write as _,
    os::unix::fs::OpenOptionsExt as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::AtomicBool,
    time::Instant,
};
use termtrace_engine::recording::resolve_visual_profile;
use termtrace_media::native_capture::preflight_visual_profile;
use termtrace_model::{
    artifact::original::CellGrid,
    execution::visual_profile::{VisualProfile, VisualProfileReport},
};
use uuid::Uuid;

/// Termtrace executable and the caller-owned root for fresh canonical sessions.
#[derive(Clone, Debug)]
pub struct Terminal {
    binary: PathBuf,
    evidence_root: PathBuf,
}

impl Terminal {
    /// Launch and attach through `binary`, publishing fresh sessions under `evidence_root`.
    pub fn new(binary: impl Into<PathBuf>, evidence_root: impl Into<PathBuf>) -> Self {
        return Self {
            binary: binary.into(),
            evidence_root: evidence_root.into(),
        };
    }

    /// Typed recorder executable selected by this launcher.
    #[must_use]
    pub fn binary(&self) -> &Path {
        return &self.binary;
    }

    /// Caller-owned parent directory used for fresh canonical sessions.
    #[must_use]
    pub fn evidence_root(&self) -> &Path {
        return &self.evidence_root;
    }

    /// Clone this launcher with a different caller-owned evidence root.
    #[must_use]
    pub fn with_evidence_root(&self, evidence_root: impl Into<PathBuf>) -> Self {
        return Self {
            binary: self.binary.clone(),
            evidence_root: evidence_root.into(),
        };
    }

    /// Spawn a recorder for `spec` and return its owned session once it is reachable.
    ///
    /// A native visual profile is resolved and its fonts checked before anything is spawned;
    /// the recorder resolves it again and commits its own result.
    ///
    /// # Errors
    ///
    /// Returns an error when the spec is invalid, a native visual profile does not resolve,
    /// the recorder cannot be spawned, or it exits or misses the startup deadline without
    /// becoming reachable.
    pub fn start(&self, spec: LaunchSpec) -> Result<Session> {
        spec.validate()?;
        if let Some(profile) = &spec.visual_profile {
            resolve_visual_profile(profile, &AtomicBool::new(false))?;
        }

        let visual_profile = spec
            .visual_profile
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|source| {
                return Error::InvalidJson {
                    path: PathBuf::from("<visual-profile>"),
                    json_path: "$".to_owned(),
                    source,
                };
            })?;

        fs_err::create_dir_all(&self.evidence_root).map_err(|source| {
            return Error::Filesystem {
                path: self.evidence_root.clone(),
                source,
            };
        })?;

        let path = self
            .evidence_root
            .join(format!("{}.tt", Uuid::now_v7().simple()));
        let log_path = self
            .evidence_root
            .join(format!(".{}.driver.log", Uuid::now_v7()));

        let environment = serde_json::to_vec(&spec.environment).map_err(|source| {
            return Error::InvalidJson {
                path: PathBuf::from("<environment-spec-stdin>"),
                json_path: "$".to_owned(),
                source,
            };
        })?;

        let log_error = |source| {
            return Error::Filesystem {
                path: log_path.clone(),
                source,
            };
        };
        let log = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&log_path)
            .map_err(log_error)?;

        let mut process = Command::new(&self.binary);
        process
            .args(["record", "--no-input", "--cols"])
            .arg(spec.cols.to_string())
            .arg("--rows")
            .arg(spec.rows.to_string())
            .arg("--out")
            .arg(&path)
            .arg("--environment-spec-stdin");

        if let Some(profile) = &visual_profile {
            process.arg("--visual-profile").arg(profile);
        }

        if let Some(limit) = spec.output_byte_budget {
            process.arg("--output-byte-budget").arg(limit.to_string());
        }

        if let Some(limit) = spec.max_duration {
            process
                .arg("--max-duration")
                .arg(format!("{}ms", limit.as_millis()));
        }

        if let Some(limit) = spec.idle_timeout {
            process
                .arg("--idle-timeout")
                .arg(format!("{}ms", limit.as_millis()));
        }

        process
            .arg("--")
            .args(&spec.command)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(log.try_clone().map_err(log_error)?))
            .stderr(Stdio::from(log));

        if let Some(directory) = &spec.working_directory {
            process.current_dir(directory);
        }

        let mut recorder = process.spawn().map_err(|source| {
            return Error::Filesystem {
                path: self.binary.clone(),
                source,
            };
        })?;
        let written = recorder
            .stdin
            .take()
            .ok_or_else(|| return client_error("recorder environment pipe was not created"))
            .and_then(|mut stdin| {
                return stdin.write_all(&environment).map_err(|error| {
                    return client_error(format!("write environment pipe: {error}"));
                });
            });

        let deadline = Instant::now() + spec.startup_timeout;
        let readiness = written.and_then(|()| return await_ready(&path, &mut recorder, deadline));

        let failure = match readiness {
            Ok(Readiness::Live | Readiness::Completed(_)) => {
                return Session::open(path, self.binary.clone(), Some(recorder));
            }
            Ok(Readiness::Exited(status)) => {
                return Err(Error::RecorderStartup {
                    reason: format!(
                        "recorder exited before readiness with {status}; diagnostics: {}",
                        log_path.display()
                    ),
                });
            }
            Ok(Readiness::TimedOut) => Error::RecorderStartup {
                reason: format!(
                    "recorder readiness timed out; diagnostics: {}",
                    log_path.display()
                ),
            },
            Err(error) => error,
        };

        let _kill = recorder.kill();
        let _wait = recorder.wait();

        return Err(failure);
    }

    /// Validate `profile` and report the identity of everything a launch with it would use,
    /// solving the exact `cols` by `rows` grid at the profile's DPI when it selects native
    /// pixels. A semantic-only profile runs no native program. The report is advisory.
    ///
    /// # Errors
    ///
    /// Returns the classified visual-profile failure, or a media error when no viewport
    /// reproduces the grid.
    pub fn preflight_visual_profile(
        &self,
        profile: &VisualProfile,
        cols: u16,
        rows: u16,
    ) -> Result<VisualProfileReport> {
        return preflight_visual_profile(&self.binary, profile, CellGrid { cols, rows }).map_err(
            |error| match error {
                termtrace_media::Error::Engine(source) => return Error::Engine(source),
                other => return Error::Media(other),
            },
        );
    }

    /// Attach to an existing session without taking ownership of its recorder.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory is missing or has no valid manifest.
    pub fn attach(&self, session: impl AsRef<Path>) -> Result<Session> {
        return Session::attach(session, self.binary.clone());
    }

    /// Attach to a session named by path or registry alias.
    ///
    /// # Errors
    ///
    /// Returns an error when the target does not resolve or cannot be attached.
    pub fn attach_target(&self, target: &SessionTarget) -> Result<Session> {
        return self.attach(target.path()?);
    }
}
