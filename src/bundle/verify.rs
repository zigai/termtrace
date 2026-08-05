use super::*;

struct TrustedBundleSource {
    canonical: PathBuf,
    manifest: SessionManifest,
    completion: SessionCompletion,
    log: EventLog,
}

/// Verify a bundle against caller-owned source and profile expectations.
pub fn verify(directory: &Path, policy: &BundleVerificationPolicy) -> Result<BundleManifest> {
    let trusted = resolve_trusted_source(policy)?;
    let path = directory.join("manifest.json");
    let manifest_bytes = read_bounded_regular_file(&path, MAX_BUNDLE_MANIFEST_BYTES)?;
    let manifest: BundleManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|source| return json_error("manifest.json", source))?;
    if manifest.format != "termtrace-bundle" || manifest.version != 1 {
        return Err(bundle_error("unsupported bundle manifest"));
    }
    if manifest.members.len() > MAX_BUNDLE_MEMBERS {
        return Err(bundle_error(format!(
            "bundle manifest exceeds {MAX_BUNDLE_MEMBERS} members"
        )));
    }
    let expected_canonical = trusted.canonical.as_os_str().as_encoded_bytes();
    if manifest.profile != policy.profile
        || manifest.session_id != trusted.manifest.session_id
        || manifest.canonical_session.as_bytes() != expected_canonical
        || manifest.source_final_event_hash != trusted.completion.final_event_hash
        || manifest.includes_canonical_evidence != (policy.profile == BundleProfile::Forensic)
    {
        return Err(bundle_error(
            "bundle manifest does not match the expected profile or canonical source identity",
        ));
    }
    verify_bundle_members(directory, policy.profile, &manifest.members)?;
    verify_bound_payloads(directory, &manifest, &trusted)?;
    if manifest.includes_canonical_evidence {
        let report = verify_session(directory.join("canonical"), VerificationLevel::Content);
        if !report.valid
            || report.session_id != Some(manifest.session_id)
            || report.final_event_hash.as_deref() != Some(manifest.source_final_event_hash.as_str())
        {
            return Err(bundle_error(
                "included canonical evidence failed validation",
            ));
        }
        verify_canonical_payload_matches_source(directory, &manifest.members, &trusted.canonical)?;
    }
    return Ok(manifest);
}

fn resolve_trusted_source(policy: &BundleVerificationPolicy) -> Result<TrustedBundleSource> {
    let canonical = policy.source_session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: policy.source_session.clone(),
            source,
        };
    })?;
    let log = EventLog::read(canonical.join("events.jsonl"))?;
    let completion = validate_completed_log(&canonical, &log)?;
    let manifest = read_manifest(&canonical)?;
    return Ok(TrustedBundleSource {
        canonical,
        manifest,
        completion,
        log,
    });
}

fn verify_bound_payloads(
    directory: &Path,
    manifest: &BundleManifest,
    trusted: &TrustedBundleSource,
) -> Result<()> {
    let observation_path = directory.join("observation.json");
    let observation: Observation = serde_json::from_slice(&read_bounded_regular_file(
        &observation_path,
        MAX_BUNDLE_STRUCTURED_MEMBER_BYTES,
    )?)
    .map_err(|source| return json_error("observation.json", source))?;
    let inspection = inspect(&trusted.log, StateSelector::LatestPresented)?;
    let expected_frame = frame_from_state(&inspection.state);
    let expected_frame_hash = frame_hash(&expected_frame)?;
    if observation.session_id != Some(trusted.manifest.session_id)
        || observation.source_kind != ObservationSourceKind::CompletedSession
        || observation
            .canonical_session
            .as_ref()
            .map(ByteString::as_bytes)
            != Some(trusted.canonical.as_os_str().as_encoded_bytes())
        || !observation.source_completed
        || observation.state_point != StatePoint::LatestPresented
        || observation.state_id != Some(inspection.record.state_id)
        || observation.state_hash != inspection.record.state_hash
        || observation.event_seq != inspection.record.event_seq
        || observation.monotonic_ns != Some(inspection.record.monotonic_ns)
        || !observation.presented
        || observation.boundary.next_event_seq != trusted.completion.event_count
        || observation.boundary.event_hash != trusted.completion.final_event_hash
        || observation.boundary.output_bytes != trusted.completion.output_bytes
        || observation.capture_quality != CaptureQuality::Completed
        || observation.truth_layer != TruthLayer::Semantic
        || observation.integrity != ObservationIntegrity::CanonicalChained
        || !matches!(
            observation.resolution,
            ObservationResolution::CanonicalReplay | ObservationResolution::ValidatedCheckpoint
        )
        || observation.frame != expected_frame
        || observation.payload_hash != expected_frame_hash
    {
        return Err(bundle_error(
            "bundle observation does not derive from the expected canonical source",
        ));
    }
    let screen = read_bounded_regular_file(
        &directory.join("screen.txt"),
        MAX_BUNDLE_STRUCTURED_MEMBER_BYTES,
    )?;
    if screen != observation.frame.viewport_text.as_bytes() {
        return Err(bundle_error(
            "bundle screen text does not match its source-bound observation",
        ));
    }

    if matches!(
        manifest.profile,
        BundleProfile::Debug | BundleProfile::Forensic
    ) {
        let verification: VerificationReport = serde_json::from_slice(&read_bounded_regular_file(
            &directory.join("verification.json"),
            MAX_BUNDLE_STRUCTURED_MEMBER_BYTES,
        )?)
        .map_err(|source| return json_error("verification.json", source))?;
        let expected_verification = verify_session(&trusted.canonical, VerificationLevel::Content);
        if !expected_verification.valid || verification != expected_verification {
            return Err(bundle_error(
                "bundle verification report does not bind the expected canonical source",
            ));
        }
    }

    if manifest.profile == BundleProfile::Debug {
        let metadata = manifest_member(manifest, "events-metadata.jsonl")?;
        let mut expected = HashingWriter::default();
        for record in &trusted.log.events {
            serde_json::to_writer(&mut expected, &EventProjection::from_record(record, true))
                .map_err(|source| return json_error("events-metadata.jsonl", source))?;
            expected.write_all(b"\n").map_err(|source| {
                return Error::Filesystem {
                    path: PathBuf::from("events-metadata.jsonl"),
                    source,
                };
            })?;
        }
        if metadata.byte_length != expected.bytes
            || metadata.blake3 != expected.hasher.finalize().to_hex().as_str()
        {
            return Err(bundle_error(
                "metadata-only event projection does not match the expected canonical source",
            ));
        }
    }
    return Ok(());
}

pub(super) fn verify_canonical_payload_matches_source(
    directory: &Path,
    members: &[ArtifactMember],
    canonical_source: &Path,
) -> Result<()> {
    let source_names = top_level_regular_file_names(canonical_source)?;
    let expected_paths = source_names
        .iter()
        .map(|name| return format!("canonical/{name}"))
        .collect::<BTreeSet<_>>();
    let included_paths = members
        .iter()
        .filter_map(|member| {
            return canonical_member_name(&member.path).map(|_| return member.path.clone());
        })
        .collect::<BTreeSet<_>>();
    if included_paths != expected_paths {
        return Err(bundle_error(
            "included canonical payload does not match the expected source inventory",
        ));
    }
    for name in source_names {
        let source = canonical_source.join(&name);
        let included = directory.join("canonical").join(&name);
        let expected = hash_regular_file(&source)?;
        let actual = hash_regular_file(&included)?;
        if actual != expected {
            return Err(bundle_error(format!(
                "included canonical `{name}` does not match the expected source"
            )));
        }
    }
    return Ok(());
}

fn manifest_member<'manifest>(
    manifest: &'manifest BundleManifest,
    path: &str,
) -> Result<&'manifest ArtifactMember> {
    return manifest
        .members
        .iter()
        .find(|member| return member.path == path)
        .ok_or_else(|| return bundle_error(format!("bundle is missing required member `{path}`")));
}

pub(super) fn read_bounded_regular_file(path: &Path, maximum_bytes: u64) -> Result<Vec<u8>> {
    let (mut file, size) = open_regular_file_nofollow(path)?;
    if size > maximum_bytes {
        return Err(bundle_error(format!(
            "bundle member `{}` exceeds {maximum_bytes} byte limit",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    io::Read::by_ref(&mut file)
        .take(maximum_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum_bytes {
        return Err(bundle_error(format!(
            "bundle member `{}` exceeds {maximum_bytes} byte limit",
            path.display()
        )));
    }
    return Ok(bytes);
}

#[derive(Default)]
struct HashingWriter {
    hasher: blake3::Hasher,
    bytes: u64,
}

impl io::Write for HashingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.hasher.update(buf);
        self.bytes = self
            .bytes
            .saturating_add(u64::try_from(buf.len()).unwrap_or(u64::MAX));
        return Ok(buf.len());
    }

    fn flush(&mut self) -> io::Result<()> {
        return Ok(());
    }
}

fn verify_bundle_members(
    directory: &Path,
    profile: BundleProfile,
    expected: &[ArtifactMember],
) -> Result<()> {
    if expected.len() > MAX_BUNDLE_MEMBERS {
        return Err(bundle_error(format!(
            "bundle manifest exceeds {MAX_BUNDLE_MEMBERS} members"
        )));
    }
    let expected_paths = expected
        .iter()
        .map(|member| return member.path.as_str())
        .collect::<BTreeSet<_>>();
    if expected_paths.len() != expected.len() {
        return Err(bundle_error(
            "bundle manifest contains duplicate member paths",
        ));
    }

    let mut actual = collect_artifact_members_bounded(directory, MAX_BUNDLE_TREE_ENTRIES)?;
    if actual.len() > MAX_BUNDLE_MEMBERS || actual.len() != expected.len() {
        return Err(bundle_error(
            "bundle payload members do not match their whole-file manifest",
        ));
    }
    for member in &mut actual {
        let semantics = bundle_member_semantics(profile, &member.path).ok_or_else(|| {
            return bundle_error(format!(
                "bundle member `{}` is outside the selected profile",
                member.path
            ));
        })?;
        member.sensitivity = semantics.sensitivity;
        member.relationship = semantics.relationship.to_owned();
        member.schema_type = semantics.schema_type.map(str::to_owned);
    }
    if actual != expected {
        return Err(bundle_error(
            "bundle payload members or semantic metadata do not match the selected profile",
        ));
    }
    for required in ["screen.txt", "observation.json"] {
        if !expected_paths.contains(required) {
            return Err(bundle_error(format!(
                "bundle is missing required member `{required}`"
            )));
        }
    }
    if matches!(profile, BundleProfile::Debug | BundleProfile::Forensic)
        && !expected_paths.contains("verification.json")
    {
        return Err(bundle_error(
            "bundle is missing required member `verification.json`",
        ));
    }
    if profile == BundleProfile::Debug && !expected_paths.contains("events-metadata.jsonl") {
        return Err(bundle_error(
            "bundle is missing required member `events-metadata.jsonl`",
        ));
    }
    if profile == BundleProfile::Forensic {
        for required in [
            "canonical/session.json",
            "canonical/events.jsonl",
            "canonical/completion.json",
        ] {
            if !expected_paths.contains(required) {
                return Err(bundle_error(format!(
                    "forensic bundle is missing required member `{required}`"
                )));
            }
        }
    }
    return Ok(());
}
