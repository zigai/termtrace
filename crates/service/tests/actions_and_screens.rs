//! Tests for ActionBuilder, Screen queries, and live control capabilities.

use std::time::Duration;
use termtrace_model::{
    protocol::control::ScreenPredicate,
    terminal::screen::{
        FrameDetail, ScreenFrame, ScreenFrameData, ScreenProjectionRequest, ScreenSpanRow,
        ScreenTextRow,
    },
    terminal::state::{KeyModifiers, NamedKey},
};
use termtrace_service::{
    actions::{ActionBuilder, ActionCondition, InputAtom},
    screen::frame_text,
};

#[test]
fn test_action_builder_constructs_composite_sequence() {
    let (atoms, options) = ActionBuilder::new()
        .text("echo hello")
        .key(NamedKey::Enter, KeyModifiers::default())
        .paste("pasted content")
        .bytes(vec![0x04]) // Ctrl-D
        .if_state("abc".repeat(21) + "a")
        .intent("test-scripted-action")
        .pace(Duration::from_millis(15))
        .until_screen(
            ScreenPredicate::Contains {
                text: "hello".to_owned(),
            },
            Duration::from_secs(5),
        )
        .projection(ScreenProjectionRequest {
            detail: FrameDetail::Text,
            region: None,
        })
        .build();
    assert_eq!(atoms.len(), 4);
    assert_eq!(atoms[0], InputAtom::Text("echo hello".to_owned()));
    assert_eq!(
        atoms[1],
        InputAtom::Key {
            key: NamedKey::Enter,
            modifiers: KeyModifiers::default()
        }
    );
    assert_eq!(atoms[2], InputAtom::Paste("pasted content".to_owned()));
    assert_eq!(atoms[3], InputAtom::Bytes(vec![0x04]));
    assert_eq!(
        options.if_state.as_deref(),
        Some("abcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabca")
    );
    assert_eq!(
        options.caller_intent.as_deref(),
        Some("test-scripted-action")
    );
    assert_eq!(options.pace, Duration::from_millis(15));
    assert_eq!(
        options.postcondition,
        ActionCondition::Screen {
            predicate: ScreenPredicate::Contains {
                text: "hello".to_owned()
            },
            timeout: Duration::from_secs(5),
        }
    );
    assert_eq!(options.projection.detail, FrameDetail::Text);
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
