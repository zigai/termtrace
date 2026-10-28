mod support;

use serde_json::Value;
use std::time::Duration;
use support::{enter, screen_text, shell, stop, terminal, wait_text, Launch, TIMEOUT};
use termtrace_sdk::{
    input::ActRequest,
    screen::{frame_text, ScreenPredicate},
    session::LaunchSpec,
    Error, FrameDetail, ScreenFrame, ScreenProjectionRequest, ScreenRegion, StableCaptureOptions,
    StatePoint,
};

const STYLED: &str = "printf '\\033[31mRED\\033[0m plain \\033[1;44mBOLD\\033[0m\\r\\nsecond line\\033[3;6H'; IFS= read -r _";

fn projection(detail: FrameDetail, region: Option<ScreenRegion>) -> ScreenProjectionRequest {
    return ScreenProjectionRequest { detail, region };
}

fn rows(frame: &ScreenFrame) -> Vec<Value> {
    let data = serde_json::to_value(&frame.data).expect("frame data");

    return data["rows"].as_array().expect("projected rows").clone();
}

fn color(kind: &str, value: u8) -> Value {
    return serde_json::json!({ "kind": kind, "value": value });
}

#[test]
fn spans_cells_and_cursor_reflect_what_the_app_drew() {
    let terminal = terminal("screen-styled");
    let mut session = terminal
        .launch(LaunchSpec::new(shell(STYLED)).geometry(40, 6))
        .expect("launch");
    wait_text(&session, "second line");

    let spans = session
        .frame(projection(FrameDetail::Spans, None))
        .expect("span frame");
    let first = &rows(&spans)[0];
    let red = first["spans"]
        .as_array()
        .expect("spans")
        .iter()
        .find(|span| return span["text"] == "RED")
        .expect("RED span");
    let bold = first["spans"]
        .as_array()
        .expect("spans")
        .iter()
        .find(|span| return span["text"] == "BOLD")
        .expect("BOLD span");
    assert_eq!(red["start_column"], 0);
    assert_eq!(
        red["appearance"]["style"]["foreground"],
        color("palette", 1)
    );
    assert_eq!(red["appearance"]["style"]["bold"], false);
    assert_eq!(bold["start_column"], 10);
    assert_eq!(bold["appearance"]["style"]["bold"], true);
    assert_eq!(
        bold["appearance"]["style"]["background"],
        color("palette", 4)
    );

    let cells = session
        .frame(projection(
            FrameDetail::Cells,
            Some(ScreenRegion {
                top: 0,
                left: 2,
                right: 5,
                bottom: 1,
            }),
        ))
        .expect("cell frame");
    let cells = rows(&cells)[0]["cells"].as_array().expect("cells").clone();
    let graphemes: Vec<&str> = cells
        .iter()
        .map(|cell| return cell["grapheme"].as_str().expect("grapheme"))
        .collect();
    assert_eq!(graphemes, ["D", " ", "p"]);
    assert_eq!(
        cells[0]["appearance"]["style"]["foreground"],
        color("palette", 1)
    );
    assert_eq!(
        cells[2]["appearance"]["style"]["foreground"]["kind"],
        "none"
    );
    assert_eq!(cells[2]["column"], 4);

    assert_eq!((spans.cursor.row, spans.cursor.column), (Some(2), Some(5)));
    stop(&mut session);
}

#[test]
fn bounds_return_only_the_requested_region() {
    let terminal = terminal("screen-bounds");
    let mut session = terminal
        .launch(LaunchSpec::new(shell(STYLED)).geometry(40, 6))
        .expect("launch");
    wait_text(&session, "second line");
    let region = ScreenRegion {
        top: 1,
        left: 0,
        right: 6,
        bottom: 2,
    };

    let frame = session
        .frame(projection(FrameDetail::Text, Some(region)))
        .expect("bounded frame");

    assert_eq!(frame.requested_region, region);
    assert_eq!(frame.returned_region, region);
    assert_eq!(frame_text(&frame), "second");
    let rows = rows(&frame);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["row"], 1);
    stop(&mut session);
}

#[test]
fn modes_follow_alternate_screen_and_bracketed_paste() {
    let terminal = terminal("screen-modes");
    let mut session = terminal
        .launch(LaunchSpec::new(shell(
            "printf MAIN; IFS= read -r _; printf '\\033[?1049h\\033[?2004hALT'; IFS= read -r _",
        )))
        .expect("launch");
    wait_text(&session, "MAIN");

    let before = session
        .frame(ScreenProjectionRequest::default())
        .expect("frame");
    assert!(!before.modes.alternate_screen);
    assert!(!before.modes.bracketed_paste);

    session
        .act(ActRequest::new().key(enter()).until_screen(
            ScreenPredicate::Contains {
                text: "ALT".to_owned(),
            },
            TIMEOUT,
        ))
        .expect("switch screens");
    let after = session
        .frame(ScreenProjectionRequest::default())
        .expect("frame");

    assert!(after.modes.alternate_screen);
    assert!(after.modes.bracketed_paste);
    assert!(!screen_text(&session).contains("MAIN"));
    stop(&mut session);
}

#[test]
fn provenance_names_the_live_actor_then_the_completed_source() {
    let terminal = terminal("screen-provenance");
    let mut session = terminal
        .launch(LaunchSpec::new(shell("printf READY; IFS= read -r _")))
        .expect("launch");
    wait_text(&session, "READY");

    let live = serde_json::to_value(
        session
            .frame(ScreenProjectionRequest::default())
            .expect("live frame")
            .source,
    )
    .expect("source");
    let completion = stop(&mut session);
    let completed = serde_json::to_value(
        session
            .frame(ScreenProjectionRequest::default())
            .expect("completed frame")
            .source,
    )
    .expect("source");

    let session_id = serde_json::to_value(completion.session_id).expect("session id");
    assert_eq!(live["source"], "actor", "{live}");
    assert_eq!(live["session_id"], session_id);
    assert!(live["boundaries"]["durable"]["next_event_seq"].as_u64() > Some(0));
    assert_eq!(completed["source"], "resolved", "{completed}");
    assert_eq!(completed["session_id"], session_id);
    assert_eq!(completed["source_completed"], true);
}

#[test]
fn hidden_states_need_explicit_permission() {
    let terminal = terminal("screen-hidden");
    let mut session = terminal
        .launch(LaunchSpec::new(shell(
            "printf SHOWN; sleep 0.3; printf '\\033[?2026h\\r\\033[KHIDDEN-FRAME'; sleep 0.3; printf '\\r\\033[KFINAL\\033[?2026l'; IFS= read -r _",
        )))
        .expect("launch");
    wait_text(&session, "FINAL");
    stop(&mut session);

    let hidden: Vec<u64> = session
        .semantic()
        .timeline(true, false)
        .expect("timeline")
        .iter()
        .map(|entry| return serde_json::to_value(&entry.item).expect("item"))
        .filter(|item| return item["kind"] == "semantic_state" && item["presented"] == false)
        .map(|item| return item["state_id"].as_u64().expect("state id"))
        .collect();
    let observed: Vec<_> = hidden
        .iter()
        .map(|state_id| {
            return session
                .observe(
                    StatePoint::StateId {
                        state_id: *state_id,
                    },
                    true,
                )
                .expect("hidden observation");
        })
        .collect();
    let transient = observed
        .iter()
        .find(|observation| return observation.frame.viewport_text.contains("HIDDEN-FRAME"))
        .expect("the synchronized frame is a hidden state");

    assert!(!transient.presented);
    assert!(session
        .observe(
            StatePoint::StateId {
                state_id: transient.state_id.expect("state id"),
            },
            false,
        )
        .is_err());
    assert!(session
        .observe(StatePoint::LatestPresented, false)
        .expect("latest")
        .frame
        .viewport_text
        .contains("FINAL"));
}

#[test]
fn capture_stable_waits_for_the_app_to_go_idle() {
    let terminal = terminal("screen-idle");
    let mut settling = terminal
        .launch(LaunchSpec::new(shell(
            "i=0; while [ $i -lt 6 ]; do printf 'TICK%s ' $i; i=$((i+1)); sleep 0.1; done; printf SETTLED; IFS= read -r _",
        )))
        .expect("launch");
    wait_text(&settling, "TICK0");

    let capture = settling
        .capture_stable(StableCaptureOptions {
            stable_for: Duration::from_millis(400),
            timeout: TIMEOUT,
            projection: projection(FrameDetail::Text, None),
            ..StableCaptureOptions::default()
        })
        .expect("stable capture");

    assert!(capture.quality.is_settled());
    assert!(frame_text(&capture.frame).contains("TICK5 SETTLED"));
    assert!(capture.stable_for_ns >= 400_000_000);
    stop(&mut settling);

    let mut busy = terminal
        .launch(LaunchSpec::new(shell(
            "i=0; while :; do printf '\\rBUSY%s' $i; i=$((i+1)); sleep 0.05; done",
        )))
        .expect("launch");
    wait_text(&busy, "BUSY");
    let options = StableCaptureOptions {
        stable_for: Duration::from_millis(500),
        timeout: Duration::from_millis(1500),
        ..StableCaptureOptions::default()
    };

    let error = busy
        .capture_stable(options.clone())
        .expect_err("a busy screen never settles");
    assert!(
        matches!(error, Error::StableCaptureIncomplete { .. }),
        "{error:?}"
    );
    let incomplete = busy
        .capture_stable(StableCaptureOptions {
            allow_incomplete: true,
            ..options
        })
        .expect("incomplete capture");
    assert!(!incomplete.quality.is_settled());
    stop(&mut busy);
}
