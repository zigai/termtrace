//! Detached, externally signed evidence-root receipts.

use std::{
    env,
    ffi::OsString,
    fs::{OpenOptions, Permissions},
    io::Write as _,
    os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{ARTIFACT_ROOT_LOG, read_artifact_roots, write_secure},
    event::ByteString,
    session::{VerificationLevel, read_completion, read_manifest, verify_session},
};

/// Detached receipt relative path.
pub const EVIDENCE_RECEIPT_NAME: &str = "evidence-receipt.json";
/// Receipt envelope format.
pub const EVIDENCE_RECEIPT_FORMAT: &str = "termtrace-evidence-receipt";
/// Receipt schema version.
pub const EVIDENCE_RECEIPT_VERSION: u32 = 1;
const SIGNATURE_NAMESPACE: &str = "termtrace-evidence";
const SIGNED_DOMAIN: &[u8] = b"TERMTRACE-EVIDENCE-RECEIPT-V1\0";

/// Canonical unsigned roots covered by a detached receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceReceiptPayload {
    /// Domain identifier included in the signed object.
    pub domain: String,
    /// Payload schema version.
    pub version: u32,
    /// Immutable session identifier.
    pub session_id: Uuid,
    /// Exact immutable session-manifest BLAKE3.
    pub session_manifest_hash: String,
    /// Final canonical event BLAKE3.
    pub final_event_hash: String,
    /// Number of canonical events.
    pub event_count: u64,
    /// Exact completion-document BLAKE3.
    pub completion_hash: String,
    /// Number of artifact-root records covered by this checkpoint.
    pub artifact_root_count: u64,
    /// Artifact-root record hash at the checkpoint, when non-empty.
    pub artifact_root_hash: Option<String>,
    /// Signer identity interpreted through the caller's trust policy.
    pub signer_key_id: String,
    /// Unix wall-clock time supplied by the signing host.
    pub created_unix_ns: u128,
    /// Explicit source of the creation time.
    pub creation_time_source: String,
    /// Pinned canonical payload encoding.
    pub canonicalization: String,
}

/// Detached signature metadata and bytes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceSignature {
    /// External signature scheme.
    pub scheme: String,
    /// Namespace supplied to the external signature implementation.
    pub namespace: String,
    /// Signer identity interpreted through an allowed-signers policy.
    pub key_id: String,
    /// Exact detached signature bytes.
    pub value: ByteString,
}

/// Detached evidence receipt envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EvidenceReceipt {
    /// Stable envelope format identifier.
    pub format: String,
    /// Envelope schema version.
    pub version: u32,
    /// Canonical unsigned payload.
    pub payload: EvidenceReceiptPayload,
    /// Detached signature excluded from the signed payload.
    pub signature: EvidenceSignature,
}

/// Cryptographic and trust-policy results kept deliberately separate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReceiptVerification {
    /// Signer identity asserted by the receipt.
    pub signer_key_id: String,
    /// Whether the detached signature is valid for the exact payload bytes.
    pub cryptographic_valid: bool,
    /// Whether a configured allowed-signers policy accepts that identity/key.
    pub signer_trusted: bool,
    /// Artifact-root prefix covered by the signed checkpoint.
    pub artifact_root_count: u64,
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
        return Err(receipt_error(
            "sign evidence receipt",
            "canonical content is invalid",
        ));
    }
    if signer_key_id.is_empty() || signer_key_id.contains(['\n', '\r']) {
        return Err(receipt_error(
            "sign evidence receipt",
            "signer key ID must be non-empty and single-line",
        ));
    }
    let payload = build_payload(session, signer_key_id, None, None)?;
    let signed_bytes = signed_payload_bytes(&payload)?;
    let temporary = session.join(format!(".receipt-sign-{}", Uuid::now_v7()));
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
            return Err(receipt_error("launch OpenSSH signer", error.to_string()));
        }
    };
    let signature = fs_err::read(&signature_path).map_err(|source| {
        cleanup_temporary(&temporary, &signature_path);
        if output.status.success() {
            return Error::Filesystem {
                path: signature_path.clone(),
                source,
            };
        }
        return receipt_error(
            "sign evidence receipt",
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        );
    })?;
    cleanup_temporary(&temporary, &signature_path);
    if !output.status.success() {
        return Err(receipt_error(
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
            key_id: signer_key_id.to_owned(),
            value: ByteString::from(signature),
        },
        payload,
    };
    let bytes = serde_json::to_vec_pretty(&receipt)
        .map_err(|source| return json_error(session.join(EVIDENCE_RECEIPT_NAME), source))?;
    let path = session.join(EVIDENCE_RECEIPT_NAME);
    write_secure(&path, &bytes)?;
    return Ok(path);
}

/// Verify receipt roots, detached signature, and optional signer trust policy.
///
/// # Errors
///
/// Returns an error when the receipt is malformed, references different roots,
/// or external signature verification cannot be executed.
pub fn verify_ssh_receipt(
    session: &Path,
    allowed_signers: Option<&Path>,
) -> Result<ReceiptVerification> {
    let path = session.join(EVIDENCE_RECEIPT_NAME);
    let bytes = fs_err::read(&path).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?;
    let receipt: EvidenceReceipt =
        serde_json::from_slice(&bytes).map_err(|source| return json_error(path.clone(), source))?;
    validate_envelope(&receipt)?;
    let expected = build_payload(
        session,
        &receipt.payload.signer_key_id,
        Some(receipt.payload.artifact_root_count),
        Some(receipt.payload.created_unix_ns),
    )?;
    if receipt.payload != expected {
        return Err(receipt_error(
            "verify evidence receipt roots",
            "receipt payload does not match current canonical roots or artifact checkpoint",
        ));
    }
    let signed_bytes = signed_payload_bytes(&receipt.payload)?;
    let temporary_directory = VerificationDirectory::create()?;
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

fn build_payload(
    session: &Path,
    signer_key_id: &str,
    checkpoint_count: Option<u64>,
    created_unix_ns: Option<u128>,
) -> Result<EvidenceReceiptPayload> {
    let manifest = read_manifest(session)?;
    let completion = read_completion(session)?;
    let completion_bytes = fs_err::read(session.join("completion.json")).map_err(|source| {
        return Error::Filesystem {
            path: session.join("completion.json"),
            source,
        };
    })?;
    let roots = read_artifact_roots(&session.join(ARTIFACT_ROOT_LOG))?;
    let available_count = u64::try_from(roots.len()).unwrap_or(u64::MAX);
    let artifact_root_count = checkpoint_count.unwrap_or(available_count);
    if artifact_root_count > available_count {
        return Err(receipt_error(
            "verify evidence receipt roots",
            "artifact checkpoint exceeds the current root catalog",
        ));
    }
    let artifact_root_hash = if artifact_root_count == 0 {
        None
    } else {
        let index = usize::try_from(artifact_root_count - 1).map_err(|_| {
            return receipt_error(
                "verify evidence receipt roots",
                "artifact checkpoint is not addressable",
            );
        })?;
        Some(
            roots
                .get(index)
                .ok_or_else(|| {
                    return receipt_error(
                        "verify evidence receipt roots",
                        "artifact checkpoint record is missing",
                    );
                })?
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

fn validate_envelope(receipt: &EvidenceReceipt) -> Result<()> {
    if receipt.format != EVIDENCE_RECEIPT_FORMAT
        || receipt.version != EVIDENCE_RECEIPT_VERSION
        || receipt.payload.domain != EVIDENCE_RECEIPT_FORMAT
        || receipt.payload.version != EVIDENCE_RECEIPT_VERSION
        || receipt.signature.scheme != "openssh"
        || receipt.signature.namespace != SIGNATURE_NAMESPACE
        || receipt.signature.key_id != receipt.payload.signer_key_id
        || receipt.payload.creation_time_source != "signing_host_system_clock"
        || receipt.payload.canonicalization != "domain-prefix-plus-serde-json-struct-v1"
    {
        return Err(receipt_error(
            "verify evidence receipt",
            "unsupported receipt format, signature scheme, or canonicalization",
        ));
    }
    return Ok(());
}

fn signed_payload_bytes(payload: &EvidenceReceiptPayload) -> Result<Vec<u8>> {
    let mut bytes = SIGNED_DOMAIN.to_vec();
    bytes.extend(serde_json::to_vec(payload).map_err(|source| {
        return json_error(PathBuf::from("<evidence-receipt-payload>"), source);
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
        .map_err(|error| return receipt_error("launch OpenSSH verifier", error.to_string()))?;
    child
        .stdin
        .take()
        .ok_or_else(|| return receipt_error("verify evidence receipt", "verifier stdin missing"))?
        .write_all(payload)
        .map_err(|error| return receipt_error("write verifier payload", error.to_string()))?;
    let status = child
        .wait()
        .map_err(|error| return receipt_error("wait for OpenSSH verifier", error.to_string()))?;
    return Ok(status.success());
}

struct VerificationDirectory {
    path: PathBuf,
}

impl VerificationDirectory {
    fn create() -> Result<Self> {
        let path = env_temp_directory().join(format!("termtrace-receipt-{}", Uuid::now_v7()));
        fs_err::create_dir(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;
        if let Err(source) = fs_err::set_permissions(&path, Permissions::from_mode(0o700)) {
            let _ = fs_err::remove_dir(&path);
            return Err(Error::Filesystem { path, source });
        }
        return Ok(Self { path });
    }

    fn path(&self) -> &Path {
        return &self.path;
    }
}

impl Drop for VerificationDirectory {
    fn drop(&mut self) {
        let _ = fs_err::remove_dir_all(&self.path);
    }
}

fn env_temp_directory() -> PathBuf {
    return env::temp_dir();
}

fn write_temporary(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    file.write_all(bytes)
        .and_then(|()| return file.sync_all())
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    return Ok(());
}

fn cleanup_temporary(payload: &Path, signature: &Path) {
    let _ = fs_err::remove_file(payload);
    let _ = fs_err::remove_file(signature);
}

fn receipt_error(operation: &'static str, reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation,
        reason: reason.into(),
    };
}

fn json_error(path: PathBuf, source: serde_json::Error) -> Error {
    return Error::InvalidJson {
        path,
        json_path: "$".to_owned(),
        source,
    };
}
