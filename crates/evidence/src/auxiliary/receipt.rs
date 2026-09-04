use std::{
    env,
    ffi::{OsStr, OsString},
    fs::{OpenOptions, Permissions},
    io::{ErrorKind, Write as _},
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use termtrace_model::{
    execution::receipt::{
        EvidenceReceipt, EvidenceReceiptPayload, EvidenceSignature, ReceiptVerification,
    },
    execution::session::VerificationLevel,
    protocol::capability::{Capability, CapabilityStatus, RECEIPT_SIGNING_CAPABILITY},
    value::ByteString,
};
use uuid::Uuid;

use crate::storage::{
    Error, Result,
    artifact::{ARTIFACT_ROOT_LOG, read_artifact_roots},
    session::{read_completion, read_manifest, verify_session},
};

pub use termtrace_model::execution::receipt::{
    EVIDENCE_RECEIPT_FORMAT, EVIDENCE_RECEIPT_NAME, EVIDENCE_RECEIPT_VERSION,
};

const SIGNATURE_NAMESPACE: &str = "termtrace-evidence";
const SIGNED_DOMAIN: &[u8] = b"TERMTRACE-EVIDENCE-RECEIPT-V1\0";
const MAX_RECEIPT_BYTES: u64 = 4 * 1024 * 1024;

/// Probe the OpenSSH signing dependency owned by receipt publication.
#[must_use]
pub fn receipt_signing_capability() -> Capability {
    if executable("ssh-keygen").is_some() {
        return Capability {
            id: RECEIPT_SIGNING_CAPABILITY.to_owned(),
            status: CapabilityStatus::Ready,
            code: "ssh_keygen_found".to_owned(),
            detail: "OpenSSH ssh-keygen was found".to_owned(),
            remediation: None,
        };
    }
    return Capability {
        id: RECEIPT_SIGNING_CAPABILITY.to_owned(),
        status: CapabilityStatus::Blocked,
        code: "ssh_keygen_missing".to_owned(),
        detail: "OpenSSH ssh-keygen was not found".to_owned(),
        remediation: Some("install an OpenSSH client package".to_owned()),
    };
}

fn executable(command: impl AsRef<OsStr>) -> Option<PathBuf> {
    let command = command.as_ref();
    let path = env::var_os("PATH")?;
    return env::split_paths(&path)
        .map(|directory| return directory.join(command))
        .find(|candidate| return candidate.is_file());
}

/// Verify canonical content and create a no-replace OpenSSH evidence receipt.
///
/// # Errors
///
/// Returns an error when canonical content is invalid, receipt roots cannot be
/// assembled, the external signer fails, or a receipt already exists.
pub fn create_ssh_receipt(session: &Path, key: &Path, signer_key_id: &str) -> Result<PathBuf> {
    let verification = verify_session(session, VerificationLevel::Content);
    if !verification.valid {
        return Err(Error::invalid("canonical content is invalid"));
    }
    if signer_key_id.is_empty() || signer_key_id.contains(['\n', '\r']) {
        return Err(Error::invalid(
            "signer key ID must be non-empty and single-line",
        ));
    }
    let payload = build_payload(session, signer_key_id, None, None)?;
    let path = session.join(EVIDENCE_RECEIPT_NAME);
    sign_ssh_receipt(&path, payload, key)?;
    return Ok(path);
}

fn build_payload(
    session: &Path,
    signer_key_id: &str,
    checkpoint_count: Option<u64>,
    created_unix_ns: Option<u128>,
) -> Result<EvidenceReceiptPayload> {
    let manifest = read_manifest(session)?;
    let completion = read_completion(session)?;
    let completion_path = session.join("completion.json");
    let completion_bytes = fs_err::read(&completion_path)
        .map_err(|source| return Error::filesystem(completion_path.clone(), source))?;
    let roots = read_artifact_roots(&session.join(ARTIFACT_ROOT_LOG))?;
    let available_count = u64::try_from(roots.len()).unwrap_or(u64::MAX);
    let artifact_root_count = checkpoint_count.unwrap_or(available_count);
    if artifact_root_count > available_count {
        return Err(Error::invalid(
            "artifact checkpoint exceeds the current root catalog",
        ));
    }
    let artifact_root_hash = if artifact_root_count == 0 {
        None
    } else {
        let index = usize::try_from(artifact_root_count - 1)
            .map_err(|_| return Error::invalid("artifact checkpoint is not addressable"))?;
        Some(
            roots
                .get(index)
                .ok_or_else(|| return Error::invalid("artifact checkpoint record is missing"))?
                .hash
                .clone(),
        )
    };
    let created_unix_ns = match created_unix_ns {
        Some(value) => value,
        None => SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| return Error::ClockBeforeEpoch)?
            .as_nanos(),
    };
    return Ok(EvidenceReceiptPayload {
        domain: EVIDENCE_RECEIPT_FORMAT.to_owned(),
        version: EVIDENCE_RECEIPT_VERSION,
        session_id: manifest.session_id,
        session_manifest_hash: completion.session_manifest_hash,
        final_event_hash: completion.final_event_hash,
        event_count: completion.event_count,
        completion_hash: blake3::hash(&completion_bytes).to_hex().to_string(),
        artifact_root_count,
        artifact_root_hash,
        signer_key_id: signer_key_id.to_owned(),
        created_unix_ns,
        creation_time_source: "signing_host_system_clock".to_owned(),
        canonicalization: "domain-prefix-plus-serde-json-struct-v1".to_owned(),
    });
}

fn sign_ssh_receipt(
    receipt_path: &Path,
    payload: EvidenceReceiptPayload,
    key: &Path,
) -> Result<()> {
    validate_payload(&payload)?;
    if receipt_path.exists() {
        return Err(Error::DestinationExists {
            path: receipt_path.to_path_buf(),
        });
    }
    let parent = receipt_path
        .parent()
        .ok_or_else(|| return Error::invalid("receipt path has no parent"))?;
    if !parent.is_dir() {
        return Err(Error::invalid("receipt parent directory does not exist"));
    }
    let signed_bytes = signed_payload_bytes(&payload)?;
    let temporary = parent.join(format!(".receipt-sign-{}", Uuid::now_v7()));
    write_temporary(&temporary, &signed_bytes)?;
    let mut signature_name = OsString::from(temporary.as_os_str());
    signature_name.push(".sig");
    let signature_path = PathBuf::from(signature_name);
    let output = Command::new("ssh-keygen")
        .args(["-Y", "sign", "-f"])
        .arg(key)
        .args(["-n", SIGNATURE_NAMESPACE])
        .arg(&temporary)
        .stdin(Stdio::null())
        .output();
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            cleanup_temporary(&temporary, &signature_path);
            return Err(command_error("launch OpenSSH signer", error.to_string()));
        }
    };
    let signature = fs_err::read(&signature_path).map_err(|source| {
        cleanup_temporary(&temporary, &signature_path);
        if output.status.success() {
            return Error::filesystem(signature_path.clone(), source);
        }
        return command_error(
            "sign evidence receipt",
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        );
    })?;
    cleanup_temporary(&temporary, &signature_path);
    if !output.status.success() {
        return Err(command_error(
            "sign evidence receipt",
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    let receipt = EvidenceReceipt {
        format: EVIDENCE_RECEIPT_FORMAT.to_owned(),
        version: EVIDENCE_RECEIPT_VERSION,
        signature: EvidenceSignature {
            scheme: "openssh".to_owned(),
            namespace: SIGNATURE_NAMESPACE.to_owned(),
            key_id: payload.signer_key_id.clone(),
            value: ByteString::from(signature),
        },
        payload,
    };
    let bytes = serde_json::to_vec_pretty(&receipt).map_err(|source| {
        return Error::InvalidJson {
            path: receipt_path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    write_temporary(receipt_path, &bytes)?;
    sync_parent(receipt_path)?;
    return Ok(());
}

/// Read and validate a bounded evidence receipt.
///
/// # Errors
///
/// Returns an error when the path is not a bounded regular file or the receipt envelope is
/// invalid.
pub fn read_receipt(receipt_path: &Path) -> Result<EvidenceReceipt> {
    let metadata = fs_err::symlink_metadata(receipt_path)
        .map_err(|source| return Error::filesystem(receipt_path, source))?;
    let file_type = metadata.file_type();
    if !file_type.is_file() || metadata.len() > MAX_RECEIPT_BYTES {
        return Err(Error::invalid("receipt is not a bounded regular file"));
    }
    let bytes = fs_err::read(receipt_path)
        .map_err(|source| return Error::filesystem(receipt_path, source))?;
    let receipt = serde_json::from_slice(&bytes).map_err(|source| {
        return Error::InvalidJson {
            path: receipt_path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    validate_envelope(&receipt)?;
    return Ok(receipt);
}

/// Verify an SSH receipt against an expected payload and optional trust policy.
///
/// Cryptographic validity and signer trust are reported independently.
///
/// # Errors
///
/// Returns an error when the receipt is invalid, its payload differs, or OpenSSH verification
/// cannot be completed.
pub fn verify_ssh_receipt(
    receipt_path: &Path,
    expected_payload: &EvidenceReceiptPayload,
    allowed_signers: Option<&Path>,
) -> Result<ReceiptVerification> {
    let receipt = read_receipt(receipt_path)?;
    if receipt.payload != *expected_payload {
        return Err(Error::invalid(
            "receipt payload does not match current canonical roots or artifact checkpoint",
        ));
    }
    let signed_bytes = signed_payload_bytes(&receipt.payload)?;
    let temporary_directory = tempfile::Builder::new()
        .prefix("termtrace-receipt-")
        .permissions(Permissions::from_mode(0o700))
        .tempdir()
        .map_err(|source| Error::Filesystem {
            path: env::temp_dir(),
            source,
        })?;
    let signature_path = temporary_directory.path().join("receipt.sig");
    write_temporary(&signature_path, receipt.signature.value.as_bytes())?;
    let cryptographic_valid = run_ssh_verification(
        "check-novalidate",
        None,
        &receipt.payload.signer_key_id,
        &signature_path,
        &signed_bytes,
    )?;
    let signer_trusted = match allowed_signers {
        Some(policy) if cryptographic_valid => run_ssh_verification(
            "verify",
            Some(policy),
            &receipt.payload.signer_key_id,
            &signature_path,
            &signed_bytes,
        )?,
        Some(_) | None => false,
    };
    return Ok(ReceiptVerification {
        signer_key_id: receipt.payload.signer_key_id,
        cryptographic_valid,
        signer_trusted,
        artifact_root_count: receipt.payload.artifact_root_count,
    });
}

fn validate_payload(payload: &EvidenceReceiptPayload) -> Result<()> {
    if payload.domain != EVIDENCE_RECEIPT_FORMAT
        || payload.version != EVIDENCE_RECEIPT_VERSION
        || payload.signer_key_id.is_empty()
        || payload.signer_key_id.contains(['\n', '\r'])
        || payload.creation_time_source != "signing_host_system_clock"
        || payload.canonicalization != "domain-prefix-plus-serde-json-struct-v1"
    {
        return Err(Error::invalid(
            "unsupported receipt payload or invalid signer key ID",
        ));
    }
    return Ok(());
}

fn validate_envelope(receipt: &EvidenceReceipt) -> Result<()> {
    validate_payload(&receipt.payload)?;
    if receipt.format != EVIDENCE_RECEIPT_FORMAT
        || receipt.version != EVIDENCE_RECEIPT_VERSION
        || receipt.signature.scheme != "openssh"
        || receipt.signature.namespace != SIGNATURE_NAMESPACE
        || receipt.signature.key_id != receipt.payload.signer_key_id
    {
        return Err(Error::invalid(
            "unsupported receipt format, signature scheme, or canonicalization",
        ));
    }
    return Ok(());
}

fn signed_payload_bytes(payload: &EvidenceReceiptPayload) -> Result<Vec<u8>> {
    let mut bytes = SIGNED_DOMAIN.to_vec();
    bytes.extend(serde_json::to_vec(payload).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<evidence-receipt-payload>"),
            json_path: "$".to_owned(),
            source,
        };
    })?);
    return Ok(bytes);
}

fn run_ssh_verification(
    mode: &str,
    allowed_signers: Option<&Path>,
    key_id: &str,
    signature: &Path,
    payload: &[u8],
) -> Result<bool> {
    let mut command = Command::new("ssh-keygen");
    command.args(["-Y", mode]);
    if let Some(policy) = allowed_signers {
        command.args(["-f"]).arg(policy).args(["-I", key_id]);
    }
    let mut child = command
        .args(["-n", SIGNATURE_NAMESPACE, "-s"])
        .arg(signature)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| return command_error("launch OpenSSH verifier", error.to_string()))?;
    let write_result = child
        .stdin
        .take()
        .ok_or_else(|| return Error::invalid("verifier stdin missing"))?
        .write_all(payload);
    let status = child
        .wait()
        .map_err(|error| return command_error("wait for OpenSSH verifier", error.to_string()))?;
    if let Err(error) = write_result {
        if error.kind() == ErrorKind::BrokenPipe && !status.success() {
            return Ok(false);
        }
        return Err(command_error("write verifier payload", error.to_string()));
    }
    return Ok(status.success());
}

fn write_temporary(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| {
            if source.kind() == ErrorKind::AlreadyExists {
                return Error::DestinationExists {
                    path: path.to_path_buf(),
                };
            }
            return Error::filesystem(path, source);
        })?;
    file.write_all(bytes)
        .and_then(|()| return file.sync_all())
        .map_err(|source| return Error::filesystem(path, source))?;
    return Ok(());
}

fn sync_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| return Error::invalid("path has no parent"))?;
    let directory =
        fs_err::File::open(parent).map_err(|source| return Error::filesystem(parent, source))?;
    directory
        .sync_all()
        .map_err(|source| return Error::filesystem(parent, source))?;
    return Ok(());
}

fn cleanup_temporary(payload: &Path, signature: &Path) {
    let _ = fs_err::remove_file(payload);
    let _ = fs_err::remove_file(signature);
}

fn command_error(operation: &'static str, reason: impl Into<String>) -> Error {
    return Error::ExternalCommand {
        operation,
        reason: reason.into(),
    };
}
