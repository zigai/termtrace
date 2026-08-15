use super::*;

#[test]
#[allow(
    clippy::needless_collect,
    reason = "all recorders must launch before any join to prove concurrent isolation"
)]
fn concurrent_client_launches_keep_environment_snapshots_isolated() {
    let temporary = TempDir::new().expect("temporary directory");
    let evidence = temporary.path().join("concurrent-evidence");
    fs_err::create_dir(&evidence).expect("evidence root");
    let workers = (0..8)
        .map(|index| {
            let evidence = evidence.clone();
            return thread::spawn(move || {
                let value = format!("isolated-{index}-{}", "x".repeat(index));
                let script = format!(
                    "test \"${{#ISOLATED_ID}}\" -eq {}; printf OK; sleep 30",
                    value.len()
                );
                let terminal = ClientTerminal::new(env!("CARGO_BIN_EXE_termtrace"), &evidence);
                let mut session = terminal
                    .launch([
                        OsString::from("/bin/sh"),
                        OsString::from("-c"),
                        OsString::from(script),
                    ])
                    .clear_environment()
                    .copy_environment("PATH")
                    .set_environment("ISOLATED_ID", value.into_bytes())
                    .color_policy(ColorPolicy::Never)
                    .output_byte_budget(1_000_000)
                    .start()
                    .expect("concurrent launch");
                wait_for_output(session.path(), b"OK", Duration::from_secs(3))
                    .expect("concurrent output");
                session
                    .stop(Some(Duration::from_secs(5)))
                    .expect("concurrent stop");
                let manifest = read_manifest(session.path()).expect("concurrent manifest");
                assert_eq!(manifest.resource_limits.output_bytes, Some(1_000_000));
                return manifest.child_environment.effective_digest;
            });
        })
        .collect::<Vec<_>>();
    let digests = workers
        .into_iter()
        .map(|worker| return worker.join().expect("launch worker"))
        .collect::<BTreeSet<_>>();
    assert_eq!(digests.len(), 8);
    assert!(
        fs_err::read_dir(&evidence)
            .expect("evidence entries")
            .filter_map(|entry| return entry.ok())
            .all(|entry| return !entry.file_name().to_string_lossy().contains("environment"))
    );
}
