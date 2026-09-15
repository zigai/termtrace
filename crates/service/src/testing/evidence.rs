use super::snapshot::{open_regular_file_nofollow, testing_error};
use super::*;

pub(super) fn capture_failure_artifacts(
    session: &Session,
    attempt_directory: &Path,
    verified_artifacts: &mut Vec<PathBuf>,
    secondary_failures: &mut Vec<SecondaryFailure>,
) {
    let screenshot = attempt_directory.join("semantic-screenshot");
    match session
        .evidence()
        .semantic_screenshot(&screenshot, StatePoint::LatestPresented)
    {
        Ok(outcome) => verified_artifacts.push(outcome.directory),
        Err(error) => {
            secondary_failures.push(bounded_secondary("semantic_screenshot", &error.to_string()))
        }
    }

    let bundle = attempt_directory.join("debug-bundle");

    match session.evidence().bundle(&bundle, BundleProfile::Debug) {
        Ok(_) => verified_artifacts.push(bundle),
        Err(error) => {
            secondary_failures.push(bounded_secondary("debug_bundle", &error.to_string()))
        }
    }
}

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TestEvidenceManifest {
    format: String,
    version: u32,
    session_id: Uuid,
    canonical_session: String,
    outcome: TestOutcome,
    artifacts: Vec<String>,
}

#[allow(
    clippy::verbose_file_reads,
    reason = "the manifest must be opened with no-follow semantics before it is reread"
)]
pub(super) fn write_evidence_manifest(
    attempt_directory: &Path,
    session_id: Uuid,
    canonical_session: &Path,
    report: &FinalizationReport,
) -> Result<PathBuf> {
    let relative_session = relative_evidence_path(attempt_directory, canonical_session)?;
    let artifacts = report
        .verified_artifacts
        .iter()
        .map(|path| return relative_evidence_path(attempt_directory, path))
        .collect::<Result<Vec<String>>>()?;

    let manifest = TestEvidenceManifest {
        format: "termtrace-rust-test-evidence-v1".to_owned(),
        version: 1,
        session_id,
        canonical_session: relative_session,
        outcome: report.outcome,
        artifacts,
    };

    let path = attempt_directory.join("termtrace-test-evidence.json");
    let mut bytes = serde_json::to_vec_pretty(&manifest).map_err(|source| {
        return Error::InvalidJson {
            path: path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    bytes.push(b'\n');
    write_secure(&path, &bytes)?;

    let (mut file, byte_count) = open_regular_file_nofollow(&path)?;

    if byte_count != u64::try_from(bytes.len()).unwrap_or(u64::MAX) {
        return Err(testing_error("test evidence manifest byte count changed"));
    }

    let mut observed = Vec::new();
    file.read_to_end(&mut observed).map_err(|source| {
        return Error::Filesystem {
            path: path.clone(),
            source,
        };
    })?;
    let decoded: TestEvidenceManifest = serde_json::from_slice(&observed).map_err(|source| {
        return Error::InvalidJson {
            path: path.clone(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    if observed != bytes || decoded != manifest {
        return Err(testing_error(
            "test evidence manifest failed post-write verification",
        ));
    }
    return Ok(path);
}

fn relative_evidence_path(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root).map_err(|_| {
        return testing_error("test artifact escaped its attempt evidence directory");
    })?;

    if relative.as_os_str().is_empty() {
        return Err(testing_error(
            "test artifact path resolved to the attempt root",
        ));
    }
    return relative
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| return testing_error("test artifact path is not valid UTF-8"));
}

pub(super) fn bounded_secondary(phase: &'static str, message: &str) -> SecondaryFailure {
    let end = message.floor_char_boundary(MAX_SECONDARY_FAILURE_CHARS);

    return SecondaryFailure {
        phase,
        message: message.get(..end).unwrap_or_default().to_owned(),
        truncated: end != message.len(),
    };
}
