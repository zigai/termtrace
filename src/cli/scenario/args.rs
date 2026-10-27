use crate::cli::parsers::{parse_positive_usize, parse_scenario_jobs};
use clap::Args;
use std::path::PathBuf;

#[derive(Debug, Args)]
pub(in crate::cli) struct TestArgs {
    /// One or more strict versioned scenario JSON documents.
    #[arg(num_args = 1..)]
    pub(in crate::cli) scenario: Vec<PathBuf>,
    /// Owner-only result, failure, and canonical evidence root.
    #[arg(long, default_value = "termtrace-results")]
    pub(in crate::cli) artifacts: PathBuf,
    /// Explicitly replace normalized semantic frame baselines.
    #[arg(long)]
    pub(in crate::cli) update_snapshots: bool,
    /// New JUnit XML result path.
    #[arg(long)]
    pub(in crate::cli) junit: Option<PathBuf>,
    /// Maximum concurrent fresh-session workers (1-64).
    #[arg(long, default_value_t = 1, value_parser = parse_scenario_jobs)]
    pub(in crate::cli) jobs: usize,
    /// Zero-based deterministic shard index.
    #[arg(long, requires = "shard_count")]
    pub(in crate::cli) shard_index: Option<usize>,
    /// Total deterministic shard count.
    #[arg(long, requires = "shard_index", value_parser = parse_positive_usize)]
    pub(in crate::cli) shard_count: Option<usize>,
    /// Reject recovered flaky cases in CI (default: tolerate; verdict stays flaky).
    #[arg(long)]
    pub(in crate::cli) fail_on_flaky: bool,
    /// Fresh-session reruns after expectation failures, never infrastructure errors (0-10).
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=10))]
    pub(in crate::cli) reruns: u8,
}
