use super::*;

/// Verify one completed session without mutating its canonical evidence.
pub fn verify_session(
    session_directory: impl AsRef<Path>,
    level: VerificationLevel,
) -> VerificationReport {
    return verify_session_with_trust(session_directory, level, None);
}

/// Verify a session with an optional OpenSSH allowed-signers trust policy.
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
    let manifest_bytes = match fs_err::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            report.findings.push(VerificationFinding {
                code: "session_manifest_unreadable".to_owned(),
                message: error.to_string(),
            });
            return report;
        }
    };
    let actual_manifest_hash = manifest_hash(&manifest_bytes);
    report.session_manifest_hash = Some(actual_manifest_hash.clone());
    let manifest = match read_manifest(session_directory) {
        Ok(manifest) => manifest,
        Err(error) => {
            report.findings.push(VerificationFinding {
                code: "session_manifest_invalid".to_owned(),
                message: error.to_string(),
            });
            return report;
        }
    };
    report.session_id = Some(manifest.session_id);
    if let Err(error) = validate_manifest(&manifest) {
        report.findings.push(VerificationFinding {
            code: "session_format_unsupported".to_owned(),
            message: error.to_string(),
        });
        return report;
    }
    let event_path = session_directory.join(&manifest.event_log);
    let log = match EventLog::read(event_path) {
        Ok(log) => log,
        Err(error) => {
            report.findings.push(VerificationFinding {
                code: "event_log_invalid".to_owned(),
                message: error.to_string(),
            });
            return report;
        }
    };
    report.final_event_hash = log.events.last().map(|record| return record.hash.clone());
    let completion = match read_completion(session_directory) {
        Ok(completion) => completion,
        Err(error) => {
            report.findings.push(VerificationFinding {
                code: "completion_invalid_or_missing".to_owned(),
                message: error.to_string(),
            });
            return report;
        }
    };
    if let Err(error) = validate_completion_format(&completion) {
        report.findings.push(VerificationFinding {
            code: "completion_format_unsupported".to_owned(),
            message: error.to_string(),
        });
        return report;
    }
    if level == VerificationLevel::Structure {
        report.valid = true;
        return report;
    }
    if let Err(error) = validate_completed_log(session_directory, &log) {
        report.findings.push(VerificationFinding {
            code: content_finding_code(&error).to_owned(),
            message: error.to_string(),
        });
        return report;
    }
    match verify_session_artifacts(
        session_directory,
        &actual_manifest_hash,
        report.final_event_hash.as_deref(),
    ) {
        Ok(count) => report.artifact_roots_verified = count,
        Err(error) => {
            report.findings.push(VerificationFinding {
                code: "artifact_integrity_invalid".to_owned(),
                message: error.to_string(),
            });
            return report;
        }
    }
    let journal_path = session_directory.join(MUTATION_JOURNAL_NAME);
    if journal_path.exists() {
        match validate_mutation_journal(session_directory, &log) {
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
        VerificationLevel::Structure | VerificationLevel::Content => report.valid = true,
        VerificationLevel::DurabilityRecord => report.valid = true,
        VerificationLevel::Signature => {
            let receipt_path = session_directory.join(EVIDENCE_RECEIPT_NAME);
            if !receipt_path.exists() {
                report.findings.push(VerificationFinding {
                    code: "signature_receipt_missing".to_owned(),
                    message: "no detached evidence receipt is available".to_owned(),
                });
                return report;
            }
            report.signature_receipt_present = true;
            match verify_ssh_receipt(session_directory, allowed_signers) {
                Ok(signature) => {
                    report.signature_signer_key_id = Some(signature.signer_key_id);
                    report.signature_cryptographic_valid = signature.cryptographic_valid;
                    report.signature_signer_trusted = signature.signer_trusted;
                    report.session_authenticity_established = signature.signer_trusted;
                    report.artifact_authenticity_established = signature.signer_trusted
                        && signature.artifact_root_count > 0
                        && signature.artifact_root_count == report.artifact_roots_verified;
                    if !signature.cryptographic_valid {
                        report.findings.push(VerificationFinding {
                            code: "signature_invalid".to_owned(),
                            message: "the detached signature does not match the receipt payload"
                                .to_owned(),
                        });
                    } else if allowed_signers.is_none() {
                        report.findings.push(VerificationFinding {
                            code: "signature_trust_policy_missing".to_owned(),
                            message:
                                "cryptography is valid but no allowed-signers policy was supplied"
                                    .to_owned(),
                        });
                    } else if !signature.signer_trusted {
                        report.findings.push(VerificationFinding {
                            code: "signature_signer_untrusted".to_owned(),
                            message:
                                "the allowed-signers policy does not trust this signer identity/key"
                                    .to_owned(),
                        });
                    } else {
                        report.valid = true;
                    }
                }
                Err(error) => report.findings.push(VerificationFinding {
                    code: "signature_receipt_invalid".to_owned(),
                    message: error.to_string(),
                }),
            }
        }
    }
    return report;
}

fn content_finding_code(error: &Error) -> &'static str {
    let Error::InvalidReplay { reason } = error else {
        return "canonical_content_invalid";
    };
    if reason.contains("session manifest hash") {
        return "session_manifest_hash_mismatch";
    }
    if reason.contains("event header hash") {
        return "event_header_hash_mismatch";
    }
    if reason.contains("does not end with an Exit") || reason.contains("no terminal Exit") {
        return "terminal_exit_missing";
    }
    if reason.contains("pending mutation") {
        return "pending_mutation_intents";
    }
    if reason.contains("event count") {
        return "event_count_mismatch";
    }
    if reason.contains("output byte count") {
        return "output_count_mismatch";
    }
    if reason.contains("final hash") {
        return "final_event_hash_mismatch";
    }
    if reason.contains("exit outcome") {
        return "exit_outcome_mismatch";
    }
    if reason.contains("durability") || reason.contains("synchronization") {
        return "durability_record_invalid";
    }
    return "canonical_content_invalid";
}
