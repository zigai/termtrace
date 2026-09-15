use super::{
    Duration, EnvironmentSpec, Error, OsString, Path, PathBuf, Result, Session, SessionBuilder,
};

/// Process launcher and canonical evidence root.
#[derive(Clone, Debug)]
pub struct Terminal {
    pub(super) binary: PathBuf,
    pub(super) evidence_root: PathBuf,
}

impl Terminal {
    /// Create a blocking facade with an explicit executable and evidence root.
    pub fn new(binary: impl Into<PathBuf>, evidence_root: impl Into<PathBuf>) -> Self {
        return Self {
            binary: binary.into(),
            evidence_root: evidence_root.into(),
        };
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

    /// Configure a fresh canonical session launch.
    #[must_use]
    pub fn launch(&self, command: impl IntoIterator<Item = OsString>) -> SessionBuilder {
        return SessionBuilder {
            terminal: self.clone(),
            command: command.into_iter().collect(),
            cols: 160,
            rows: 45,
            working_directory: None,
            startup_timeout: Duration::from_secs(10),
            environment: EnvironmentSpec::default(),
            output_byte_budget: None,
        };
    }

    /// Attach a high-level handle without changing the session.
    pub fn attach(&self, session: impl AsRef<Path>) -> Result<Session> {
        let path = session.as_ref().canonicalize().map_err(|source| {
            return Error::Filesystem {
                path: session.as_ref().to_path_buf(),
                source,
            };
        })?;

        return Session::open(path, self.binary.clone(), None);
    }
}
