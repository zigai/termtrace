//! Public Rust SDK: source-bound observations, profiles and real aggregate track evidence.
use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fs,
    os::unix::{
        ffi::{OsStrExt as _, OsStringExt as _},
        fs::PermissionsExt as _,
    },
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};
use termtrace_sdk::{
    analysis::AnalysisService,
    artifacts::{ArtifactService, SaveFormat},
    integrity::{IntegrityService, VerificationLevel},
    session::Session,
    tmux::{
        CatalogPage, ServiceTracks, StopRequest, TmuxClient, TmuxGhosttyProfile,
        TmuxRecordConfiguration, TmuxService, TmuxSocket,
    },
    BundleProfile, BundleVerificationPolicy, ByteString, StatePoint, Uuid,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
fn hashes(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            let output = Command::new("sha256sum").arg(entry.path()).output()?;
            if !output.status.success() {
                return Err("source hashing failed".into());
            }
            let digest =
                std::str::from_utf8(output.stdout.get(..64).ok_or("hash missing")?)?.to_owned();
            result.insert(entry.file_name().to_string_lossy().into_owned(), digest);
        }
    }
    return Ok(result);
}
fn main() -> Result<()> {
    let binary = PathBuf::from(env::var_os("TERMTRACE_BIN").ok_or("TERMTRACE_BIN missing")?);
    let root = PathBuf::from(
        env::args_os()
            .nth(1)
            .ok_or("private evidence root required")?,
    )
    .join(Uuid::now_v7().to_string());
    fs::create_dir_all(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let tracks = ServiceTracks::open(&binary, Some(&tracks_root(&root)))?;
    let socket = root.join("s");
    let client = TmuxClient::new(
        "/usr/bin/tmux".into(),
        TmuxSocket::Path(ByteString::from(socket.as_os_str().as_bytes())),
    );
    struct PrivateSocket<'a>(&'a Path);
    impl Drop for PrivateSocket<'_> {
        fn drop(&mut self) {
            let _cleanup = Command::new("/usr/bin/tmux")
                .arg("-S")
                .arg(self.0)
                .arg("kill-server")
                .output();
        }
    }
    let _private_socket = PrivateSocket(&socket);
    let outcome = (|| -> Result<()> {
        let started = TmuxService::record_configured(
            &tracks,
            client.clone(),
            TmuxRecordConfiguration {
                name: "rust-native".into(),
                new_server: true,
                ghostty_profile: TmuxGhosttyProfile::Off,
                command: ["/usr/bin/tmux", "new-session", "-s", "native"]
                    .into_iter()
                    .map(|v| ByteString::from(v.as_bytes()))
                    .collect(),
                ..Default::default()
            },
        )?;
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let audit = TmuxService::status(&client)?;
            if audit.recorded_panes >= 1 {
                assert_eq!(
                    audit
                        .configured_executable
                        .as_ref()
                        .map(ByteString::as_bytes),
                    Some(fs::canonicalize(&binary)?.as_os_str().as_bytes())
                );
                break;
            }
            if Instant::now() >= deadline {
                return Err("pane capture did not start".into());
            }
            thread::sleep(Duration::from_millis(25));
        }
        let mut recorded = TmuxService::recording_status(&tracks_root(&root), "rust-native")?;
        let pane_deadline = Instant::now() + Duration::from_secs(8);
        while recorded.state.pane_tracks.is_empty() && Instant::now() < pane_deadline {
            thread::sleep(Duration::from_millis(25));
            recorded = TmuxService::recording_status(&tracks_root(&root), "rust-native")?;
        }
        assert!(!recorded.state.pane_tracks.is_empty());
        for path in recorded.state.pane_tracks.values() {
            if env::var_os("TASK05_AUDIT_MAP_STOP_ONLY").is_some() {
                assert!(path.as_bytes().contains(&255));
            }
        }
        // Real committed aggregate stop followed by an uncertain nested actor transport.
        let registry = termtrace_sdk::registry::RegistryService::open(tracks_root(&root))?;
        let pid = registry
            .resolve("tmux-rust-native-composite")?
            .recorder_pid
            .ok_or("live recorder pid")?;
        let status = Command::new("kill")
            .args(["-STOP", &pid.to_string()])
            .status()?;
        assert!(status.success());
        let failed = TmuxService::stop(
            &tracks,
            StopRequest {
                name: "rust-native",
                wait: true,
                timeout: Duration::from_millis(30),
            },
        );
        let resume = Command::new("kill")
            .args(["-CONT", &pid.to_string()])
            .status()?;
        assert!(resume.success());
        let error = failed.expect_err("paused composite cannot acknowledge");
        use termtrace_sdk::management::{FailureContext, ManagementFailure, TmuxFailurePhase};
        let FailureContext::Management { management, .. } = error.failure_context() else {
            return Err("management scope".into());
        };
        let ManagementFailure::Tmux(facts) = *management else {
            return Err("tmux facts".into());
        };
        assert_eq!(facts.stop_requested, Some(true));
        assert_eq!(facts.phase, TmuxFailurePhase::StopRequested);
        assert_eq!(facts.manifest, Some(started.manifest.clone()));
        assert_eq!(error.kind(), termtrace_sdk::ErrorKind::ControlTimeout);
        let stopped = TmuxService::stop(
            &tracks,
            StopRequest {
                name: "rust-native",
                wait: true,
                timeout: Duration::from_secs(10),
            },
        )?;
        let (_, summary, catalog) = TmuxService::catalog(
            tracks_root(&root).as_path(),
            "rust-native",
            false,
            1,
            100,
            false,
        )?;
        assert_eq!(summary.unfinished_tracks, 0);
        let CatalogPage::Tracks(entries) = catalog else {
            return Err("expected tracks".into());
        };
        assert!(!entries.is_empty());
        assert!(entries.iter().all(|entry| entry.completed));
        let link = stopped
            .state
            .composite_track
            .link
            .as_ref()
            .ok_or("composite missing")?;
        let source = PathBuf::from(std::ffi::OsString::from_vec(
            link.session.as_bytes().to_vec(),
        ));
        let before = hashes(&source)?;
        if env::var_os("TASK05_AUDIT_MAP_STOP_ONLY").is_some() {
            fs::write(
                root.join("audit05-observed.json"),
                serde_json::to_vec_pretty(
                    &serde_json::json!({"management": ManagementFailure::Tmux(facts), "pane_tracks": recorded.state.pane_tracks, "source_hashes": before}),
                )?,
            )?;
            return Ok(());
        }
        let attached = Session::attach(&source, &binary)?;
        let mut expected = attached.observe(StatePoint::LatestPresented, false)?;
        expected.resolution = termtrace_sdk::screen::ObservationResolution::CanonicalReplay;
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../fixtures/conformance/sdk/diagnostics-evidence.json"
        ))?;
        for name in fixture["profiles"].as_array().ok_or("profile fixture")? {
            let profile: BundleProfile = serde_json::from_value(name.clone())?;
            let directory = root.join(format!("profile-{}", name.as_str().ok_or("profile name")?));
            ArtifactService::create_bundle(&source, &directory, profile)?;
            AnalysisService::cache_prune(&source)?;
            ArtifactService::verify_bundle(
                &directory,
                BundleVerificationPolicy::new(&source, profile),
            )?;
            let mut actual = attached.observe(StatePoint::LatestPresented, false)?;
            actual.resolution = expected.resolution;
            assert_eq!(actual, expected);
            if env::var_os("TASK05_NONUTF8_BUNDLE_PROFILES").is_some()
                && profile != BundleProfile::Screen
            {
                let encoded = fs::read(directory.join("verification.json"))?;
                let report: termtrace_sdk::integrity::VerificationReport =
                    serde_json::from_slice(&encoded)?;
                assert_eq!(
                    report.session.as_os_str().as_bytes(),
                    source.as_os_str().as_bytes()
                );
                let different = entries
                    .iter()
                    .find(|entry| entry.track.session.as_bytes() != source.as_os_str().as_bytes())
                    .ok_or("another actual track required")?;
                let different = PathBuf::from(std::ffi::OsString::from_vec(
                    different.track.session.as_bytes().to_vec(),
                ));
                assert!(ArtifactService::verify_bundle(
                    &directory,
                    BundleVerificationPolicy::new(&different, profile)
                )
                .is_err());
                let mut damaged = encoded.clone();
                damaged.extend_from_slice(b"TAMPER");
                fs::write(directory.join("verification.json"), damaged)?;
                assert!(ArtifactService::verify_bundle(
                    &directory,
                    BundleVerificationPolicy::new(&source, profile)
                )
                .is_err());
                fs::write(directory.join("verification.json"), encoded)?;
                ArtifactService::verify_bundle(
                    &directory,
                    BundleVerificationPolicy::new(&source, profile),
                )?;
            }
        }
        assert_eq!(before, hashes(&source)?);
        if env::var_os("TASK05_NONUTF8_BUNDLE_PROFILES").is_some() {
            fs::write(
                root.join("verification-path-observed.json"),
                serde_json::to_vec_pretty(
                    &serde_json::json!({"source": ByteString::from(source.as_os_str().as_bytes()), "profiles": fixture["profiles"], "source_hashes_before": before, "source_hashes_after": hashes(&source)?}),
                )?,
            )?;
            return Ok(());
        }
        let manifest = PathBuf::from(std::ffi::OsString::from_vec(started.manifest.into_bytes()));
        let directory = root.join("aggregate-bundle");
        let bundle = TmuxService::bundle(&manifest, &directory)?;
        let verified = TmuxService::verify_bundle(&directory, &manifest)?;
        assert_eq!(bundle.recording_id, verified.recording_id);
        assert!(bundle.track_count >= 2);
        assert!(TmuxService::bundle(&manifest, &directory).is_err());
        let formats: Vec<SaveFormat> = serde_json::from_value(fixture["save_formats"].clone())?;
        let saved = ArtifactService::save(
            &source,
            &root.join("all-formats"),
            StatePoint::LatestPresented,
            &formats,
            false,
        )?;
        assert_eq!(saved.manifest.files.len(), formats.len());
        assert!(!saved.manifest.semantic_image_resources.is_empty());
        assert!(ArtifactService::save(
            &source,
            &root.join("all-formats"),
            StatePoint::LatestPresented,
            &formats,
            false
        )
        .is_err());
        assert_eq!(before, hashes(&source)?);
        fs::write(
            root.join("observed.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({"source_hashes_before": before, "source_hashes_after": hashes(&source)?, "track_count": bundle.track_count, "profiles": fixture["profiles"], "aggregate_verified": true}),
            )?,
        )?;
        signing(&source, &root)?;
        return Ok(());
    })();
    let cleanup = TmuxService::stop(
        &tracks,
        StopRequest {
            name: "rust-native",
            wait: true,
            timeout: Duration::from_secs(10),
        },
    );
    let _socket_cleanup = Command::new("/usr/bin/tmux")
        .arg("-S")
        .arg(&socket)
        .arg("kill-server")
        .output();
    if outcome.is_err() && cleanup.is_err() {
        eprintln!("secondary tmux cleanup failure: {cleanup:?}");
    }
    outcome?;
    cleanup?;
    if env::var_os("TASK05_AUDIT_MAP_STOP_ONLY").is_some() {
        println!(
            "Rust management/nonUTF8 typed-map evidence retained at {}",
            root.display()
        );
    } else if env::var_os("TASK05_NONUTF8_BUNDLE_PROFILES").is_some() {
        println!(
            "Rust nonUTF8 bundle verification-path evidence retained at {}",
            root.display()
        );
    } else {
        println!(
            "Rust native aggregate/profile/source-hash evidence retained at {}",
            root.display()
        );
    }
    return Ok(());
}
fn tracks_root(root: &Path) -> PathBuf {
    if env::var_os("TASK05_AUDIT_MAP_STOP_ONLY").is_some()
        || env::var_os("TASK05_NONUTF8_BUNDLE_PROFILES").is_some()
    {
        return root.join(std::ffi::OsString::from_vec(b"state-\xff".to_vec()));
    }
    return root.join("state");
}

fn signing(source: &Path, root: &Path) -> Result<()> {
    let keys = root.join("temporary-ssh");
    fs::create_dir(&keys)?;
    fs::set_permissions(&keys, fs::Permissions::from_mode(0o700))?;
    let outcome = (|| -> Result<()> {
        let unsigned = IntegrityService::verify(source, VerificationLevel::Signature, None);
        assert!(!unsigned.valid && !unsigned.signature_receipt_present);
        let key = keys.join("key");
        let generated = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(&key)
            .output()?;
        if !generated.status.success() {
            return Err("temporary OpenSSH key generation failed".into());
        }
        let public = fs::read_to_string(key.with_extension("pub"))?;
        let allowed = keys.join("allowed");
        let wrong = keys.join("wrong");
        fs::write(&allowed, format!("rust05 {public}"))?;
        fs::write(&wrong, format!("wrong-identity {public}"))?;
        let receipt = IntegrityService::sign(source, &key, "rust05")?;
        let untrusted = IntegrityService::verify(source, VerificationLevel::Signature, None);
        assert!(untrusted.signature_cryptographic_valid);
        assert!(!untrusted.valid && !untrusted.signature_signer_trusted);
        let trusted =
            IntegrityService::verify(source, VerificationLevel::Signature, Some(&allowed));
        assert!(trusted.valid && trusted.session_authenticity_established);
        let rejected = IntegrityService::verify(source, VerificationLevel::Signature, Some(&wrong));
        assert!(rejected.signature_cryptographic_valid && !rejected.signature_signer_trusted);
        let original = fs::read(&receipt)?;
        fs::write(&receipt, b"{")?;
        assert!(
            !IntegrityService::verify(source, VerificationLevel::Signature, Some(&allowed)).valid
        );
        let mut tampered: serde_json::Value = serde_json::from_slice(&original)?;
        tampered["payload"]["session_manifest_hash"] = serde_json::Value::String("0".repeat(64));
        fs::write(&receipt, serde_json::to_vec(&tampered)?)?;
        assert!(
            !IntegrityService::verify(source, VerificationLevel::Signature, Some(&allowed)).valid
        );
        fs::write(&receipt, original)?;
        assert!(
            IntegrityService::verify(source, VerificationLevel::Signature, Some(&allowed)).valid
        );
        fs::write(
            root.join("trust-observed.json"),
            serde_json::to_vec_pretty(
                &serde_json::json!({"unsigned": unsigned, "valid_untrusted": untrusted, "trusted": trusted, "wrong_signer": rejected}),
            )?,
        )?;
        return Ok(());
    })();
    let cleanup = fs::remove_dir_all(&keys);
    if outcome.is_err() && cleanup.is_err() {
        eprintln!("secondary temporary-key cleanup failure: {cleanup:?}");
    }
    outcome?;
    cleanup?;
    return Ok(());
}
