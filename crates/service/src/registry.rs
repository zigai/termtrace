//! Friendly session registry, retention, garbage collection, and restart services.

use crate::error::{Error, Result};
use std::io::ErrorKind::NotFound as StdNotFound;
use std::path::{Path, PathBuf};
use termtrace_engine::registry::SessionRegistry;
use termtrace_model::registry::{
    GarbageCollectionReport, RegistryEntry, RegistryListEntry, RegistrySessionResolution,
};

mod detached;
mod restart;

pub use detached::{spawn_detached, DetachedRecording, DetachedStart};

/// Semantic override policy for restart orchestration.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ValueOverride<T> {
    /// Retain the predecessor configuration value.
    #[default]
    Keep,
    /// Replace with an explicit new value.
    Set(T),
    /// Clear or unset the value.
    Clear,
}

/// Restart configuration overrides preserving predecessor configuration by default.
#[derive(Clone, Debug, Default)]
pub struct RestartOverrides {
    /// Terminal columns override.
    pub cols: ValueOverride<u16>,
    /// Terminal rows override.
    pub rows: ValueOverride<u16>,
    /// Working directory override.
    pub working_dir: ValueOverride<PathBuf>,
    /// Output byte limit override.
    pub output_byte_budget: ValueOverride<u64>,
}

/// Service managing friendly session names, discovery, and retention.
pub struct RegistryService {
    registry: SessionRegistry,
}

impl RegistryService {
    /// Open the default user session registry.
    ///
    /// # Errors
    /// Returns an error if the registry directory cannot be prepared or opened.
    pub fn open_default() -> Result<Self> {
        let registry = SessionRegistry::open_default().map_err(Error::from)?;

        return Ok(Self { registry });
    }

    /// Open a registry at an explicit state root.
    ///
    /// # Errors
    /// Returns an error if the root cannot be prepared.
    pub fn open(state_root: impl AsRef<Path>) -> Result<Self> {
        let registry = SessionRegistry::open(state_root).map_err(Error::from)?;

        return Ok(Self { registry });
    }

    /// Managed state root holding sessions, the registry, and aggregate recordings.
    #[must_use]
    pub fn state_root(&self) -> &Path {
        return self.registry.state_root();
    }

    /// Fail unless `alias` is valid and neither reserved nor registered.
    ///
    /// # Errors
    /// Returns an error if the alias is invalid, taken, or the registry cannot be read.
    pub fn ensure_available(&self, alias: &str) -> Result<()> {
        return self.registry.ensure_available(alias).map_err(Error::from);
    }

    /// Register a friendly name for an existing canonical session directory.
    ///
    /// # Errors
    /// Returns an error if the name is invalid or already registered.
    pub fn register(&self, alias: &str, session: &Path) -> Result<RegistryEntry> {
        return self.registry.register(alias, session).map_err(Error::from);
    }

    /// Resolve a friendly name to its registry entry.
    ///
    /// # Errors
    /// Returns an error if the alias does not exist or is invalid.
    pub fn resolve(&self, alias: &str) -> Result<RegistryEntry> {
        return self.registry.resolve(alias).map_err(Error::from);
    }

    /// Resolve a name optionally, distinguishing a missing alias from corrupt/inaccessible evidence.
    /// # Errors
    /// Returns every resolution failure except a missing alias record in an intact registry.
    pub fn resolve_optional(&self, alias: &str) -> Result<Option<RegistryEntry>> {
        match self.registry.resolve(alias) {
            Ok(entry) => return Ok(Some(entry)),
            Err(termtrace_engine::Error::Filesystem { path, source })
                if source.kind() == StdNotFound
                    && path
                        == self
                            .state_root()
                            .join("registry")
                            .join(format!("{alias}.json")) =>
            {
                SessionRegistry::validate_owner_directory(&self.state_root().join("registry"))?;

                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        }
    }

    /// Query the complete derived lifecycle for an alias.
    ///
    /// # Errors
    /// Returns an error if resolution fails.
    pub fn resolve_lifecycle(&self, alias: &str) -> Result<RegistrySessionResolution> {
        return self.registry.resolve_lifecycle(alias).map_err(Error::from);
    }

    /// List registered sessions and their derived lifecycle.
    ///
    /// # Errors
    /// Returns an error if the registry directory cannot be read.
    pub fn list(&self, include_all: bool) -> Result<Vec<RegistryListEntry>> {
        return self.registry.list(include_all).map_err(Error::from);
    }

    /// Remove a friendly name binding while preserving all canonical evidence.
    ///
    /// # Errors
    /// Returns an error if the alias does not exist or removal fails.
    pub fn remove(&self, alias: &str) -> Result<RegistryEntry> {
        return self.registry.remove(alias).map_err(Error::from);
    }

    /// Plan or execute garbage collection over completed unregistered sessions.
    ///
    /// # Errors
    /// Returns an error if state inspection or deletion fails.
    pub fn gc(
        &self,
        older_than_days: u64,
        keep_latest: u64,
        max_completed_bytes: Option<u64>,
        apply: bool,
    ) -> Result<GarbageCollectionReport> {
        if let Some(quota) = max_completed_bytes {
            return self
                .registry
                .garbage_collect_with_quota(older_than_days, keep_latest, Some(quota), apply)
                .map_err(Error::from);
        }
        return self
            .registry
            .garbage_collect(older_than_days, keep_latest, apply)
            .map_err(Error::from);
    }
}
