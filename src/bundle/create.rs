use super::*;

/// Atomically publish one manifested bundle directory without replacement.
pub fn create(
    session: &Path,
    destination: &Path,
    profile: BundleProfile,
) -> Result<BundleManifest> {
    let canonical = session.canonicalize().map_err(|source| {
        return Error::Filesystem {
            path: session.to_path_buf(),
            source,
        };
    })?;
    if destination.starts_with(&canonical) {
        return Err(bundle_error(
            "bundle destination must be outside the canonical session",
        ));
    }
    let log = EventLog::read(canonical.join("events.jsonl"))?;
    let completion = validate_completed_log(&canonical, &log)?;
    let source_manifest = read_manifest(&canonical)?;
    let parent = destination
        .parent()
        .ok_or_else(|| return bundle_error("bundle path has no parent"))?;
    let name = destination
        .file_name()
        .ok_or_else(|| return bundle_error("bundle path has no basename"))?;
    if !parent.is_dir() {
        return Err(bundle_error("bundle parent directory does not exist"));
    }
    let staging = parent.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        Uuid::now_v7()
    ));
    create_private_directory(&staging)?;
    let publish = (|| -> Result<BundleManifest> {
        let observation = observe_session(&canonical, StatePoint::LatestPresented)?;
        write_secure(
            &staging.join("screen.txt"),
            observation.frame.viewport_text.as_bytes(),
        )?;
        write_json(&staging.join("observation.json"), &observation)?;
        if matches!(profile, BundleProfile::Debug | BundleProfile::Forensic) {
            let verification = verify_session(&canonical, VerificationLevel::Content);
            if !verification.valid {
                return Err(bundle_error("canonical source failed content verification"));
            }
            write_json(&staging.join("verification.json"), &verification)?;
        }
        if profile == BundleProfile::Debug {
            let mut bytes = Vec::new();
            for record in &log.events {
                serde_json::to_writer(&mut bytes, &EventProjection::from_record(record, true))
                    .map_err(|source| return json_error("events-metadata.jsonl", source))?;
                bytes.push(b'\n');
            }
            write_secure(&staging.join("events-metadata.jsonl"), &bytes)?;
        }
        if profile == BundleProfile::Forensic {
            let target = staging.join("canonical");
            create_private_directory(&target)?;
            copy_top_level_files(&canonical, &target)?;
        }
        let mut members = collect_artifact_members_bounded(&staging, MAX_BUNDLE_TREE_ENTRIES)?;
        for member in &mut members {
            let semantics = bundle_member_semantics(profile, &member.path).ok_or_else(|| {
                return bundle_error(format!(
                    "created bundle member `{}` is outside the selected profile",
                    member.path
                ));
            })?;
            member.sensitivity = semantics.sensitivity;
            member.relationship = semantics.relationship.to_owned();
            member.schema_type = semantics.schema_type.map(str::to_owned);
        }
        let manifest = BundleManifest {
            format: "termtrace-bundle".to_owned(),
            version: 1,
            profile,
            session_id: source_manifest.session_id,
            canonical_session: ByteString::from(canonical.as_os_str().as_encoded_bytes()),
            source_final_event_hash: completion.final_event_hash,
            includes_canonical_evidence: profile == BundleProfile::Forensic,
            members,
        };
        write_json(&staging.join("manifest.json"), &manifest)?;
        sync_directory(&staging)?;
        return Ok(manifest);
    })();
    let manifest = match publish {
        Ok(value) => value,
        Err(error) => {
            let _cleanup = fs_err::remove_dir_all(&staging);
            return Err(error);
        }
    };
    if let Err(error) = commit_directory(parent, &staging, destination) {
        let _cleanup = fs_err::remove_dir_all(&staging);
        return Err(error);
    }
    return Ok(manifest);
}

fn copy_top_level_files(source: &Path, destination: &Path) -> Result<()> {
    for name in top_level_regular_file_names(source)? {
        let path = source.join(&name);
        let (mut reader, _size) = open_regular_file_nofollow(&path)?;
        let target = destination.join(name);
        let mut writer = secure_writer(&target)?;
        io::copy(&mut reader, &mut writer).map_err(|source_error| {
            return Error::Filesystem {
                path: path.clone(),
                source: source_error,
            };
        })?;
        finish_writer(writer, &target)?;
    }
    return Ok(());
}

pub(super) fn top_level_regular_file_names(directory: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs_err::read_dir(directory).map_err(|source_error| {
        return Error::Filesystem {
            path: directory.to_path_buf(),
            source: source_error,
        };
    })? {
        let entry = entry.map_err(|source_error| {
            return Error::Filesystem {
                path: directory.to_path_buf(),
                source: source_error,
            };
        })?;
        let file_type: FileType = entry.file_type().map_err(|source_error| {
            return Error::Filesystem {
                path: entry.path(),
                source: source_error,
            };
        })?;
        if file_type.is_symlink() {
            return Err(bundle_error(
                "canonical session contains a top-level symlink",
            ));
        }
        if file_type.is_dir() {
            continue;
        }
        if file_type.is_block_device()
            || file_type.is_char_device()
            || file_type.is_fifo()
            || file_type.is_socket()
        {
            return Err(bundle_error(
                "canonical session contains a top-level special file",
            ));
        }
        let name = entry.file_name().into_string().map_err(|_name| {
            return bundle_error("canonical top-level file names must be valid UTF-8");
        })?;
        names.push(name);
        if names.len() > MAX_BUNDLE_MEMBERS {
            return Err(bundle_error(format!(
                "canonical source exceeds {MAX_BUNDLE_MEMBERS} top-level files"
            )));
        }
    }
    names.sort();
    return Ok(names);
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|source| return json_error(path.to_string_lossy().as_ref(), source))?;
    bytes.push(b'\n');
    return write_secure(path, &bytes);
}
