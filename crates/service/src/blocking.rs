//! Managed execution context and blocking adapter over shared application services.

use crate::error::{Error, Result};
use std::sync::OnceLock;
use tokio::runtime::{Builder, Handle, Runtime};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Execution context for synchronous SDK callers.
///
/// Ensures all blocking calls share one background thread pool and detects
/// unsupported nested execution inside an existing async executor.
pub struct BlockingContext;

impl BlockingContext {
    /// Execute a future synchronously within the managed runtime context.
    ///
    /// # Errors
    /// Returns `Error::InvalidOperation` if called from inside an existing async runtime.
    pub fn block_on<F, T>(future: F) -> Result<T>
    where
        F: Future<Output = T>,
    {
        // Check if we are already inside a Tokio runtime thread
        if Handle::try_current().is_ok() {
            return Err(Error::InvalidOperation {
                reason: "cannot invoke blocking Termtrace API from inside an async runtime context; use the async API directly"
                    .to_owned(),
            });
        }

        if let Some(runtime) = RUNTIME.get() {
            return Ok(runtime.block_on(future));
        }

        let new_runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("termtrace-blocking-worker")
            .build()
            .map_err(|source| {
                return Error::InvalidOperation {
                    reason: format!("failed to initialize managed blocking runtime: {source}"),
                };
            })?;

        let runtime = RUNTIME.get_or_init(|| return new_runtime);

        return Ok(runtime.block_on(future));
    }
}
