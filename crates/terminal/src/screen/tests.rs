use super::*;
use crate::ghostty_vt::GhosttyVt;
use std::time::Instant;
use termtrace_model::{
    execution::event_log::CanonicalBoundary, terminal::observation::ActorBoundaries,
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

    let empty = ScreenProjectionRequest {
        detail: FrameDetail::Spans,
        region: Some(ScreenRegion {
            top: 0,
            left: 1,
            right: 1,
            bottom: 1,
        }),
    };

    assert!(matches!(
        validate_screen_region(&empty, 80, 24),
        Err(ScreenFrameError::InvalidRegion {
            reason: InvalidRegionReason::Empty,
            ..
        })
    ));

    let out_of_bounds = ScreenProjectionRequest {
        detail: FrameDetail::Spans,
        region: Some(ScreenRegion {
            top: 0,
            left: 0,
            right: 81,
            bottom: 24,
        }),
    };

    assert!(matches!(
        validate_screen_region(&out_of_bounds, 80, 24),
        Err(ScreenFrameError::InvalidRegion {
            reason: InvalidRegionReason::OutOfBounds,
            ..
        })
    ));
}

#[test]
fn region_cell_limit_is_typed_and_never_clipped() {
    let request = ScreenProjectionRequest {
        detail: FrameDetail::Spans,
        region: Some(ScreenRegion {
            top: 0,
            left: 0,
            right: 600,
            bottom: 500,
        }),
    };
    assert_eq!(
        validate_screen_region(&request, 600, 500),
        Err(ScreenFrameError::ResourceLimit {
            limit: ScreenFrameLimitKind::CellCount,
            requested: 300_000,
            maximum: u64::from(MAX_SCREEN_FRAME_CELLS),
            detail: FrameDetail::Spans,
            region: request.region.expect("region"),
        })
    );
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

    let ScreenFrameData::Spans { rows } = &frame.data else {
        panic!("span detail");
    };

    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0]
            .spans
            .iter()
            .map(|span| return usize::from(span.cell_count))
            .sum::<usize>(),
        6
    );
    assert_eq!(rows[0].spans.last().expect("trailing span").end_column, 6);

    let mut projected: Vec<(u16, String, WideKind)> = Vec::new();
    frame.for_each_cell(|_, column, grapheme, wide_kind, _| {
        projected.push((column, grapheme.to_owned(), wide_kind));
    });

    assert_eq!(projected.len(), 12);
    assert!(projected.iter().any(|(_, text, kind)| {
        return text == "界" && *kind == WideKind::Wide;
    }));
    assert!(projected.iter().any(|(_, _, kind)| {
        return *kind == WideKind::SpacerTail;
    }));
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
fn serialized_byte_limit_returns_bounded_error() {
    let mut state = semantic_fixture();
    state.grid[0].cells[0].grapheme =
        "x".repeat(usize::try_from(MAX_SCREEN_FRAME_JSON_BYTES).expect("byte limit") + 1);
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
    state.grid[0].cells[0].grapheme = "\u{0001}".repeat(1_500_000);
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
    state.grid[0].cells[1].selected = true;
    let frame = project_screen_frame(
        &state,
        &ScreenProjectionRequest::default(),
        source(1),
        true,
        CaptureQuality::Live,
    )
    .expect("span frame");
    let ScreenFrameData::Spans { rows } = frame.data else {
        panic!("span detail");
    };

    assert!(rows[0].spans.iter().any(|span| {
        return span.start_column == 1 && span.end_column == 2 && span.appearance.selected;
    }));
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
