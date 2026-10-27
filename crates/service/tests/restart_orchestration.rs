//! Shared restart admission failures preserve evidence/alias authority and retain orphan diagnostics.
mod support;

#[cfg(test)]
mod tests {
    use super::support::record_session;
    use std::{
        collections::BTreeMap,
        ffi::OsString,
        fs,
        path::{Path, PathBuf},
        time::Duration,
    };
    use tempfile::TempDir;
    use termtrace_model::protocol::management::{
        FailureContext, ManagementFailure, ManagementGuidance, RestartFailureFacts,
        RestartFailurePhase, SuccessorReadiness,
    };
    use termtrace_model::protocol::mutation::{MutationEffect, Retry};
    use termtrace_model::{registry::RestartConfiguration, value::ByteString};
    use termtrace_service::{error::ErrorKind, registry::RegistryService};

    fn facts(error: &termtrace_service::Error) -> RestartFailureFacts {
        assert_eq!(error.retry(), Retry::DoNotRetry);
        let FailureContext::Management {
            guidance,
            management,
        } = error.failure_context()
        else {
            panic!("management discriminator")
        };
        assert_eq!(guidance, ManagementGuidance::InspectNoAutomaticRetry);
        let ManagementFailure::RegistryRestart(facts) = *management else {
            panic!("restart facts")
        };
        assert!(facts.predecessor_completion.is_some(), "{error:?}");

        return facts;
    }

    fn binary() -> PathBuf {
        return PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/termtrace");
    }

    fn bytes(source: &Path) -> BTreeMap<OsString, Vec<u8>> {
        return fs::read_dir(source)
            .expect("source directory")
            .filter_map(|item| {
                let item = item.expect("canonical member");
                if item.file_type().expect("member type").is_dir() {
                    return None;
                }
                return Some((
                    item.file_name(),
                    fs::read(item.path()).expect("canonical bytes"),
                ));
            })
            .collect();
    }

    fn arguments(predecessor: &Path, destination: &Path, lineage: bool) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![
            "record".into(),
            "--no-input".into(),
            "--out".into(),
            destination.as_os_str().into(),
        ];

        if lineage {
            args.extend([
                OsString::from("--restart-source"),
                predecessor.as_os_str().into(),
            ]);
        }

        args.extend(["--".into(), "/bin/true".into()]);

        return args;
    }

    #[test]
    fn failed_readiness_and_timeout_preserve_completed_predecessor_and_orphan_logs() {
        let root = TempDir::new().expect("private root");
        let source = root.path().join("predecessor");
        record_session(&source, "printf PREDECESSOR");
        let before = bytes(&source);
        let registry = RegistryService::open(root.path().join("state")).expect("registry");
        let old = registry.register("demo", &source).expect("alias");

        for (program, args, timeout) in [
            (Path::new("/bin/false"), Vec::new(), Duration::from_secs(1)),
            (
                Path::new("/bin/sleep"),
                vec![OsString::from("0.2")],
                Duration::from_millis(1),
            ),
        ] {
            let error = registry
                .restart_with(program, "demo", timeout, None, |_, _| return Ok(args))
                .expect_err("successor cannot be admitted");

            let progress = facts(&error);
            assert_eq!(progress.phase, RestartFailurePhase::SuccessorLaunched);

            let successor = progress.successor.expect("allocated orphan");

            assert!(successor.launched);
            assert!(matches!(
                successor.readiness,
                Some(SuccessorReadiness::Exited | SuccessorReadiness::TimedOut)
            ));
            assert_eq!(
                progress.predecessor_path.expect("path").as_bytes(),
                source.as_os_str().as_encoded_bytes()
            );
            assert!(error.to_string().contains("recoverable orphan"));
            assert!(error.to_string().contains("stdout log"));
            assert!(error.to_string().contains("stderr log"));
            assert_eq!(
                registry.resolve("demo").expect("retained alias").session_id,
                old.session_id
            );
            assert_eq!(bytes(&source), before);
        }
    }

    #[test]
    fn ready_successor_without_lineage_is_not_published_and_evidence_is_not_removed() {
        let root = TempDir::new().expect("private root");
        let source = root.path().join("predecessor");
        record_session(&source, "printf PREDECESSOR");
        let before = bytes(&source);
        let registry = RegistryService::open(root.path().join("state")).expect("registry");
        let old = registry.register("demo", &source).expect("alias");
        let mut orphan = None;
        let error = registry
            .restart_with(
                &binary(),
                "demo",
                Duration::from_secs(5),
                None,
                |predecessor, destination| {
                    orphan = Some(destination.to_path_buf());

                    return Ok(arguments(predecessor, destination, false));
                },
            )
            .expect_err("missing immutable lineage");
        assert_eq!(facts(&error).phase, RestartFailurePhase::SuccessorReady);
        assert!(error.to_string().contains("lineage"));

        let orphan = orphan.expect("real allocated successor");

        assert!(orphan.join("session.json").exists());
        assert_eq!(
            registry
                .resolve("demo")
                .expect("predecessor alias")
                .session_id,
            old.session_id
        );
        assert_eq!(bytes(&source), before);
    }

    #[test]
    fn compare_and_retarget_failure_never_clobbers_another_owners_alias() {
        let root = TempDir::new().expect("private root");
        let source = root.path().join("predecessor");
        let other = root.path().join("other");
        record_session(&source, "printf PREDECESSOR");
        record_session(&other, "printf OTHER");
        let before = bytes(&source);
        let registry = RegistryService::open(root.path().join("state")).expect("registry");
        registry
            .register("demo", &source)
            .expect("predecessor alias");

        let mut replacement = None;
        let mut orphan = None;
        let error = registry
            .restart_with(
                &binary(),
                "demo",
                Duration::from_secs(5),
                None,
                |predecessor, destination| {
                    registry.remove("demo")?;
                    replacement = Some(registry.register("demo", &other)?);
                    orphan = Some(destination.to_path_buf());

                    return Ok(arguments(predecessor, destination, true));
                },
            )
            .expect_err("concurrent owner won alias");
        assert_eq!(facts(&error).phase, RestartFailurePhase::AliasRetarget);
        assert_eq!(error.kind(), ErrorKind::Engine);
        assert!(
            facts(&error)
                .successor
                .expect("allocated orphan facts")
                .launched
        );
        assert_eq!(
            registry
                .resolve("demo")
                .expect("other owner's alias")
                .session_id,
            replacement.expect("replacement").session_id
        );
        assert!(orphan.expect("orphan").join("session.json").exists());
        assert_eq!(bytes(&source), before);
    }

    #[test]
    fn typed_restart_keeps_lineage_and_explicit_overrides_without_source_rewrite() {
        let root = TempDir::new().expect("private root");
        let source = root.path().join("predecessor");
        record_session(&source, "printf PREDECESSOR");
        let before = bytes(&source);
        let registry = RegistryService::open(root.path().join("state")).expect("registry");
        let old = registry.register("demo", &source).expect("alias");
        let report = registry
            .restart(
                &binary(),
                "demo",
                &RestartConfiguration {
                    cols: Some(66),
                    command: Some(vec![ByteString::from(b"/bin/true".as_slice())]),
                    ..Default::default()
                },
                None,
                Duration::from_secs(5),
            )
            .expect("typed successor");
        assert!(report.ready);
        assert_eq!(report.predecessor.session_id, old.session_id);
        assert_eq!(
            registry
                .resolve("demo")
                .expect("successor alias")
                .session_id,
            report.successor.session_id
        );

        let value: serde_json::Value = serde_json::from_slice(
            &fs::read(report.successor.session_path().join("session.json")).expect("successor"),
        )
        .expect("manifest");
        assert_eq!(
            value["lineage"]["predecessor_session_id"],
            old.session_id.to_string()
        );
        assert_eq!(value["geometry"]["cols"], 66);
        assert_eq!(bytes(&source), before);
    }

    #[test]
    fn live_predecessor_completion_is_not_lost_when_successor_exits() {
        use std::{
            process::{Child, Command},
            thread,
            time::Instant,
        };
        use termtrace_service::client::{await_ready, Readiness, Session};
        let root = TempDir::new().expect("private root");
        let source = root.path().join("live");

        struct Recorder(Child);

        impl Drop for Recorder {
            fn drop(&mut self) {
                let _kill = self.0.kill();
                let _wait = self.0.wait();
            }
        }

        let mut recorder = Recorder(
            Command::new(binary())
                .args(["record", "--no-input", "--out"])
                .arg(&source)
                .args(["--", "/bin/sh", "-c", "printf LIVE; read line"])
                .spawn()
                .expect("recorder"),
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        while !source.join("session.json").exists() {
            assert!(Instant::now() < deadline);

            thread::sleep(Duration::from_millis(10));
        }

        let ready = await_ready(&source, &mut recorder.0, deadline).expect("actual readiness");
        assert!(matches!(ready, Readiness::Live));

        let registry = RegistryService::open(root.path().join("state")).expect("registry");
        let old = registry.register("demo", &source).expect("live alias");
        let error = registry
            .restart_with(
                Path::new("/bin/false"),
                "demo",
                Duration::from_secs(5),
                None,
                |_, _| return Ok(Vec::new()),
            )
            .expect_err("failed successor");

        let facts = facts(&error);
        let completion = facts.predecessor_completion.expect("actual completed stop");

        assert_eq!(completion.session_id, old.session_id);
        assert!(completion.event_count > 0);
        assert_eq!(
            Session::attach(&source, binary())
                .expect("attach")
                .completion()
                .expect("completion"),
            Some(*completion)
        );

        let _status = recorder
            .0
            .wait()
            .expect("original child reaped after requested stop");

        assert_eq!(
            registry.resolve("demo").expect("safe alias").session_id,
            old.session_id
        );
        assert_ne!(error.effect(), MutationEffect::NotApplied);
    }
}
