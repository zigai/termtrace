//! Integration tests for `RegistryService` and `TmuxService`.

mod support;

use support::record_session;
use tempfile::TempDir;
use termtrace_model::execution::session::SessionPhase;
use termtrace_service::registry::{RegistryService, ValueOverride};

#[test]
fn test_registry_lifecycle_and_gc() {
    let temp = TempDir::new().expect("tempdir");
    let state_root = temp.path().join("state-root");
    let session_path = temp.path().join("sample-reg.termtrace");
    record_session(&session_path, "printf 'REGISTRY-TEST\n'; sleep 0.05");

    let registry = RegistryService::open(&state_root).expect("open registry");

    // Register alias
    let entry = registry
        .register("test-alias", &session_path)
        .expect("register");

    assert_eq!(entry.alias, "test-alias");
    assert_eq!(entry.session_path(), session_path);

    // Resolve alias
    let resolved = registry.resolve("test-alias").expect("resolve");

    assert_eq!(resolved.alias, "test-alias");
    assert_eq!(resolved.session_path(), session_path);

    // Resolve lifecycle
    let lifecycle = registry
        .resolve_lifecycle("test-alias")
        .expect("resolve lifecycle");
    assert_eq!(lifecycle.session, session_path);
    assert_eq!(lifecycle.lifecycle, SessionPhase::Completed);

    // List aliases
    let list = registry.list(true).expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].alias, "test-alias");

    // Remove alias
    let removed = registry.remove("test-alias").expect("remove");

    assert_eq!(removed.alias, "test-alias");
    assert!(registry.resolve("test-alias").is_err());

    // Session evidence still exists after alias removal
    assert!(session_path.join("session.json").is_file());

    // Run GC (dry run)
    let gc_report = registry.gc(30, 20, None, false).expect("gc dry run");

    assert_eq!(gc_report.removed_sessions, 0);
}

#[test]
fn test_value_override_semantics() {
    let keep: ValueOverride<u16> = ValueOverride::Keep;
    let set: ValueOverride<u16> = ValueOverride::Set(120);
    let clear: ValueOverride<u16> = ValueOverride::Clear;

    assert_eq!(keep, ValueOverride::Keep);
    assert_ne!(keep, set);
    assert_ne!(set, clear);

    let default: ValueOverride<String> = ValueOverride::default();
    assert_eq!(default, ValueOverride::Keep);
}

#[test]
fn detached_track_adapter_reaps_its_original_child_after_completion() {
    use std::{
        ffi::OsString,
        path::PathBuf,
        thread,
        time::{Duration, Instant},
    };
    use termtrace_service::{
        client::{Session, StopRequest},
        registry::DetachedStart,
    };
    let temporary = TempDir::new().expect("private root");
    let registry = RegistryService::open(temporary.path().join("state")).expect("registry");
    let binary = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/termtrace");
    let started = registry
        .start_detached(
            &binary,
            DetachedStart {
                alias: "adapter-reap".into(),
                session: None,
                startup_timeout: Duration::from_secs(5),
                stdin: None,
            },
            |path| {
                return vec![
                    OsString::from("record"),
                    "--no-input".into(),
                    "--out".into(),
                    path.as_os_str().into(),
                    "--".into(),
                    "/bin/sh".into(),
                    "-c".into(),
                    "printf READY; read line".into(),
                ];
            },
        )
        .expect("start via shared adapter path");

    let pid = started
        .entry
        .recorder_pid
        .expect("live original recorder identity");

    let mut attached =
        Session::attach(&started.paths.session, &binary).expect("attach detached track");
    attached
        .stop(StopRequest::default(), Some(Duration::from_secs(5)))
        .expect("validated completion");

    let process = PathBuf::from(format!("/proc/{pid}"));
    let deadline = Instant::now() + Duration::from_secs(3);

    while process.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }

    assert!(
        !process.exists(),
        "original recorder must not be left as a zombie"
    );
}
