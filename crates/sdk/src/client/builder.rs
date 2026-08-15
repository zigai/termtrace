use super::control::client_error;
use super::*;

/// Fresh-session launch configuration.
#[derive(Clone, Debug)]
pub struct SessionBuilder {
    pub(super) terminal: Terminal,
    pub(super) command: Vec<OsString>,
    pub(super) cols: u16,
    pub(super) rows: u16,
    pub(super) working_directory: Option<PathBuf>,
    pub(super) startup_timeout: Duration,
    pub(super) environment: EnvironmentSpec,
    pub(super) output_byte_budget: Option<u64>,
}

impl SessionBuilder {
    /// Set the immutable initial grid.
    #[must_use]
    pub fn geometry(mut self, cols: u16, rows: u16) -> Self {
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

    /// Set the actor readiness deadline.
    #[must_use]
    pub fn startup_timeout(mut self, timeout: Duration) -> Self {
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
    pub fn set_environment(mut self, name: impl Into<String>, value: impl Into<Vec<u8>>) -> Self {
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
    pub fn color_policy(mut self, policy: ColorPolicy) -> Self {
        self.environment.color = policy;
        return self;
    }

    /// Terminate runaway work after this many output bytes without truncating evidence.
    #[must_use]
    pub fn output_byte_budget(mut self, bytes: u64) -> Self {
        self.output_byte_budget = Some(bytes);
        return self;
    }

    /// Spawn the recorder and wait for actor readiness.
    pub fn start(self) -> Result<Session> {
        if self.command.is_empty() {
            return Err(client_error("launch command is empty"));
        }
        fs_err::create_dir_all(&self.terminal.evidence_root).map_err(|source| {
            return Error::Filesystem {
                path: self.terminal.evidence_root.clone(),
                source,
            };
        })?;
        self.environment
            .validate()
            .map_err(|source| return client_error(source.to_string()))?;
        let path = self
            .terminal
            .evidence_root
            .join(format!("{}.tt", Uuid::now_v7().simple()));
        let log_path = self
            .terminal
            .evidence_root
            .join(format!(".{}.driver.log", Uuid::now_v7()));
        let environment_bytes = serde_json::to_vec(&self.environment).map_err(|source| {
            return Error::InvalidJson {
                path: PathBuf::from("<environment-spec-stdin>"),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        let log = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&log_path)
            .map_err(|source| {
                return Error::Filesystem {
                    path: log_path.clone(),
                    source,
                };
            })?;
        let mut process = Command::new(&self.terminal.binary);
        process
            .args([
                "record",
                "--no-input",
                "--cols",
                &self.cols.to_string(),
                "--rows",
                &self.rows.to_string(),
                "--out",
            ])
            .arg(&path)
            .arg("--environment-spec-stdin");
        if let Some(limit) = self.output_byte_budget {
            process.arg("--output-byte-budget").arg(limit.to_string());
        }
        process
            .arg("--")
            .args(&self.command)
            .stdin(Stdio::piped())
            .stdout(Stdio::from(log.try_clone().map_err(|source| {
                return Error::Filesystem {
                    path: log_path.clone(),
                    source,
                };
            })?))
            .stderr(Stdio::from(log));
        if let Some(directory) = self.working_directory {
            process.current_dir(directory);
        }
        let mut child = process.spawn().map_err(|source| {
            return Error::Filesystem {
                path: self.terminal.binary.clone(),
                source,
            };
        })?;
        let environment_write = child
            .stdin
            .take()
            .ok_or_else(|| return client_error("recorder environment pipe was not created"))
            .and_then(|mut stdin| {
                return stdin.write_all(&environment_bytes).map_err(|error| {
                    return client_error(format!("write environment pipe: {error}"));
                });
            });
        if let Err(error) = environment_write {
            let _kill = child.kill();
            let _wait = child.wait();
            return Err(error);
        }
        let deadline = Instant::now() + self.startup_timeout;
        while !control_endpoint_is_reachable(&path) {
            let status = child.try_wait().map_err(|source| {
                let _kill = child.kill();
                return Error::Filesystem {
                    path: self.terminal.binary.clone(),
                    source,
                };
            })?;
            if let Some(status) = status {
                return Err(client_error(format!(
                    "recorder exited before readiness with {status}"
                )));
            }
            if Instant::now() >= deadline {
                let _kill = child.kill();
                let _wait = child.wait();
                return Err(client_error(format!(
                    "recorder readiness timed out; diagnostics: {}",
                    log_path.display()
                )));
            }
            thread::sleep(Duration::from_millis(10));
        }
        return Session::open(path, Some(child));
    }
}
