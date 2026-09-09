use super::*;
use crate::recording::{record, RecordOptions};
use std::{ffi::OsString, os::unix::fs::symlink};
use tempfile::TempDir;
use termtrace_evidence::event_log::DurabilityPolicy;
use termtrace_model::execution::session::RecordingFailurePolicy;
use termtrace_model::{
    execution::environment::EnvironmentSpec,
    execution::event::TerminalColors,
    execution::provenance::ProvenancePolicy,
    execution::session::{GeometryProvenance, ResourceLimits},
};

#[test]
fn alias_validation_rejects_paths_and_ambiguous_prefixes() {
    for alias in ["", "-bad", "../escape", "has space", "é"] {
        assert!(validate_alias(alias).is_err(), "accepted alias {alias:?}");
    }
    assert!(validate_alias("build-01_main").is_ok());
}

#[test]
fn registry_rejects_symlink_entries_without_following_them() {
    let temporary = TempDir::new().expect("temporary directory");
    let registry = SessionRegistry::open(temporary.path().join("state")).expect("registry");
    let target = temporary.path().join("target");
    fs_err::write(&target, b"{}").expect("target");
    symlink(&target, registry.entry_path("demo")).expect("symlink");

    assert!(registry.resolve("demo").is_err());
}

#[test]
fn reservation_excludes_concurrent_owners_and_promotes_only_the_winner() {
    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    let temporary = TempDir::new().expect("temporary directory");
    let registry =
        Arc::new(SessionRegistry::open(temporary.path().join("state")).expect("registry"));
    let sessions = registry.state_root().join("sessions");
    fs_err::create_dir(&sessions).expect("sessions directory");

    let barrier = Arc::new(Barrier::new(3));
    let mut contenders = Vec::new();

    for name in ["first", "second"] {
        let registry = Arc::clone(&registry);
        let barrier = Arc::clone(&barrier);
        let pending = sessions.join(format!("{name}.termtrace"));

        contenders.push(thread::spawn(move || {
            barrier.wait();
            return registry.reserve("demo", &pending, Duration::from_secs(30));
        }));
    }

    barrier.wait();

    let results = contenders
        .into_iter()
        .map(|contender| return contender.join().expect("reservation contender"))
        .collect::<Vec<_>>();

    assert_eq!(
        results
            .iter()
            .filter(|result| return result.is_ok())
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| return result.is_err())
            .count(),
        1
    );

    let reservation = results
        .into_iter()
        .find_map(Result::ok)
        .expect("winning reservation");
    let reservation_record: AliasReservation = serde_json::from_slice(
        &fs_err::read(registry.entry_path("demo")).expect("reservation record"),
    )
    .expect("reservation JSON");

    assert_eq!(reservation_record.owner_token, reservation.owner_token);
    assert_eq!(
        reservation_record.pending_session,
        reservation.pending_session
    );
    assert!(reservation_record.created_unix_ns > 0);
    assert!(reservation_record.lease_expires_unix_ns > 0);

    let mut wrong_owner = reservation.clone();
    wrong_owner.owner_token = Uuid::now_v7();
    assert!(registry.release(&wrong_owner).is_err());

    record_fixture(reservation.session_path());
    let entry = registry.promote(&reservation).expect("promote winner");
    assert_eq!(registry.resolve("demo").expect("resolve promoted"), entry);
}

#[test]
fn expired_reservation_is_reclaimed_without_accepting_the_old_owner() {
    let temporary = TempDir::new().expect("temporary directory");
    let registry = SessionRegistry::open(temporary.path().join("state")).expect("registry");
    let first_path = registry.state_root().join("sessions/first.termtrace");
    let first = registry
        .reserve("demo", &first_path, Duration::from_secs(30))
        .expect("first reservation");

    let mut expired = first.clone();
    expired.created_unix_ns = 0;
    expired.lease_expires_unix_ns = 1;
    fs_err::write(
        registry.entry_path("demo"),
        registry
            .encode_reservation(&expired)
            .expect("expired reservation encoding"),
    )
    .expect("publish expired reservation fixture");

    let second_path = registry.state_root().join("sessions/second.termtrace");
    let second = registry
        .reserve("demo", &second_path, Duration::from_secs(30))
        .expect("reclaim expired reservation");

    assert_ne!(second.owner_token, first.owner_token);
    assert!(registry.promote(&first).is_err());

    let resolved = registry
        .resolve_lifecycle("demo")
        .expect("resolve replacement reservation");

    assert_eq!(resolved.lifecycle, RegistryLifecycle::Incomplete);
    assert_eq!(resolved.session, second_path);

    registry.release(&second).expect("release replacement");
}

#[test]
fn retarget_uses_compare_and_swap_and_preserves_previous_binding_on_failure() {
    let temporary = TempDir::new().expect("temporary directory");
    let registry = SessionRegistry::open(temporary.path().join("state")).expect("registry");
    let first = record_fixture(temporary.path().join("first"));
    let second = record_fixture(temporary.path().join("second"));
    let original = registry.register("demo", &first).expect("register first");
    let rejected = registry.retarget("demo", Uuid::now_v7(), &second);
    assert!(rejected.is_err());
    assert_eq!(
        registry.resolve("demo").unwrap().session_id,
        original.session_id
    );

    let successor = registry
        .retarget("demo", original.session_id, &second)
        .expect("retarget successor");

    assert_ne!(successor.session_id, original.session_id);
    assert_eq!(registry.resolve("demo").unwrap(), successor);
}

fn record_fixture(output_directory: PathBuf) -> PathBuf {
    record(&RecordOptions {
        failure_policy: RecordingFailurePolicy::Stop,
        output_directory: output_directory.clone(),
        creation_alias: None,
        lineage: None,
        import_provenance: None,
        tmux_pane: None,
        command: vec![OsString::from("/bin/true")],
        working_directory: None,
        cols: 80,
        rows: 24,
        cell_width_px: 9,
        cell_height_px: 18,
        geometry_provenance: GeometryProvenance::default(),
        ghostty_profile: None,
        term: "xterm-ghostty".to_owned(),
        environment: EnvironmentSpec::default(),
        colors: TerminalColors::default(),
        forward_stdin: false,
        continue_after_output_mirror_closes: false,
        durability_policy: DurabilityPolicy::default(),
        provenance_policy: ProvenancePolicy::default(),
        resource_limits: ResourceLimits::default(),
    })
    .expect("record fixture");

    return output_directory;
}
