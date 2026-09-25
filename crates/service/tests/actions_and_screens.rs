//! Tests for action requests and rich-frame text.

use std::time::Duration;
use termtrace_model::{
    protocol::control::{InputSequenceAtom, ScreenPredicate},
    terminal::screen::{
        FrameDetail, ScreenFrame, ScreenFrameData, ScreenProjectionRequest, ScreenSpanRow,
        ScreenTextRow,
    },
    terminal::state::{KeyChord, KeyModifiers, NamedKey},
};
use termtrace_service::{
    actions::{ActRequest, ActionCondition, InputAtom},
    screen::frame_text,
};
use uuid::Uuid;

#[test]
fn act_request_builds_a_composite_sequence() {
    let enter = KeyChord {
        key: NamedKey::Enter,
        modifiers: KeyModifiers::default(),
    };
    let request = ActRequest::new()
        .text("echo hello")
        .key(enter)
        .paste("pasted content")
        .bytes(vec![0x04])
        .if_state("a".repeat(64))
        .intent("test-scripted-action")
        .until_screen(
            ScreenPredicate::Contains {
                text: "hello".to_owned(),
            },
            Duration::from_secs(5),
        )
        .projection(ScreenProjectionRequest {
            detail: FrameDetail::Text,
            region: None,
        });

    assert_eq!(
        request.atoms,
        vec![
            InputAtom::Text("echo hello".to_owned()),
            InputAtom::Key(enter),
            InputAtom::Paste("pasted content".to_owned()),
            InputAtom::Bytes(vec![0x04]),
        ]
    );
    assert_eq!(request.guard.if_state, Some("a".repeat(64)));
    assert_eq!(request.intent.as_deref(), Some("test-scripted-action"));
    assert_eq!(
        request.postcondition,
        ActionCondition::Screen {
            predicate: ScreenPredicate::Contains {
                text: "hello".to_owned()
            },
            timeout: Duration::from_secs(5),
        }
    );
    assert_eq!(request.projection.detail, FrameDetail::Text);
    assert_eq!(request.wire_atoms(Uuid::now_v7()).len(), 4);
}

#[test]
fn paced_text_is_typed_one_scalar_per_atom() {
    let request = ActRequest::new()
        .text("hé")
        .key(KeyChord {
            key: NamedKey::Tab,
            modifiers: KeyModifiers::default(),
        })
        .pace(Duration::from_millis(15));
    let wire = request.wire_atoms(Uuid::now_v7());

    assert_eq!(wire.len(), 3);
    assert!(matches!(
        &wire[1],
        InputSequenceAtom::Bytes { data, .. } if data.as_bytes() == "é".as_bytes()
    ));
    assert!(matches!(
        &wire[2],
        InputSequenceAtom::Key {
            key: NamedKey::Tab,
            ..
        }
    ));
}

#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test-only fixture parsing"
)]
fn fixture_frame() -> ScreenFrame {
    let value: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/conformance/locators/locator.json"
    ))
    .expect("valid locator fixture JSON");

    return serde_json::from_value(value["frame"].clone()).expect("valid ScreenFrame");
}

#[test]
fn test_frame_text_extraction_from_variants() {
    let mut frame = fixture_frame();
    frame.data = ScreenFrameData::None;

    assert_eq!(frame_text(&frame), "");

    frame.data = ScreenFrameData::Text {
        rows: vec![
            ScreenTextRow {
                row: 0,
                soft_wrapped: false,
                wrap_continuation: false,
                text: "First row".to_owned(),
            },
            ScreenTextRow {
                row: 1,
                soft_wrapped: false,
                wrap_continuation: false,
                text: "Second row".to_owned(),
            },
        ],
    };

    assert_eq!(frame_text(&frame), "First row\nSecond row");

    frame.data = ScreenFrameData::Spans {
        rows: vec![ScreenSpanRow {
            row: 0,
            soft_wrapped: false,
            wrap_continuation: false,
            text: "Span row".to_owned(),
            spans: Vec::new(),
        }],
    };

    assert_eq!(frame_text(&frame), "Span row");
}
