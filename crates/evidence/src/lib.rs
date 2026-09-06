//! Durable storage, verification, and packaging of Termtrace evidence.
//!
//! This crate owns filesystem-backed evidence operations while its serialized contracts remain
//! in `termtrace-model`.

mod auxiliary;
mod storage;

/// Indexed, bounded access to complete tmux recording and coverage history.
pub mod tmux_catalog;
/// Append-only aggregate tmux lifecycle history.
pub mod tmux_journal;

/// Evidence storage failure.
pub use storage::{Error, Result};

/// Derived-artifact storage and verification.
pub mod artifact {
    pub use crate::storage::artifact::{
        ARTIFACT_ROOT_LOG, BudgetWriter, append_artifact_prefix_root, append_artifact_root,
        collect_artifact_members, commit_directory, create_private_directory, finish_writer,
        hash_regular_file, open_regular_file_nofollow, read_artifact_roots, secure_file,
        secure_writer, sync_directory, verify_artifact_members, verify_session_artifacts,
        write_secure,
    };
}

/// Canonical event-log reading, writing, and recovery.
pub mod event_log {
    pub use crate::storage::event_log::{
        CanonicalBoundary, CanonicalLogFile, DurabilityMode, DurabilityPolicy, DurabilityRecord,
        EventLog, EventLogCursor, EventLogSummary, EventLogWriter, OperationKind, PendingOperation,
        PendingOperationReport, RecoveryIssue, RecoveryReport, SessionClock,
        canonical_source_fingerprint,
    };
}

/// Session manifest, completion, and verification operations.
pub mod session {
    pub use crate::storage::session::{
        SessionSnapshot, canonical_manifest_bytes, manifest_hash, read_completion, read_manifest,
        recover_session, validate_completed_log, validate_completed_session,
        validate_completion_candidate, validate_completion_summary_candidate, verify_session,
        verify_session_with_trust,
    };
}

/// Execution-provenance collection.
pub mod provenance {
    pub use crate::auxiliary::provenance::{
        EffectiveEnvironment, RecorderBuildIdentity, collect_execution_provenance,
    };
}

/// Detached SSH receipt creation and verification.
pub mod receipt {
    pub use crate::auxiliary::receipt::{
        EVIDENCE_RECEIPT_FORMAT, EVIDENCE_RECEIPT_NAME, EVIDENCE_RECEIPT_VERSION,
        create_ssh_receipt, read_receipt, receipt_signing_capability, verify_ssh_receipt,
    };
}

/// Durable mutation-request journal operations.
pub mod request_journal {
    pub use crate::auxiliary::request_journal::{
        MUTATION_JOURNAL_FORMAT, MUTATION_JOURNAL_NAME, MUTATION_JOURNAL_VERSION, MutationJournal,
        MutationJournalCursor, guarded_command_hash, read_mutation_journal,
        validate_mutation_journal,
    };
}

/// Portable evidence-bundle creation and verification.
pub mod bundle {
    pub use crate::auxiliary::bundle::{
        BundleDraft, BundleIdentity, BundleMemberMetadata, BundleMemberWriter, archive,
        hash_regular_file, verify, verify_archive,
    };
}
