use super::*;

#[derive(Debug, Args)]
pub(super) struct TestArgs {
    /// One or more strict versioned scenario JSON documents.
    #[arg(num_args = 1..)]
    pub(super) scenario: Vec<PathBuf>,
    /// Owner-only result, failure, and canonical evidence root.
    #[arg(long, default_value = "termtrace-results")]
    pub(super) artifacts: PathBuf,
    /// Explicitly replace normalized semantic frame baselines.
    #[arg(long)]
    pub(super) update_snapshots: bool,
    /// New JUnit XML result path.
    #[arg(long)]
    pub(super) junit: Option<PathBuf>,
    /// Maximum concurrent fresh-session workers.
    #[arg(long, default_value_t = 1, value_parser = parse_scenario_jobs)]
    pub(super) jobs: usize,
    /// Zero-based deterministic shard index.
    #[arg(long, requires = "shard_count")]
    pub(super) shard_index: Option<usize>,
    /// Total deterministic shard count.
    #[arg(long, requires = "shard_index", value_parser = parse_positive_usize)]
    pub(super) shard_count: Option<usize>,
    /// Fresh-session reruns after a failed attempt.
    #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=10))]
    pub(super) reruns: u8,
}
