use super::*;
use crate::{
    artifact::TmuxPaneSnapshot,
    client::{PaneMarks, PaneRow},
};
use termtrace_model::value::ByteString;

fn coverage(
    pane_id: &str,
    pid: u32,
    marks: PaneMarks,
    selected: bool,
    recorded: bool,
) -> PaneCoverage {
    return PaneCoverage {
        row: PaneRow {
            dead: false,
            pid: Some(pid),
            marks,
            window_name: ByteString::default(),
            current_command: ByteString::default(),
            snapshot: TmuxPaneSnapshot {
                session_id: "$0".to_owned(),
                session_name: "work".to_owned(),
                window_id: "@0".to_owned(),
                window_index: 0,
                layout: String::new(),
                window_active: true,
                zoomed: false,
                pane_id: pane_id.to_owned(),
                pane_active: true,
                copy_mode: false,
                pane_mode: String::new(),
                recording_session: None,
            },
        },
        selected,
        recorded,
    };
}

#[test]
fn unrecorded_selected_panes_are_enforced_after_the_grace_period() {
    let start = Instant::now();
    let panes = [
        coverage("%1", 11, PaneMarks::default(), true, false),
        coverage("%2", 12, PaneMarks::default(), true, true),
        coverage(
            "%3",
            13,
            PaneMarks {
                grandfathered: Some(13),
                ..PaneMarks::default()
            },
            true,
            false,
        ),
        coverage("%4", 14, PaneMarks::default(), false, false),
        coverage(
            "%5",
            15,
            PaneMarks {
                excluded: Some(15),
                ..PaneMarks::default()
            },
            false,
            false,
        ),
    ];

    let mut suspects = BTreeMap::new();
    assert_eq!(
        audit(&panes, TmuxRecordingCoverage::Strict, &mut suspects, start),
        [Enforcement::MarkExcluded {
            pane_id: "%4".to_owned(),
            pid: 14
        }]
    );
    assert_eq!(suspects.keys().collect::<Vec<_>>(), ["%1"]);

    let later = start + UNRECORDED_GRACE;
    let enforcement = audit(&panes, TmuxRecordingCoverage::Strict, &mut suspects, later);
    assert!(enforcement.contains(&Enforcement::Kill {
        pane_id: "%1".to_owned()
    }));
    assert!(suspects.is_empty());

    audit(&panes, TmuxRecordingCoverage::Warn, &mut suspects, later);

    assert!(audit(
        &panes,
        TmuxRecordingCoverage::Warn,
        &mut suspects,
        later + UNRECORDED_GRACE
    )
    .contains(&Enforcement::Warn {
        pane_id: "%1".to_owned()
    }));

    audit(&panes, TmuxRecordingCoverage::Warn, &mut suspects, later);
    audit(&[], TmuxRecordingCoverage::Warn, &mut suspects, later);

    assert!(suspects.is_empty(), "vanished panes are forgotten");
}

#[test]
fn a_dead_watchdog_is_restarted_with_backoff() {
    let now = Instant::now();
    assert!(!watchdog_restart_due(true, None, now));
    assert!(watchdog_restart_due(false, None, now));
    assert!(!watchdog_restart_due(
        false,
        Some(now),
        now + Duration::from_millis(500)
    ));
    assert!(watchdog_restart_due(
        false,
        Some(now),
        now + WATCHDOG_RESTART_BACKOFF
    ));
}
