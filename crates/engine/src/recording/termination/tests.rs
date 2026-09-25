use super::*;

const MS: Duration = Duration::from_millis(1);

fn exited() -> ExitStatus {
    return ExitStatus::with_exit_code(0);
}

fn tick(now: Instant) -> Tick {
    return Tick {
        now,
        received_output: false,
        output_bytes: 0,
        input_sequence_pending: false,
    };
}

fn output_tick(now: Instant) -> Tick {
    return Tick {
        received_output: true,
        ..tick(now)
    };
}

fn force_kill(code: &'static str, message: &'static str) -> Vec<Effect> {
    return vec![
        Effect::AppendHostError { code, message },
        Effect::KillProcessGroup,
    ];
}

fn drain_failure() -> Vec<Effect> {
    return vec![Effect::Fail {
        operation: "drain PTY output",
        reason: "PTY remained open after terminating retained process-group descendants",
    }];
}

fn draining_since(start: Instant) -> Termination {
    let mut termination = Termination::new();
    termination.observe(Fact::ChildExited {
        status: exited(),
        at: start,
    });

    return termination;
}

#[test]
fn running_child_needs_no_effects() {
    let start = Instant::now();
    let mut termination = Termination::new();

    assert!(termination.step(tick(start + 10 * MS)).is_empty());
    assert!(!termination.child_exited());
    assert!(!termination.output_closed());
    assert!(termination.exit_status().is_none());
}

#[test]
fn exit_with_open_output_kills_retained_descendants_after_grace() {
    let start = Instant::now();
    let mut termination = draining_since(start);

    assert!(termination.child_exited());
    assert!(!termination.output_closed());
    assert!(termination.step(tick(start + 249 * MS)).is_empty());
    assert_eq!(
        termination.step(tick(start + 250 * MS)),
        vec![Effect::KillProcessGroup]
    );
    assert!(termination.step(tick(start + 2249 * MS)).is_empty());
    assert_eq!(termination.step(tick(start + 2250 * MS)), drain_failure());
}

#[test]
fn late_output_extends_drain_up_to_the_ceiling() {
    let start = Instant::now();
    let mut termination = draining_since(start);

    assert!(termination.step(output_tick(start + 200 * MS)).is_empty());
    assert!(termination.step(tick(start + 449 * MS)).is_empty());
    assert!(termination.step(output_tick(start + 1900 * MS)).is_empty());
    assert!(termination.step(tick(start + 1999 * MS)).is_empty());
    assert_eq!(
        termination.step(tick(start + 2000 * MS)),
        vec![Effect::KillProcessGroup]
    );
}

#[test]
fn group_kill_resets_the_drain_window_and_output_extends_within_it() {
    let start = Instant::now();
    let mut termination = draining_since(start);
    let killed_at = start + 250 * MS;

    assert_eq!(
        termination.step(tick(killed_at)),
        vec![Effect::KillProcessGroup]
    );
    assert!(termination
        .step(output_tick(killed_at + 1900 * MS))
        .is_empty());
    assert!(termination.step(tick(killed_at + 1999 * MS)).is_empty());
    assert_eq!(
        termination.step(tick(killed_at + 2000 * MS)),
        drain_failure()
    );
}

#[test]
fn output_close_during_drain_settles_without_killing() {
    let start = Instant::now();
    let mut termination = draining_since(start);
    termination.observe(Fact::OutputClosed);

    assert!(termination.output_closed());
    assert_eq!(
        termination.step(tick(start + 5 * MS)),
        vec![Effect::BeginControlShutdown, Effect::AwaitControlShutdown]
    );
}

#[test]
fn exit_after_output_closed_skips_draining() {
    let start = Instant::now();
    let mut termination = Termination::new();
    termination.observe(Fact::OutputClosed);

    assert!(termination.output_closed());
    assert!(!termination.child_exited());
    assert!(termination.step(tick(start)).is_empty());

    termination.observe(Fact::ChildExited {
        status: exited(),
        at: start,
    });

    assert!(termination.exit_status().is_some());
    assert_eq!(
        termination.step(tick(start + MS)),
        vec![Effect::BeginControlShutdown, Effect::AwaitControlShutdown]
    );
}

#[test]
fn closing_waits_for_the_input_sequence_to_drain() {
    let start = Instant::now();
    let mut termination = draining_since(start);
    termination.observe(Fact::OutputClosed);
    let pending = Tick {
        input_sequence_pending: true,
        ..tick(start + MS)
    };

    assert!(termination.step(pending).is_empty());
    assert!(termination.step(pending).is_empty());
    assert_eq!(
        termination.step(tick(start + 2 * MS)),
        vec![Effect::BeginControlShutdown, Effect::AwaitControlShutdown]
    );
    assert_eq!(
        termination.step(tick(start + 3 * MS)),
        vec![Effect::AwaitControlShutdown]
    );
    assert!(termination.step(pending).is_empty());
    assert_eq!(
        termination.step(tick(start + 10 * MS)),
        vec![Effect::AwaitControlShutdown]
    );
}

#[test]
fn resource_escalations_name_their_limit() {
    let cases = [
        (
            ResourceLimitKind::MaxDuration,
            "max_duration_force_kill",
            "child did not exit after duration-limit termination; process group was killed",
        ),
        (
            ResourceLimitKind::IdleTimeout,
            "idle_timeout_force_kill",
            "child did not exit after idle-timeout termination; process group was killed",
        ),
        (
            ResourceLimitKind::OutputBytes,
            "output_budget_force_kill",
            "child did not exit after output-budget termination; process group was killed",
        ),
    ];

    for (kind, code, message) in cases {
        let start = Instant::now();
        let mut termination = Termination::new();
        termination.observe(Fact::ResourceLimitExceeded {
            kind,
            limit: 1_000_000,
            at: start,
        });

        assert!(termination.step(tick(start + 249 * MS)).is_empty());
        assert_eq!(
            termination.step(tick(start + 250 * MS)),
            force_kill(code, message)
        );
        assert!(termination.step(tick(start + 10_000 * MS)).is_empty());
    }
}

#[test]
fn runaway_output_force_kills_before_the_grace_expires() {
    let start = Instant::now();
    let mut termination = Termination::new();
    termination.observe(Fact::ResourceLimitExceeded {
        kind: ResourceLimitKind::OutputBytes,
        limit: 100,
        at: start,
    });

    assert!(termination
        .step(Tick {
            output_bytes: 8291,
            ..tick(start)
        })
        .is_empty());
    assert_eq!(
        termination.step(Tick {
            output_bytes: 8292,
            ..tick(start)
        }),
        force_kill(
            "output_budget_force_kill",
            "child did not exit after output-budget termination; process group was killed",
        )
    );

    let mut large = Termination::new();
    large.observe(Fact::ResourceLimitExceeded {
        kind: ResourceLimitKind::OutputBytes,
        limit: 100_000,
        at: start,
    });

    assert!(large
        .step(Tick {
            output_bytes: 199_999,
            ..tick(start)
        })
        .is_empty());
    assert_eq!(
        large
            .step(Tick {
                output_bytes: 200_000,
                ..tick(start)
            })
            .len(),
        2
    );
}

#[test]
fn only_output_budgets_run_away() {
    let start = Instant::now();
    let mut termination = Termination::new();
    termination.observe(Fact::ResourceLimitExceeded {
        kind: ResourceLimitKind::MaxDuration,
        limit: 1,
        at: start,
    });

    assert!(termination
        .step(Tick {
            output_bytes: u64::MAX,
            ..tick(start)
        })
        .is_empty());
}

#[test]
fn forwarded_signal_force_kills_after_its_grace() {
    let start = Instant::now();
    let mut termination = Termination::new();
    termination.observe(Fact::SignalForwarded { at: start });
    termination.observe(Fact::SignalForwarded {
        at: start + 1000 * MS,
    });

    assert_eq!(
        termination.signal_kill_deadline(),
        Some(start + HOST_SIGNAL_GRACE_PERIOD)
    );
    assert!(termination.step(tick(start + 1999 * MS)).is_empty());
    assert_eq!(
        termination.step(tick(start + 2000 * MS)),
        force_kill(
            "recorder_signal_force_kill",
            "child did not exit after a signal delivered to the recorder; process group was killed",
        )
    );
    assert_eq!(termination.signal_kill_deadline(), None);
}

#[test]
fn child_exit_cancels_the_signal_escalation_but_later_signals_rearm_it() {
    let start = Instant::now();
    let mut termination = Termination::new();
    termination.observe(Fact::OutputClosed);
    termination.observe(Fact::SignalForwarded { at: start });
    termination.observe(Fact::ChildExited {
        status: exited(),
        at: start + MS,
    });

    assert_eq!(termination.signal_kill_deadline(), None);

    termination.observe(Fact::SignalForwarded { at: start + 2 * MS });
    let pending = Tick {
        input_sequence_pending: true,
        ..tick(start + 2002 * MS)
    };

    assert_eq!(
        termination.step(pending),
        force_kill(
            "recorder_signal_force_kill",
            "child did not exit after a signal delivered to the recorder; process group was killed",
        )
    );
}

#[test]
fn resource_escalation_wins_over_a_due_signal_and_the_group_is_killed_once() {
    let start = Instant::now();
    let mut termination = Termination::new();
    termination.observe(Fact::SignalForwarded { at: start });
    termination.observe(Fact::ResourceLimitExceeded {
        kind: ResourceLimitKind::IdleTimeout,
        limit: 10,
        at: start + 1900 * MS,
    });

    assert_eq!(
        termination.step(tick(start + 3000 * MS)),
        force_kill(
            "idle_timeout_force_kill",
            "child did not exit after idle-timeout termination; process group was killed",
        )
    );
    assert!(termination.step(tick(start + 9000 * MS)).is_empty());

    termination.observe(Fact::SignalForwarded {
        at: start + 9000 * MS,
    });
    termination.observe(Fact::ResourceLimitExceeded {
        kind: ResourceLimitKind::MaxDuration,
        limit: 10,
        at: start + 9000 * MS,
    });

    assert!(termination.step(tick(start + 20_000 * MS)).is_empty());
}

#[test]
fn escalation_kill_before_exit_makes_drain_expiry_fatal() {
    let start = Instant::now();
    let mut termination = Termination::new();
    termination.observe(Fact::ResourceLimitExceeded {
        kind: ResourceLimitKind::MaxDuration,
        limit: 10,
        at: start,
    });

    assert_eq!(termination.step(tick(start + 250 * MS)).len(), 2);

    termination.observe(Fact::ChildExited {
        status: exited(),
        at: start + 260 * MS,
    });

    assert!(termination.step(tick(start + 509 * MS)).is_empty());
    assert_eq!(termination.step(tick(start + 510 * MS)), drain_failure());
}

#[test]
fn escalation_and_drain_expiry_in_one_tick_fail_after_the_single_kill() {
    let start = Instant::now();
    let mut termination = Termination::new();
    termination.observe(Fact::SignalForwarded { at: start });
    termination.observe(Fact::ResourceLimitExceeded {
        kind: ResourceLimitKind::OutputBytes,
        limit: 10,
        at: start,
    });
    termination.observe(Fact::ChildExited {
        status: exited(),
        at: start,
    });

    let mut expected = force_kill(
        "output_budget_force_kill",
        "child did not exit after output-budget termination; process group was killed",
    );
    expected.extend(drain_failure());

    assert_eq!(termination.step(tick(start + 250 * MS)), expected);
}
