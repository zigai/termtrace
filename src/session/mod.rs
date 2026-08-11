//! Stable session provenance and completion manifests.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{hash_regular_file, verify_session_artifacts},
    environment::ChildEnvironmentPolicy,
    event::{ByteString, Event},
    event_log::{DurabilityPolicy, DurabilityRecord, EventLog},
    pagination::OutputPage,
    provenance::ExecutionProvenance,
    receipt::{EVIDENCE_RECEIPT_NAME, verify_ssh_receipt},
    request_journal::{MUTATION_JOURNAL_NAME, validate_mutation_journal},
};

mod io;
mod model;
mod provenance;
mod validation;
mod verification;

pub use io::*;
pub use model::*;
pub use provenance::*;
pub use validation::validate_completed_log;
pub(crate) use validation::validate_completion_candidate;
use validation::{validate_completion_format, validate_manifest};
pub use verification::{verify_session, verify_session_with_trust};

#[cfg(test)]
mod tests;
