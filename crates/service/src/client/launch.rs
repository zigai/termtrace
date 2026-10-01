use super::client_error;
use crate::{defaults, Error, Result};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Child, ExitStatus},
    time::{Duration, Instant},
};
use termtrace_control::{observe_phase, read_completed};
use termtrace_model::{
    execution::environment::{ColorPolicy, EnvironmentBase, EnvironmentSpec},
    execution::session::{SessionCompletion, SessionPhase},
    value::ByteString,
};
use termtrace_process::poll_until;

const READINESS_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// How long a recorder must keep answering before it counts as ready, so a child that exits
/// immediately is reported as completed rather than momentarily live.
const READINESS_SETTLE: Duration = Duration::from_millis(30);

/// What a fresh recording runs and how it starts.
#[derive(Clone, Debug)]
pub struct LaunchSpec {
    /// Child command and arguments.
    pub command: Vec<OsString>,
    /// Initial terminal columns.
    pub cols: u16,
    /// Initial terminal rows.
    pub rows: u16,
    /// Child working directory, or the caller's when `None`.
    pub working_directory: Option<PathBuf>,
    /// How long the recorder may take to answer its control socket.
    pub startup_timeout: Duration,
    /// Complete deterministic child-environment policy.
    pub environment: EnvironmentSpec,
    /// Terminate runaway work after this many output bytes without truncating evidence.
    pub output_byte_budget: Option<u64>,
}

impl LaunchSpec {
    /// Run `command` with every other setting at its default.
    pub fn new(command: impl IntoIterator<Item = impl Into<OsString>>) -> Self {
        return Self {
            command: command.into_iter().map(Into::into).collect(),
            cols: defaults::COLS,
            rows: defaults::ROWS,
            working_directory: None,
            startup_timeout: defaults::STARTUP_TIMEOUT,
            environment: EnvironmentSpec::default(),
            output_byte_budget: None,
        };
    }

    /// Set the immutable initial grid.
    #[must_use]
    pub const fn geometry(mut self, cols: u16, rows: u16) -> Self {
        self.cols = cols;
        self.rows = rows;

        return self;
    }

    /// Set the child working directory.
    #[must_use]
    pub fn working_directory(mut self, path: impl Into<PathBuf>) -> Self {
        self.working_directory = Some(path.into());

        return self;
    }

    /// Set the recorder readiness deadline.
    #[must_use]
    pub const fn startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;

        return self;
    }

    /// Replace the complete deterministic child-environment policy.
    #[must_use]
    pub fn environment(mut self, environment: EnvironmentSpec) -> Self {
        self.environment = environment;

        return self;
    }

    /// Begin with an empty child environment instead of inheriting the recorder environment.
    #[must_use]
    pub fn clear_environment(mut self) -> Self {
        self.environment.base = EnvironmentBase::Clear;

        return self;
    }

    /// Set one child variable without putting its value in process arguments or logs.
    #[must_use]
    pub fn environment_variable(
        mut self,
        name: impl Into<String>,
        value: impl Into<Vec<u8>>,
    ) -> Self {
        self.environment
            .set
            .insert(name.into(), ByteString::new(value.into()));

        return self;
    }

    /// Remove one child variable after base-environment resolution.
    #[must_use]
    pub fn unset_environment(mut self, name: impl Into<String>) -> Self {
        self.environment.unset.insert(name.into());

        return self;
    }

    /// Copy one variable from the recorder snapshot into an otherwise clear environment.
    #[must_use]
    pub fn copy_environment(mut self, name: impl Into<String>) -> Self {
        self.environment.copy.insert(name.into());

        return self;
    }

    /// Select deterministic common color-variable behavior.
    #[must_use]
    pub const fn color_policy(mut self, policy: ColorPolicy) -> Self {
        self.environment.color = policy;

        return self;
    }

    /// Terminate runaway work after this many output bytes without truncating evidence.
    #[must_use]
    pub const fn output_byte_budget(mut self, bytes: u64) -> Self {
        self.output_byte_budget = Some(bytes);

        return self;
    }

    pub(super) fn validate(&self) -> Result<()> {
        if self.command.is_empty() {
            return Err(client_error("launch command is empty"));
        }

        if self.cols == 0 || self.rows == 0 {
            return Err(client_error("launch geometry must be nonzero"));
        }
        return self
            .environment
            .validate()
            .map_err(|source| return client_error(source.to_string()));
    }
}

/// How a freshly spawned recorder ended its startup.
#[derive(Debug)]
pub enum Readiness {
    /// The recorder answers its control socket.
    Live,
    /// The session completed before or while the recorder became reachable.
    Completed(Box<SessionCompletion>),
    /// The recorder exited without publishing completion.
    Exited(ExitStatus),
    /// Neither happened before the deadline.
    TimedOut,
}

/// Wait until a freshly spawned recorder answers for `session`, completes, or exits.
///
/// # Errors
///
/// Returns an error when the recorder cannot be polled or published evidence is invalid.
pub fn await_ready(session: &Path, recorder: &mut Child, deadline: Instant) -> Result<Readiness> {
    let mut live_since: Option<Instant> = None;
    let readiness = poll_until(
        deadline,
        READINESS_POLL_INTERVAL,
        || -> Result<Option<Readiness>> {
            match observe_phase(session).phase {
                SessionPhase::Live => {
                    if live_since.get_or_insert_with(Instant::now).elapsed() >= READINESS_SETTLE {
                        return Ok(Some(Readiness::Live));
                    }
                }
                SessionPhase::Completed => {
                    if let Some(completion) = read_completed(session)? {
                        return Ok(Some(Readiness::Completed(Box::new(completion))));
                    }
                }
                _ => {}
            }

            let Some(status) = recorder.try_wait().map_err(|source| {
                return Error::Filesystem {
                    path: session.to_path_buf(),
                    source,
                };
            })?
            else {
                return Ok(None);
            };

            return Ok(Some(read_completed(session)?.map_or(
                Readiness::Exited(status),
                |completion| {
                    return Readiness::Completed(Box::new(completion));
                },
            )));
        },
    )?;

    return Ok(readiness.unwrap_or(Readiness::TimedOut));
}
