use std::{fs, path::Path};

use termtrace_model::{
    execution::receipt::{
        EVIDENCE_RECEIPT_FORMAT, EVIDENCE_RECEIPT_NAME, EVIDENCE_RECEIPT_VERSION,
        EvidenceReceiptPayload,
    },
    execution::request_journal::MUTATION_JOURNAL_NAME,
    execution::session::{
        MutationJournalStatus, SessionCompletion, VERIFICATION_REPORT_FORMAT,
        VERIFICATION_REPORT_VERSION, VerificationFinding, VerificationLevel, VerificationReport,
    },
};

use crate::{
    auxiliary::{
        receipt::{read_receipt, verify_ssh_receipt},
        request_journal::validate_mutation_journal,
    },
    storage::{
        Error,
        artifact::{ARTIFACT_ROOT_LOG, read_artifact_roots, verify_session_artifacts},
        event_log::EventLog,
    },
};

use super::{
    io::{manifest_hash, read_completion, read_manifest},
    validation::{validate_completion_format, validate_completion_with_loaded, validate_manifest},
};

/// Verify a session at the requested integrity level without a signer trust policy.
pub fn verify_session(
    session_directory: impl AsRef<Path>,
    level: VerificationLevel,
) -> VerificationReport {
    return verify_session_with_trust(session_directory, level, None);
}

/// Verify a session at the requested integrity level and optional allowed-signers policy.
///
/// Expected evidence failures are returned as findings in the report.
pub fn verify_session_with_trust(
    session_directory: impl AsRef<Path>,
    level: VerificationLevel,
    allowed_signers: Option<&Path>,
) -> VerificationReport {
    let session_directory = session_directory.as_ref();
    let mut report = VerificationReport {
        format: VERIFICATION_REPORT_FORMAT.to_owned(),
        version: VERIFICATION_REPORT_VERSION,
        session: session_directory.to_path_buf(),
        level,
        valid: false,
        session_id: None,
        session_manifest_hash: None,
        final_event_hash: None,
        artifact_roots_verified: 0,
        artifact_authenticity_established: false,
        session_authenticity_established: false,
        mutation_journal_status: MutationJournalStatus::Missing,
        mutation_journal_records_verified: 0,
        mutation_journal_error: None,
        signature_receipt_present: false,
        signature_signer_key_id: None,
        signature_cryptographic_valid: false,
        signature_signer_trusted: false,
        findings: Vec::new(),
        output_page: None,
    };
    let manifest_path = session_directory.join("session.json");
    let manifest_bytes = match fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            finding(
                &mut report,
                "session_manifest_unreadable",
                error.to_string(),
            );
            return report;
        }
    };
    let actual_manifest_hash = manifest_hash(&manifest_bytes);
    report.session_manifest_hash = Some(actual_manifest_hash.clone());
    let manifest = match read_manifest(session_directory) {
        Ok(manifest) => manifest,
        Err(error) => {
            finding(&mut report, "session_manifest_invalid", error.to_string());
            return report;
        }
    };
    report.session_id = Some(manifest.session_id);
    if let Err(error) = validate_manifest(&manifest) {
        finding(&mut report, "session_format_unsupported", error.to_string());
        return report;
    }
    let log = match EventLog::read(session_directory.join(&manifest.event_log)) {
        Ok(log) => log,
        Err(error) => {
            finding(&mut report, "event_log_invalid", error.to_string());
            return report;
        }
    };
    report.final_event_hash = log.events.last().map(|record| return record.hash.clone());
    let completion = match read_completion(session_directory) {
        Ok(completion) => completion,
        Err(error) => {
            finding(
                &mut report,
                "completion_invalid_or_missing",
                error.to_string(),
            );
            return report;
        }
    };
    if let Err(error) = validate_completion_format(&completion) {
        finding(
            &mut report,
            "completion_format_unsupported",
            error.to_string(),
        );
        return report;
    }
    if level == VerificationLevel::Structure {
        report.valid = true;
        return report;
    }
    if let Err(error) = validate_completion_with_loaded(
        session_directory,
        &log,
        &manifest,
        &manifest_bytes,
        &completion,
    ) {
        let code = content_finding_code(&error);
        finding(&mut report, code, error.to_string());
        return report;
    }
    match verify_session_artifacts(
        session_directory,
        &actual_manifest_hash,
        report.final_event_hash.as_deref(),
    ) {
        Ok(count) => report.artifact_roots_verified = count,
        Err(error) => {
            finding(&mut report, "artifact_integrity_invalid", error.to_string());
            return report;
        }
    }
    if session_directory.join(MUTATION_JOURNAL_NAME).exists() {
        match validate_mutation_journal(session_directory, &log.header, &log.events) {
            Ok(count) => {
                report.mutation_journal_status = MutationJournalStatus::Verified;
                report.mutation_journal_records_verified = count;
            }
            Err(error) => {
                report.mutation_journal_status = MutationJournalStatus::Invalid;
                report.mutation_journal_error = Some(error.to_string());
            }
        }
    }
    match level {
        VerificationLevel::Structure
        | VerificationLevel::Content
        | VerificationLevel::DurabilityRecord => report.valid = true,
        VerificationLevel::Signature => {
            verify_signature(session_directory, allowed_signers, &completion, &mut report)
        }
    }
    return report;
}

fn verify_signature(
    session: &Path,
    allowed_signers: Option<&Path>,
    completion: &SessionCompletion,
    report: &mut VerificationReport,
) {
    let receipt_path = session.join(EVIDENCE_RECEIPT_NAME);
    if !receipt_path.exists() {
        finding(
            report,
            "signature_receipt_missing",
            "no detached evidence receipt is available",
        );
        return;
    }
    report.signature_receipt_present = true;
    let expected = match expected_receipt_payload(session, completion, &receipt_path) {
        Ok(payload) => payload,
        Err(error) => {
            finding(report, "signature_receipt_invalid", error.to_string());
            return;
        }
    };
    match verify_ssh_receipt(&receipt_path, &expected, allowed_signers) {
        Ok(signature) => {
            report.signature_signer_key_id = Some(signature.signer_key_id);
            report.signature_cryptographic_valid = signature.cryptographic_valid;
            report.signature_signer_trusted = signature.signer_trusted;
            report.session_authenticity_established = signature.signer_trusted;
            report.artifact_authenticity_established = signature.signer_trusted
                && signature.artifact_root_count > 0
                && signature.artifact_root_count == report.artifact_roots_verified;
            if !signature.cryptographic_valid {
                finding(
                    report,
                    "signature_invalid",
                    "the detached signature does not match the receipt payload",
                );
            } else if allowed_signers.is_none() {
                finding(
                    report,
                    "signature_trust_policy_missing",
                    "cryptography is valid but no allowed-signers policy was supplied",
                );
            } else if !signature.signer_trusted {
                finding(
                    report,
                    "signature_signer_untrusted",
                    "the allowed-signers policy does not trust this signer identity/key",
                );
            } else {
                report.valid = true;
            }
        }
        Err(error) => finding(report, "signature_receipt_invalid", error.to_string()),
    }
}

fn expected_receipt_payload(
    session: &Path,
    completion: &SessionCompletion,
    receipt_path: &Path,
) -> Result<EvidenceReceiptPayload, Error> {
    let receipt = read_receipt(receipt_path).map_err(|error| {
        return Error::InvalidEvidence {
            reason: error.to_string(),
        };
    })?;
    let roots = read_artifact_roots(&session.join(ARTIFACT_ROOT_LOG))?;
    let count = receipt.payload.artifact_root_count;
    if count > u64::try_from(roots.len()).unwrap_or(u64::MAX) {
        return Err(Error::InvalidEvidence {
            reason: "artifact checkpoint exceeds the current root catalog".to_owned(),
        });
    }
    let artifact_root_hash = if count == 0 {
        None
    } else {
        let index = usize::try_from(count - 1).map_err(|_| {
            return Error::InvalidEvidence {
                reason: "artifact checkpoint is not addressable".to_owned(),
            };
        })?;
        Some(
            roots
                .get(index)
                .ok_or_else(|| {
                    return Error::InvalidEvidence {
                        reason: "artifact checkpoint record is missing".to_owned(),
                    };
                })?
                .hash
                .clone(),
        )
    };
    let completion_path = session.join("completion.json");
    let completion_bytes = fs::read(&completion_path).map_err(|source| {
        return Error::Filesystem {
            path: completion_path,
            source,
        };
    })?;
    return Ok(EvidenceReceiptPayload {
        domain: EVIDENCE_RECEIPT_FORMAT.to_owned(),
        version: EVIDENCE_RECEIPT_VERSION,
        session_id: completion.session_id,
        session_manifest_hash: completion.session_manifest_hash.clone(),
        final_event_hash: completion.final_event_hash.clone(),
        event_count: completion.event_count,
        completion_hash: blake3::hash(&completion_bytes).to_hex().to_string(),
        artifact_root_count: count,
        artifact_root_hash,
        signer_key_id: receipt.payload.signer_key_id,
        created_unix_ns: receipt.payload.created_unix_ns,
        creation_time_source: "signing_host_system_clock".to_owned(),
        canonicalization: "domain-prefix-plus-serde-json-struct-v1".to_owned(),
    });
}

fn finding(report: &mut VerificationReport, code: &str, message: impl Into<String>) {
    report.findings.push(VerificationFinding {
        code: code.to_owned(),
        message: message.into(),
    });
}
fn content_finding_code(error: &Error) -> &'static str {
    let Error::InvalidEvidence { reason } = error else {
        return "canonical_content_invalid";
    };
    if reason.contains("session manifest hash") {
        return "session_manifest_hash_mismatch";
    } else if reason.contains("event header hash") {
        return "event_header_hash_mismatch";
    } else if reason.contains("does not end with an Exit") || reason.contains("no terminal Exit") {
        return "terminal_exit_missing";
    } else if reason.contains("pending mutation") {
        return "pending_mutation_intents";
    } else if reason.contains("event count") {
        return "event_count_mismatch";
    } else if reason.contains("output byte count") {
        return "output_count_mismatch";
    } else if reason.contains("final hash") {
        return "final_event_hash_mismatch";
    } else if reason.contains("exit outcome") {
        return "exit_outcome_mismatch";
    } else if reason.contains("durability") || reason.contains("synchronization") {
        return "durability_record_invalid";
    } else {
        return "canonical_content_invalid";
    }
}
