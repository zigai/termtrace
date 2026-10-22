use super::*;
use crate::ghostty_vt::GhosttyVt;
use std::{collections::BTreeSet, sync::Arc, time::Instant};
use termtrace_model::{
    execution::event_log::CanonicalBoundary,
    terminal::{observation::ActorBoundaries, state::DirtyState},
    value::ByteString,
};

fn source(boundary: u64) -> ScreenFrameSource {
    let position = CanonicalBoundary {
        next_event_seq: boundary,
        event_hash: format!("hash-{boundary}"),
        output_bytes: boundary,
    };

    return ScreenFrameSource::Actor {
        session_id: Default::default(),
        boundaries: ActorBoundaries {
            durable: position.clone(),
            observed: position,
        },
        observed_monotonic_ns: boundary,
    };
}

fn semantic_fixture() -> SemanticState {
    let mut terminal = GhosttyVt::new(6, 2, 0).expect("create terminal");
    terminal.write(b"\x1b[38;2;1;2;3mA");
    terminal.write("界".as_bytes());

    return terminal.snapshot().expect("semantic snapshot");
}

fn checked_state_hash(state: &SemanticState) -> String {
    let hash = semantic_state_hash(state).expect("semantic identity");
    let rows = state
        .grid
        .iter()
        .map(|row| return hash_semantic_row(row).expect("row identity"))
        .collect::<Vec<_>>();
    assert_eq!(
        semantic_state_hash_from_row_hashes(state, &rows).expect("cached semantic identity"),
        hash
    );

    return hash;
}

#[test]
fn semantic_identity_distinguishes_cell_and_active_styles() {
    let styles = [
        "\x1b[1m",
        "\x1b[3m",
        "\x1b[2m",
        "\x1b[5m",
        "\x1b[7m",
        "\x1b[8m",
        "\x1b[9m",
        "\x1b[53m",
        "\x1b[4m",
        "\x1b[31m",
        "\x1b[41m",
        "\x1b[58;5;1m",
        "\x1b[38;2;1;2;3m",
        "\x1b[48;2;4;5;6m",
        "\x1b[58;2;7;8;9m",
    ];

    let mut terminal = GhosttyVt::new(6, 2, 0).expect("plain terminal");
    let mut active_hashes = BTreeSet::from([checked_state_hash(
        &terminal.snapshot().expect("plain active style"),
    )]);
    terminal.write(b"X");
    let mut cell_hashes = BTreeSet::from([checked_state_hash(
        &terminal.snapshot().expect("plain cell"),
    )]);

    for sequence in styles {
        let mut terminal = GhosttyVt::new(6, 2, 0).expect("styled terminal");
        terminal.write(sequence.as_bytes());
        let active = terminal.snapshot().expect("active style");
        assert!(
            active_hashes.insert(checked_state_hash(&active)),
            "active {sequence:?}"
        );

        terminal.write(b"X\x1b[0m");
        let cell = terminal.snapshot().expect("styled cell and reset pen");
        assert!(
            cell_hashes.insert(checked_state_hash(&cell)),
            "cell {sequence:?}"
        );
    }
}

#[test]
fn semantic_identity_excludes_only_transient_dirtiness() {
    let state = semantic_fixture();
    let hash = checked_state_hash(&state);
    let mut changed = state.clone();
    changed.dirty = DirtyState::Clean;
    changed.dirty_rows.clear();

    for row in &mut changed.grid {
        row.dirty = false;
    }

    assert_eq!(checked_state_hash(&changed), hash);

    changed.metadata.title = ByteString::from(b"window".as_slice());

    assert_ne!(checked_state_hash(&changed), hash);

    changed = state.clone();
    changed.metadata.working_directory = ByteString::from(b"file:///work".as_slice());

    assert_ne!(checked_state_hash(&changed), hash);

    changed = state.clone();
    changed.cursor.visible = !changed.cursor.visible;

    assert_ne!(checked_state_hash(&changed), hash);

    changed = state.clone();
    changed.modes.bracketed_paste = !changed.modes.bracketed_paste;

    assert_ne!(checked_state_hash(&changed), hash);

    changed = state;
    changed.colors.background.r ^= 1;

    assert_ne!(checked_state_hash(&changed), hash);
}

#[test]
fn exact_cells_require_strict_explicit_region() {
    let missing = ScreenProjectionRequest {
        detail: FrameDetail::Cells,
        region: None,
    };
    assert!(matches!(
        validate_screen_region(&missing, 80, 24),
        Err(ScreenFrameError::InvalidRegion {
            reason: InvalidRegionReason::CellsRequireExplicitRegion,
            ..
        })
    ));

    for (region, reason) in [
        (
            ScreenRegion {
                top: 0,
                left: 2,
                right: 1,
                bottom: 1,
            },
            InvalidRegionReason::Reversed,
        ),
        (
            ScreenRegion {
                top: 2,
                left: 0,
                right: 1,
                bottom: 1,
            },
            InvalidRegionReason::Reversed,
        ),
        (
            ScreenRegion {
                top: 0,
                left: 1,
                right: 1,
                bottom: 1,
            },
            InvalidRegionReason::Empty,
        ),
        (
            ScreenRegion {
                top: 1,
                left: 0,
                right: 1,
                bottom: 1,
            },
            InvalidRegionReason::Empty,
        ),
        (
            ScreenRegion {
                top: 0,
                left: 0,
                right: 81,
                bottom: 24,
            },
            InvalidRegionReason::OutOfBounds,
        ),
        (
            ScreenRegion {
                top: 0,
                left: 0,
                right: 80,
                bottom: 25,
            },
            InvalidRegionReason::OutOfBounds,
        ),
    ] {
        let request = ScreenProjectionRequest {
            detail: FrameDetail::Spans,
            region: Some(region),
        };
        assert_eq!(
            validate_screen_region(&request, 80, 24),
            Err(ScreenFrameError::InvalidRegion {
                region: Some(region),
                cols: 80,
                rows: 24,
                reason,
            }),
            "region {region:?}"
        );
    }
}

#[test]
fn region_cell_limit_is_typed_and_never_clipped() {
    for (cols, rows, requested) in [(5, 52_429, 262_145), (65_535, 65_535, 4_294_836_225)] {
        let region = ScreenRegion {
            top: 0,
            left: 0,
            right: cols,
            bottom: rows,
        };
        let request = ScreenProjectionRequest {
            detail: FrameDetail::Spans,
            region: Some(region),
        };
        assert_eq!(
            validate_screen_region(&request, cols, rows),
            Err(ScreenFrameError::ResourceLimit {
                limit: ScreenFrameLimitKind::CellCount,
                requested,
                maximum: u64::from(MAX_SCREEN_FRAME_CELLS),
                detail: FrameDetail::Spans,
                region,
            })
        );
    }
}

#[test]
fn region_at_the_cell_limit_is_accepted_without_clipping() {
    let region = ScreenRegion {
        top: 0,
        left: 0,
        right: 512,
        bottom: 512,
    };
    let request = ScreenProjectionRequest {
        detail: FrameDetail::Cells,
        region: Some(region),
    };
    assert_eq!(validate_screen_region(&request, 512, 512), Ok(region));
}

#[test]
fn validate_screen_region_4k_grid_succeeds() {
    let request = ScreenProjectionRequest {
        detail: FrameDetail::Spans,
        region: Some(ScreenRegion {
            top: 0,
            left: 0,
            right: 548,
            bottom: 135,
        }),
    };
    let region = validate_screen_region(&request, 548, 135).expect("4k region succeeds");
    assert_eq!(region.width(), 548);
    assert_eq!(region.height(), 135);
}

#[test]
fn spans_expand_losslessly_and_keep_trailing_blank_cells() {
    let state = semantic_fixture();
    let request = ScreenProjectionRequest::default();
    let frame = project_screen_frame(&state, &request, source(1), true, CaptureQuality::Live)
        .expect("project spans");

    let ScreenFrameContent::Spans { rows } = &frame.data else {
        panic!("span detail");
    };

    assert_eq!(rows.len(), 2);

    let first = rows.first().expect("first row");
    assert_eq!(first.text, "A界   ");
    assert_eq!(
        first
            .spans
            .iter()
            .map(|span| return span.text.as_str())
            .collect::<String>(),
        "A界   "
    );
    assert_eq!(first.spans.last().expect("trailing span").end_column, 6);

    let mut projected = Vec::new();
    frame.for_each_cell(|row, column, grapheme, wide_kind, _| {
        projected.push((row.row, column, grapheme.to_owned(), wide_kind));
    });

    assert_eq!(
        projected,
        vec![
            (0, 0, "A".to_owned(), WideKind::Narrow),
            (0, 1, "界".to_owned(), WideKind::Wide),
            (0, 2, String::new(), WideKind::SpacerTail),
            (0, 3, String::new(), WideKind::Narrow),
            (0, 4, String::new(), WideKind::Narrow),
            (0, 5, String::new(), WideKind::Narrow),
            (1, 0, String::new(), WideKind::Narrow),
            (1, 1, String::new(), WideKind::Narrow),
            (1, 2, String::new(), WideKind::Narrow),
            (1, 3, String::new(), WideKind::Narrow),
            (1, 4, String::new(), WideKind::Narrow),
            (1, 5, String::new(), WideKind::Narrow),
        ]
    );
}

#[test]
fn projection_hash_binds_detail_region_and_source_occurrence() {
    let state = semantic_fixture();
    let spans = project_screen_frame(
        &state,
        &ScreenProjectionRequest::default(),
        source(1),
        true,
        CaptureQuality::Live,
    )
    .expect("span frame");

    let cells = project_screen_frame(
        &state,
        &ScreenProjectionRequest {
            detail: FrameDetail::Cells,
            region: Some(ScreenRegion {
                top: 0,
                left: 0,
                right: 3,
                bottom: 1,
            }),
        },
        source(1),
        true,
        CaptureQuality::Live,
    )
    .expect("cell frame");

    let moved = project_screen_frame(
        &state,
        &ScreenProjectionRequest::default(),
        source(2),
        true,
        CaptureQuality::Live,
    )
    .expect("different occurrence");

    assert_eq!(spans.state_hash, cells.state_hash);
    assert_ne!(spans.frame_hash, cells.frame_hash);
    assert_ne!(spans.frame_hash, moved.frame_hash);
    assert_eq!(
        screen_frame_hash(&spans).expect("projection identity"),
        spans.frame_hash
    );

    let mut reidentified = spans.clone();
    reidentified.frame_hash.clear();

    assert_eq!(
        screen_frame_hash(&reidentified).expect("identity excludes itself"),
        spans.frame_hash
    );
}

#[test]
fn snapshot_normalization_omits_volatile_source_identity() {
    let state = semantic_fixture();
    let first = project_screen_frame(
        &state,
        &ScreenProjectionRequest::default(),
        source(1),
        true,
        CaptureQuality::Live,
    )
    .expect("first");

    let second = project_screen_frame(
        &state,
        &ScreenProjectionRequest::default(),
        source(2),
        true,
        CaptureQuality::Live,
    )
    .expect("second");
    assert_eq!(
        normalize_screen_frame_spans(&first, ScreenSnapshotOptions::default())
            .expect("normalize first"),
        normalize_screen_frame_spans(&second, ScreenSnapshotOptions::default())
            .expect("normalize second")
    );
}

#[test]
fn byte_limit_rejections_report_a_request_above_the_permitted_amount() {
    let mut state = semantic_fixture();
    Arc::make_mut(&mut state.grid[0].cells)[0].grapheme = "x"
        .repeat(usize::try_from(DEFAULT_MAX_SCREEN_FRAME_JSON_BYTES).expect("byte limit"))
        .into();

    let request = ScreenProjectionRequest {
        detail: FrameDetail::Text,
        region: Some(ScreenRegion {
            top: 0,
            left: 0,
            right: 1,
            bottom: 1,
        }),
    };
    let error = project_screen_frame(&state, &request, source(1), true, CaptureQuality::Live)
        .expect_err("JSON envelope exceeds the byte budget");
    let ScreenFrameError::ResourceLimit {
        requested, maximum, ..
    } = error
    else {
        panic!("expected a byte resource limit");
    };

    assert!(
        requested > maximum,
        "requested {requested}, permitted {maximum}"
    );
}

#[test]
fn serialized_byte_limit_returns_bounded_error() {
    let mut state = semantic_fixture();
    Arc::make_mut(&mut state.grid[0].cells)[0].grapheme = "x"
        .repeat(usize::try_from(MAX_SCREEN_FRAME_JSON_BYTES).expect("byte limit") + 1)
        .into();
    let error = project_screen_frame(
        &state,
        &ScreenProjectionRequest::default(),
        source(1),
        true,
        CaptureQuality::Live,
    )
    .expect_err("oversized projection");
    assert!(matches!(
        error,
        ScreenFrameError::ResourceLimit {
            limit: ScreenFrameLimitKind::SerializedBytes,
            requested,
            maximum: MAX_SCREEN_FRAME_JSON_BYTES,
            detail: FrameDetail::Spans,
            ..
        } if requested > MAX_SCREEN_FRAME_JSON_BYTES
    ));
}

#[test]
fn escaped_projection_strings_are_bounded_before_projection_clones() {
    let mut state = semantic_fixture();
    Arc::make_mut(&mut state.grid[0].cells)[0].grapheme = "\u{0001}".repeat(1_500_000).into();
    let region =
        validate_screen_region(&ScreenProjectionRequest::default(), state.cols, state.rows)
            .expect("region");
    assert!(matches!(
        validate_projection_clone_bytes(&state, region, FrameDetail::Spans),
        Err(ScreenFrameError::ResourceLimit {
            limit: ScreenFrameLimitKind::SerializedBytes,
            requested,
            maximum: MAX_SCREEN_FRAME_JSON_BYTES,
            ..
        }) if requested > MAX_SCREEN_FRAME_JSON_BYTES
    ));
}

#[test]
fn span_boundaries_preserve_complete_selection_appearance() {
    let mut state = semantic_fixture();
    Arc::make_mut(&mut state.grid[0].cells)[1].selected = true;
    let frame = project_screen_frame(
        &state,
        &ScreenProjectionRequest::default(),
        source(1),
        true,
        CaptureQuality::Live,
    )
    .expect("span frame");
    let ScreenFrameContent::Spans { rows } = frame.data else {
        panic!("span detail");
    };

    assert!(rows[0].spans.iter().any(|span| {
        return span.start_column == 1 && span.end_column == 2 && span.appearance.selected;
    }));
}

#[test]
fn inconsistent_semantic_grids_are_rejected_with_their_invariant_category() {
    let state = semantic_fixture();
    let mut missing_row = state.clone();
    missing_row.grid.pop();

    let mut short_palette = state.clone();
    short_palette.colors.palette.pop();

    let mut wrong_row = state.clone();
    wrong_row.grid.first_mut().expect("first row").row = 1;
    let mut missing_cell = state.clone();
    let row = missing_cell.grid.first_mut().expect("first row");
    row.cells = row.cells[..row.cells.len() - 1].into();

    let mut wrong_column = state;
    Arc::make_mut(&mut wrong_column.grid.first_mut().expect("first row").cells)
        .first_mut()
        .expect("first cell")
        .column = 1;

    for (invalid, reason) in [
        (missing_row, SemanticStateFailure::RowCount),
        (short_palette, SemanticStateFailure::PaletteSize),
        (wrong_row, SemanticStateFailure::RowCoordinate),
        (missing_cell, SemanticStateFailure::CellCount),
        (wrong_column, SemanticStateFailure::CellCoordinate),
    ] {
        assert_eq!(
            project_screen_frame(
                &invalid,
                &ScreenProjectionRequest::default(),
                source(1),
                true,
                CaptureQuality::Live,
            ),
            Err(ScreenFrameError::InconsistentSemanticState { reason }),
            "invalid {reason:?}"
        );
    }
}

#[test]
fn cropped_projection_text_keeps_absolute_rows_blanks_and_wide_cell_roles() {
    let state = semantic_fixture();
    let region = ScreenRegion {
        top: 0,
        left: 1,
        right: 5,
        bottom: 2,
    };

    for detail in [FrameDetail::Text, FrameDetail::Spans, FrameDetail::Cells] {
        let request = ScreenProjectionRequest {
            detail,
            region: Some(region),
        };
        let frame = project_screen_frame(&state, &request, source(1), true, CaptureQuality::Live)
            .expect("cropped frame");
        assert_eq!(frame.geometry, ScreenGeometry { cols: 6, rows: 2 });
        assert_eq!(frame.requested_region, region);
        assert_eq!(frame.returned_region, region);
        let rows: Vec<_> = match frame.data {
            ScreenFrameContent::Text { rows } => rows
                .into_iter()
                .map(|row| return (row.row, row.soft_wrapped, row.wrap_continuation, row.text))
                .collect(),
            ScreenFrameContent::Spans { rows } => rows
                .into_iter()
                .map(|row| return (row.row, row.soft_wrapped, row.wrap_continuation, row.text))
                .collect(),
            ScreenFrameContent::Cells { rows } => rows
                .into_iter()
                .map(|row| return (row.row, row.soft_wrapped, row.wrap_continuation, row.text))
                .collect(),
            ScreenFrameContent::None => panic!("requested row payload"),
        };
        assert_eq!(
            rows,
            vec![
                (0, false, false, "界  ".to_owned()),
                (1, false, false, "    ".to_owned()),
            ],
            "detail {detail:?}"
        );
    }
}

#[test]
fn viewport_text_keeps_leading_spaces_blank_rows_and_non_ascii_spaces() {
    let mut terminal = GhosttyVt::new(6, 3, 0).expect("terminal");
    terminal.write(" A  \r\n\r\n\u{a0} ".as_bytes());
    let state = terminal.snapshot().expect("snapshot");
    assert_eq!(semantic_viewport_text(&state), " A\n\n\u{a0}");
}

#[test]
fn snapshot_normalizers_preserve_payload_and_only_requested_optional_fields() {
    type Normalizer =
        fn(&ScreenFrame, ScreenSnapshotOptions) -> Result<serde_json::Value, ScreenFrameError>;

    let state = semantic_fixture();
    let region = ScreenRegion {
        top: 0,
        left: 0,
        right: 1,
        bottom: 1,
    };

    let appearance = serde_json::json!({
        "style": {
            "foreground": {"kind": "rgb", "value": {"r": 1, "g": 2, "b": 3}},
            "background": {"kind": "none"},
            "underline_color": {"kind": "none"},
            "bold": false, "italic": false, "faint": false, "blink": false,
            "inverse": false, "invisible": false, "strikethrough": false,
            "overline": false, "underline": 0
        },
        "resolved_foreground": {"r": 1, "g": 2, "b": 3},
        "resolved_background": null,
        "selected": false
    });

    let cases: [(FrameDetail, Normalizer, serde_json::Value); 3] = [
        (
            FrameDetail::Text,
            normalize_screen_frame_text,
            serde_json::json!([{
                "row": 0, "soft_wrapped": false, "wrap_continuation": false, "text": "A"
            }]),
        ),
        (
            FrameDetail::Spans,
            normalize_screen_frame_spans,
            serde_json::json!([{
                "row": 0, "soft_wrapped": false, "wrap_continuation": false, "text": "A",
                "spans": [{
                    "start_column": 0, "end_column": 1, "cell_count": 1, "text": "A",
                    "appearance": appearance,
                    "cells": [{"grapheme": "A", "wide_kind": "narrow"}]
                }]
            }]),
        ),
        (
            FrameDetail::Cells,
            normalize_screen_frame_cells,
            serde_json::json!([{
                "row": 0, "soft_wrapped": false, "wrap_continuation": false, "text": "A",
                "cells": [{
                    "row": 0, "column": 0, "grapheme": "A", "wide_kind": "narrow",
                    "appearance": appearance
                }]
            }]),
        ),
    ];

    for (detail, normalize, rows) in cases {
        let frame = project_screen_frame(
            &state,
            &ScreenProjectionRequest {
                detail,
                region: Some(region),
            },
            source(1),
            true,
            CaptureQuality::Live,
        )
        .expect("frame");

        for (options, optional_fields) in [
            (ScreenSnapshotOptions::default(), serde_json::json!({})),
            (
                ScreenSnapshotOptions {
                    include_cursor: true,
                    include_modes: false,
                },
                serde_json::json!({"cursor": state.cursor}),
            ),
            (
                ScreenSnapshotOptions {
                    include_cursor: false,
                    include_modes: true,
                },
                serde_json::json!({"modes": state.modes}),
            ),
            (
                ScreenSnapshotOptions {
                    include_cursor: true,
                    include_modes: true,
                },
                serde_json::json!({"cursor": state.cursor, "modes": state.modes}),
            ),
        ] {
            let mut expected = serde_json::json!({
                "version": 1,
                "geometry": {"cols": 6, "rows": 2},
                "requested_region": {"top": 0, "left": 0, "right": 1, "bottom": 1},
                "returned_region": {"top": 0, "left": 0, "right": 1, "bottom": 1},
                "detail": detail,
                "colors": state.colors,
                "rows": rows,
            });
            expected.as_object_mut().expect("snapshot object").extend(
                optional_fields
                    .as_object()
                    .expect("optional fields")
                    .clone(),
            );

            assert_eq!(
                normalize(&frame, options).expect("normalized snapshot"),
                expected,
                "detail {detail:?}, options {options:?}"
            );
        }
    }
}

#[test]
fn snapshot_normalizers_reject_frames_without_their_payload() {
    let state = semantic_fixture();
    let frame = project_screen_frame(
        &state,
        &ScreenProjectionRequest {
            detail: FrameDetail::None,
            region: None,
        },
        source(1),
        true,
        CaptureQuality::Live,
    )
    .expect("metadata-only frame");
    assert_eq!(
        normalize_screen_frame_text(&frame, ScreenSnapshotOptions::default()),
        Err(ScreenFrameError::Serialization)
    );
    assert_eq!(
        normalize_screen_frame_spans(&frame, ScreenSnapshotOptions::default()),
        Err(ScreenFrameError::Serialization)
    );
    assert_eq!(
        normalize_screen_frame_cells(&frame, ScreenSnapshotOptions::default()),
        Err(ScreenFrameError::Serialization)
    );
}

#[test]
fn serialized_frame_budget_accepts_the_limit_and_rejects_one_extra_byte() {
    let state = semantic_fixture();
    let request = ScreenProjectionRequest {
        detail: FrameDetail::None,
        region: None,
    };

    let mut provenance = source(1);
    let ScreenFrameSource::Actor { boundaries, .. } = &mut provenance else {
        panic!("actor source");
    };
    boundaries.durable.event_hash.clear();

    let baseline = project_screen_frame(
        &state,
        &request,
        provenance.clone(),
        true,
        CaptureQuality::Live,
    )
    .expect("baseline frame");

    let baseline_bytes = serde_json::to_vec(&baseline)
        .expect("serialized baseline")
        .len();

    let maximum = usize::try_from(MAX_SCREEN_FRAME_JSON_BYTES).expect("byte limit");
    let padding = "x".repeat(maximum - baseline_bytes);
    let ScreenFrameSource::Actor { boundaries, .. } = &mut provenance else {
        panic!("actor source");
    };
    boundaries.durable.event_hash = padding;

    let frame = project_screen_frame(
        &state,
        &request,
        provenance.clone(),
        true,
        CaptureQuality::Live,
    )
    .expect("exact byte budget is allowed");
    assert_eq!(
        serde_json::to_vec(&frame).expect("serialized frame").len(),
        maximum
    );

    let ScreenFrameSource::Actor { boundaries, .. } = &mut provenance else {
        panic!("actor source");
    };
    boundaries.durable.event_hash.push('x');

    assert_eq!(
        project_screen_frame(&state, &request, provenance, true, CaptureQuality::Live),
        Err(ScreenFrameError::ResourceLimit {
            limit: ScreenFrameLimitKind::SerializedBytes,
            requested: MAX_SCREEN_FRAME_JSON_BYTES + 1,
            maximum: MAX_SCREEN_FRAME_JSON_BYTES,
            detail: FrameDetail::None,
            region: ScreenRegion {
                top: 0,
                left: 0,
                right: 6,
                bottom: 2
            },
        })
    );
}

#[test]
fn short_json_escapes_within_the_budget_are_not_rejected_as_unicode_escapes() {
    let mut state = semantic_fixture();
    let payload = "\"\\\n\r\t\u{0008}\u{000c}".repeat(400_000);
    Arc::make_mut(&mut state.grid.first_mut().expect("first row").cells)
        .first_mut()
        .expect("first cell")
        .grapheme = payload.clone().into();
    let frame = project_screen_frame(
        &state,
        &ScreenProjectionRequest {
            detail: FrameDetail::Text,
            region: None,
        },
        source(1),
        true,
        CaptureQuality::Live,
    )
    .expect("short JSON escapes fit the byte budget");
    let ScreenFrameContent::Text { rows } = frame.data else {
        panic!("text frame");
    };

    assert_eq!(
        rows.first().expect("first row").text,
        format!("{payload}界   ")
    );
}

#[test]
fn large_regions_scale_the_byte_budget_but_a_small_crop_does_not() {
    let mut terminal = GhosttyVt::new(512, 256, 0).expect("large terminal");
    let mut state = terminal.snapshot().expect("large snapshot");
    Arc::make_mut(&mut state.grid.first_mut().expect("first row").cells)
        .first_mut()
        .expect("first cell")
        .grapheme = "x".repeat(9_000_000).into();

    let full = ScreenProjectionRequest {
        detail: FrameDetail::Text,
        region: None,
    };
    let frame = project_screen_frame(&state, &full, source(1), true, CaptureQuality::Live)
        .expect("large region has a scaled budget");

    let bytes = serde_json::to_vec(&frame)
        .expect("serialize large frame")
        .len();
    assert!(bytes > 8_388_608 && bytes <= 16_777_216);

    let region = ScreenRegion {
        top: 0,
        left: 0,
        right: 1,
        bottom: 1,
    };
    let crop = ScreenProjectionRequest {
        detail: FrameDetail::Text,
        region: Some(region),
    };

    assert_eq!(
        project_screen_frame(&state, &crop, source(1), true, CaptureQuality::Live),
        Err(ScreenFrameError::ResourceLimit {
            limit: ScreenFrameLimitKind::SerializedBytes,
            requested: 9_000_000,
            maximum: MAX_SCREEN_FRAME_JSON_BYTES,
            detail: FrameDetail::Text,
            region,
        })
    );
}

#[test]
#[ignore = "manual rich-frame payload and projection benchmark"]
fn common_viewport_projection_metrics() {
    for (cols, rows) in [(80, 24), (160, 45)] {
        let mut terminal = GhosttyVt::new(cols, rows, 0).expect("create benchmark terminal");
        let state = terminal.snapshot().expect("benchmark semantic snapshot");

        for detail in [FrameDetail::Spans, FrameDetail::Cells] {
            let request = ScreenProjectionRequest {
                detail,
                region: (detail == FrameDetail::Cells).then_some(ScreenRegion {
                    top: 0,
                    left: 0,
                    right: cols,
                    bottom: rows,
                }),
            };

            let started = Instant::now();
            let result =
                project_screen_frame(&state, &request, source(1), true, CaptureQuality::Live);

            let elapsed = started.elapsed();

            match result {
                Ok(frame) => {
                    let bytes = serde_json::to_vec(&frame)
                        .expect("serialize benchmark projection")
                        .len();
                    assert!(
                        u64::try_from(bytes).unwrap_or(u64::MAX) <= MAX_SCREEN_FRAME_JSON_BYTES
                    );
                    println!(
                        "screen_frame detail={detail:?} cols={cols} rows={rows} \
                             status=ok payload_bytes={bytes} projection_us={}",
                        elapsed.as_micros()
                    );
                }
                Err(ScreenFrameError::ResourceLimit {
                    limit: ScreenFrameLimitKind::SerializedBytes,
                    requested,
                    maximum,
                    ..
                }) => {
                    assert!(requested > maximum);
                    println!(
                        "screen_frame detail={detail:?} cols={cols} rows={rows} \
                             status=serialized_limit payload_bytes={requested} \
                             maximum={maximum} projection_us={}",
                        elapsed.as_micros()
                    );
                }
                Err(error) => {
                    panic!("unexpected benchmark projection error: {error}");
                }
            }
        }
    }
}

#[test]
fn semantic_viewport_text_preserves_wide_characters_without_spacer_tail_spaces() {
    let mut terminal = crate::GhosttyVt::new(10, 1, 0).expect("terminal");
    terminal.write("世界".as_bytes());
    let state = terminal.snapshot().expect("snapshot");
    let text = semantic_viewport_text(&state);
    assert_eq!(text, "世界");
}
