//! The one definition of a session directory's file layout.
//!
//! Every crate names session files through this module. A file's [`FileRole`]
//! says whether it is canonical evidence, auxiliary diagnostics, or a derived
//! read model that may be discarded and rebuilt.

use std::path::{Path, PathBuf};

/// Immutable session manifest.
pub const MANIFEST: &str = "session.json";
/// Hash-chained canonical event log.
pub const EVENT_LOG: &str = "events.jsonl";
/// Completion record published after the final event is durable.
pub const COMPLETION: &str = "completion.json";
/// Durable mutation-request journal.
pub const MUTATION_JOURNAL: &str = "requests.jsonl";
/// Per-recording Ghostty replay profile.
pub const GHOSTTY_REPLAY_PROFILE: &str = "ghostty-profile.ghostty";
/// Diagnostic written when a recording could not publish completion.
pub const FAILURE_DIAGNOSTIC: &str = "failure.json";
/// Diagnostic written by explicit recovery of an incomplete session.
pub const RECOVERY_DIAGNOSTIC: &str = "recovery.json";
/// Detached SSH evidence receipt.
pub const EVIDENCE_RECEIPT: &str = "evidence-receipt.json";
/// Append-only catalog of committed derived-artifact roots.
pub const ARTIFACT_ROOT_LOG: &str = "artifact-roots.jsonl";
/// Lock serializing appends to [`ARTIFACT_ROOT_LOG`].
pub const ARTIFACT_ROOT_LOCK: &str = ".artifact-roots.lock";
/// Directory of replay-derived, verifiable artifacts.
pub const DERIVED: &str = "derived";
/// Directory of disposable semantic-state caches.
pub const DERIVED_CACHE: &str = "derived-cache";

/// What a top-level session file is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileRole {
    /// Recorder-written evidence that defines the session.
    Canonical,
    /// Human-facing diagnostics and receipts that do not define the session.
    Auxiliary,
    /// Rebuildable read models; never authoritative.
    Derived,
    /// Coordination state such as locks.
    Internal,
}

/// Classify a top-level session entry by name, or `None` when it is not part of the layout.
pub fn file_role(name: &str) -> Option<FileRole> {
    return match name {
        MANIFEST | EVENT_LOG | COMPLETION | MUTATION_JOURNAL | GHOSTTY_REPLAY_PROFILE => {
            Some(FileRole::Canonical)
        }
        FAILURE_DIAGNOSTIC | RECOVERY_DIAGNOSTIC | EVIDENCE_RECEIPT => Some(FileRole::Auxiliary),
        ARTIFACT_ROOT_LOG | DERIVED | DERIVED_CACHE => Some(FileRole::Derived),
        ARTIFACT_ROOT_LOCK => Some(FileRole::Internal),
        _ => None,
    };
}

/// Paths of the files in one session directory.
#[derive(Clone, Copy, Debug)]
pub struct SessionLayout<'root> {
    root: &'root Path,
}

impl<'root> SessionLayout<'root> {
    /// Describe the session rooted at `root`.
    pub const fn new(root: &'root Path) -> Self {
        return Self { root };
    }

    /// The session directory.
    pub const fn root(self) -> &'root Path {
        return self.root;
    }

    /// Path of [`MANIFEST`].
    pub fn manifest(self) -> PathBuf {
        return self.root.join(MANIFEST);
    }

    /// Path of [`EVENT_LOG`].
    pub fn event_log(self) -> PathBuf {
        return self.root.join(EVENT_LOG);
    }

    /// Path of [`COMPLETION`].
    pub fn completion(self) -> PathBuf {
        return self.root.join(COMPLETION);
    }

    /// Path of [`MUTATION_JOURNAL`].
    pub fn mutation_journal(self) -> PathBuf {
        return self.root.join(MUTATION_JOURNAL);
    }

    /// Path of [`GHOSTTY_REPLAY_PROFILE`].
    pub fn ghostty_replay_profile(self) -> PathBuf {
        return self.root.join(GHOSTTY_REPLAY_PROFILE);
    }

    /// Path of [`FAILURE_DIAGNOSTIC`].
    pub fn failure_diagnostic(self) -> PathBuf {
        return self.root.join(FAILURE_DIAGNOSTIC);
    }

    /// Path of [`RECOVERY_DIAGNOSTIC`].
    pub fn recovery_diagnostic(self) -> PathBuf {
        return self.root.join(RECOVERY_DIAGNOSTIC);
    }

    /// Path of [`EVIDENCE_RECEIPT`].
    pub fn evidence_receipt(self) -> PathBuf {
        return self.root.join(EVIDENCE_RECEIPT);
    }

    /// Path of [`ARTIFACT_ROOT_LOG`].
    pub fn artifact_root_log(self) -> PathBuf {
        return self.root.join(ARTIFACT_ROOT_LOG);
    }

    /// Path of [`ARTIFACT_ROOT_LOCK`].
    pub fn artifact_root_lock(self) -> PathBuf {
        return self.root.join(ARTIFACT_ROOT_LOCK);
    }

    /// Path of [`DERIVED`].
    pub fn derived(self) -> PathBuf {
        return self.root.join(DERIVED);
    }

    /// Path of [`DERIVED_CACHE`].
    pub fn derived_cache(self) -> PathBuf {
        return self.root.join(DERIVED_CACHE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_file_has_a_role() {
        for name in [
            MANIFEST,
            EVENT_LOG,
            COMPLETION,
            MUTATION_JOURNAL,
            GHOSTTY_REPLAY_PROFILE,
            FAILURE_DIAGNOSTIC,
            RECOVERY_DIAGNOSTIC,
            EVIDENCE_RECEIPT,
            ARTIFACT_ROOT_LOG,
            ARTIFACT_ROOT_LOCK,
            DERIVED,
            DERIVED_CACHE,
        ] {
            assert!(file_role(name).is_some(), "{name} has no role");
        }

        assert_eq!(file_role("unexpected.bin"), None);
    }
}
