use super::*;
use serde_json::json;
use termtrace_model::{
    terminal::observation::CaptureQuality,
    terminal::screen::{ScreenFrameSource, SpanCell},
};

fn fixture_frame() -> ScreenFrame {
    let value: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../fixtures/conformance/locators/locator.json"
    ))
    .expect("locator fixture");
    return serde_json::from_value(value.get("frame").cloned().expect("frame"))
        .expect("screen frame");
}

#[test]
fn empty_and_zero_width_text_queries_are_rejected() {
    assert!(matches!(
        LocatorSpec::text(""),
        Err(QueryError::InvalidQuery { .. })
    ));

    for pattern in ["a*", r"\b", r"(?m)^\b", r"(?:x{0})\B"] {
        assert!(
            matches!(
                LocatorSpec::native_regex(pattern),
                Err(QueryError::InvalidQuery { .. })
            ),
            "{pattern}"
        );
    }

    assert!(LocatorSpec::native_regex("a+").is_ok());
}

#[test]
fn conflicting_composition_is_rejected_without_observation() {
    let first = LocatorSpec::text("first").unwrap_or_default();
    let second = LocatorSpec::exact("second").unwrap_or_default();
    assert!(matches!(
        first.and(second),
        Err(QueryError::InvalidQuery { .. })
    ));

    let first_row = LocatorSpec::default().in_row(1);
    let second_row = LocatorSpec::default().in_row(2);
    let compiled = Regex::new("ready").expect("valid regex");
    assert!(LocatorSpec::regex(compiled).is_ok());
    assert!(matches!(
        first_row.and(second_row),
        Err(QueryError::InvalidQuery { .. })
    ));

    let row = LocatorSpec::default().in_row(0);
    let disjoint_region = LocatorSpec::default()
        .in_region(ScreenRegion {
            top: 1,
            left: 0,
            right: 2,
            bottom: 2,
        })
        .expect("region");
    assert!(matches!(
        row.and(disjoint_region),
        Err(QueryError::InvalidQuery { .. })
    ));

    let direct = LocatorSpec::default()
        .in_region(ScreenRegion {
            top: 1,
            left: 0,
            right: 2,
            bottom: 2,
        })
        .expect("region")
        .in_row(0);
    assert!(matches!(
        fixture_frame().query(direct),
        Err(QueryError::InvalidQuery { .. })
    ));
}

#[test]
fn spacer_heads_remain_queryable_at_wrap_edges() {
    let mut frame = fixture_frame();
    let ScreenFrameContent::Spans { rows } = &mut frame.data else {
        panic!("spans frame");
    };
    let span = rows[0].spans.first_mut().expect("row span");
    let spacer = span.cells.last_mut().expect("last cell");
    spacer.grapheme.clear();
    spacer.wide_kind = WideKind::SpacerHead;

    let row_match = frame
        .query(LocatorSpec::default().in_row(0))
        .expect("row query")
        .single()
        .expect("single row match")
        .clone();
    assert_eq!(
        row_match.ranges.as_ref(),
        &[MatchRange {
            row: 0,
            start_column: 0,
            end_column: 12,
        }]
    );

    let region_match = frame
        .query(
            LocatorSpec::default()
                .in_region(ScreenRegion {
                    top: 0,
                    left: 11,
                    right: 12,
                    bottom: 1,
                })
                .expect("region"),
        )
        .expect("region query")
        .single()
        .expect("single spacer match")
        .clone();

    assert_eq!(region_match.ranges[0].start_column, 11);
    assert_eq!(region_match.ranges[0].end_column, 12);
}

#[test]
fn overlapping_matches_stop_at_aggregate_work_budget() {
    let mut frame = fixture_frame();
    frame.geometry.cols = 16_384;
    frame.geometry.rows = 1;
    frame.requested_region = ScreenRegion {
        top: 0,
        left: 0,
        right: 16_384,
        bottom: 1,
    };
    frame.returned_region = frame.requested_region;
    let ScreenFrameContent::Spans { rows } = &mut frame.data else {
        panic!("spans frame");
    };
    rows.truncate(1);
    rows[0].text = "a".repeat(16_384);
    let row_text = rows[0].text.clone();
    let span = rows[0].spans.first_mut().expect("row span");
    span.start_column = 0;
    span.end_column = 16_384;
    span.cell_count = 16_384;
    span.text = row_text;
    span.cells = (0..16_384)
        .map(|_| {
            return SpanCell {
                grapheme: "a".to_owned(),
                wide_kind: WideKind::Narrow,
            };
        })
        .collect();

    rows[0].spans.truncate(1);

    let error = frame
        .query(LocatorSpec::text("a".repeat(8_192)).expect("locator"))
        .expect_err("aggregate budget");
    assert!(matches!(error, QueryError::ResourceLimit { .. }));
}

#[test]
fn diagnostics_truncate_only_at_unicode_boundaries() {
    let input = "界".repeat(MAX_DIAGNOSTIC_CHARS);
    let (bounded, truncated) = bounded_text(&input);
    assert!(truncated);
    assert_eq!(bounded, "界".repeat(MAX_DIAGNOSTIC_CHARS / "界".len()));
}

#[test]
fn grapheme_boundaries_retain_terminal_columns_for_combining_and_wide_cells() {
    let frame = fixture_frame();
    let query_match = frame
        .query(LocatorSpec::text("e\u{301}界").expect("text query"))
        .expect("query execution")
        .single()
        .expect("single match")
        .clone();
    assert_eq!(query_match.text.as_ref(), "e\u{301}界");
    assert_eq!(
        query_match.ranges.as_ref(),
        &[MatchRange {
            row: 1,
            start_column: 0,
            end_column: 3,
        }]
    );
}

#[test]
fn composed_regions_intersect_and_partial_graphemes_are_rejected() {
    let outer = LocatorSpec::default()
        .in_region(ScreenRegion {
            top: 0,
            left: 0,
            bottom: 4,
            right: 8,
        })
        .unwrap_or_default();

    let inner = LocatorSpec::default()
        .in_region(ScreenRegion {
            top: 1,
            left: 2,
            bottom: 3,
            right: 6,
        })
        .unwrap_or_default();

    let combined = outer.and(inner).unwrap_or_default();
    assert_eq!(
        combined.region,
        Some(ScreenRegion {
            top: 1,
            left: 2,
            bottom: 3,
            right: 6,
        })
    );

    let boundaries = [(0, 3, 0), (3, 6, 1)];
    assert!(matches!(
        matched_boundary_range(&boundaries, 1, 3),
        Err(QueryError::InvalidQuery { .. })
    ));
}

#[derive(serde::Deserialize)]
struct ConformanceCorpus {
    version: u32,
    frame: ScreenFrame,
    cases: Vec<ConformanceCase>,
}

#[derive(serde::Deserialize)]
struct ConformanceCase {
    name: String,
    query: serde_json::Value,
    #[serde(default)]
    matches: Vec<ExpectedMatch>,
    error: Option<String>,
}

#[derive(serde::Deserialize)]
struct ExpectedMatch {
    text: String,
    ranges: Vec<[u16; 3]>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ConformanceQuery {
    Text {
        #[serde(default)]
        value: String,
        #[serde(default)]
        exact: bool,
        #[serde(default)]
        cross_soft_wraps: bool,
    },
    Regex {
        #[serde(default)]
        pattern: String,
    },
    Style {
        #[serde(default)]
        bold: Option<bool>,
        #[serde(default)]
        resolved_background: Option<String>,
    },
    Row {
        row: u16,
    },
    Region {
        top: u16,
        left: u16,
        width: u16,
        height: u16,
    },
    All {
        specs: Vec<ConformanceQuery>,
    },
}

impl TryFrom<ConformanceQuery> for LocatorSpec {
    type Error = QueryError;

    fn try_from(query: ConformanceQuery) -> Result<Self, Self::Error> {
        match query {
            ConformanceQuery::Text {
                value,
                exact,
                cross_soft_wraps,
            } => {
                let spec =
                    if exact { LocatorSpec::exact(value)? } else { LocatorSpec::text(value)? };

                return Ok(spec.across_soft_wraps(cross_soft_wraps));
            }
            ConformanceQuery::Regex { pattern } => return LocatorSpec::native_regex(pattern),
            ConformanceQuery::Style {
                bold,
                resolved_background,
            } => {
                let bg = resolved_background.and_then(|color| {
                    if color == "#3b1e1c" {
                        return Some(Rgb {
                            r: 59,
                            g: 30,
                            b: 28,
                        });
                    }
                    return None;
                });

                return Ok(LocatorSpec::default().with_style(StyleFilter {
                    bold,
                    resolved_background: bg.map(Some),
                    ..StyleFilter::default()
                }));
            }
            ConformanceQuery::Row { row } => return Ok(LocatorSpec::default().in_row(row)),
            ConformanceQuery::Region {
                top,
                left,
                width,
                height,
            } => {
                return LocatorSpec::default().in_region(ScreenRegion {
                    top,
                    left,
                    right: left.saturating_add(width),
                    bottom: top.saturating_add(height),
                });
            }
            ConformanceQuery::All { specs } => {
                let mut combined = LocatorSpec::default();
                for spec in specs {
                    combined = combined.and(LocatorSpec::try_from(spec)?)?;
                }

                return Ok(combined);
            }
        }
    }
}

fn conformance_spec(value: &serde_json::Value) -> Result<LocatorSpec, QueryError> {
    let query: ConformanceQuery = serde_json::from_value(value.clone()).map_err(|e| {
        return QueryError::InvalidQuery {
            reason: e.to_string(),
        };
    })?;
    return LocatorSpec::try_from(query);
}

#[test]
fn shared_locator_conformance_corpus_matches_public_contract() {
    let corpus: ConformanceCorpus = serde_json::from_str(include_str!(
        "../../../../../fixtures/conformance/locators/locator.json"
    ))
    .expect("locator conformance fixture");

    assert_eq!(corpus.version, 1);
    assert!(!corpus.frame.presented);
    assert_eq!(corpus.frame.capture_quality, CaptureQuality::Incomplete);

    for test_case in corpus.cases {
        let result = conformance_spec(&test_case.query)
            .and_then(|spec| return corpus.frame.query(spec))
            .map(|query| {
                return query
                    .all()
                    .iter()
                    .map(|screen_match| {
                        assert_eq!(screen_match.frame.state_hash, "fixture-state-v1");
                        assert_eq!(screen_match.frame.frame_hash, "fixture-frame-v1");
                        assert!(!screen_match.frame.presented);
                        assert!(screen_match.query.text.len() <= MAX_DIAGNOSTIC_CHARS);

                        return ExpectedMatch {
                            text: screen_match.text.to_string(),
                            ranges: screen_match
                                .ranges
                                .iter()
                                .map(|range| {
                                    return [range.row, range.start_column, range.end_column];
                                })
                                .collect(),
                        };
                    })
                    .collect::<Vec<_>>();
            });

        if test_case.error.as_deref() == Some("invalid_query") {
            assert!(
                matches!(result, Err(QueryError::InvalidQuery { .. })),
                "{}",
                test_case.name
            );
            continue;
        }

        let actual = result.unwrap_or_else(|error| {
            panic!("{} failed: {error}", test_case.name);
        });
        assert_eq!(actual.len(), test_case.matches.len(), "{}", test_case.name);

        for (actual_match, expected_match) in actual.iter().zip(&test_case.matches) {
            assert_eq!(actual_match.text, expected_match.text, "{}", test_case.name);
            assert_eq!(
                actual_match.ranges, expected_match.ranges,
                "{}",
                test_case.name
            );
        }
    }
}

fn parity_spec(spec: &serde_json::Value) -> Result<LocatorSpec, QueryError> {
    let mut locator = if let Some(text) = spec.get("text").and_then(serde_json::Value::as_str) {
        LocatorSpec::text(text)?
    } else if let Some(text) = spec.get("exact").and_then(serde_json::Value::as_str) {
        LocatorSpec::exact(text)?
    } else if let Some(pattern) = spec.get("regex").and_then(serde_json::Value::as_str) {
        LocatorSpec::native_regex(pattern)?
    } else {
        LocatorSpec::default()
    };

    if let Some(row) = spec.get("row").and_then(serde_json::Value::as_u64) {
        locator = locator.in_row(u16::try_from(row).expect("fixture row"));
    }

    if let Some(region) = spec.get("region") {
        locator = locator.in_region(serde_json::from_value(region.clone()).expect("region"))?;
    }

    if let Some(style) = spec.get("style") {
        locator = locator.with_style(serde_json::from_value(style.clone()).expect("style"));
    }

    if spec
        .get("cross_soft_wraps")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        locator = locator.across_soft_wraps(true);
    }

    return Ok(locator);
}

fn parity_matches(
    frame: &Arc<ScreenFrame>,
    case: &serde_json::Value,
) -> Result<Vec<ScreenMatch>, QueryError> {
    let result = frame.query(parity_spec(&case["spec"])?)?;
    let index = case
        .get("index")
        .and_then(serde_json::Value::as_u64)
        .map(|index| return usize::try_from(index).expect("fixture index"));

    return match case.get("operation").and_then(serde_json::Value::as_str) {
        None | Some("all") => Ok(result.all().to_vec()),
        Some("single") => Ok(vec![result.single()?.clone()]),
        Some("first") => Ok(vec![result.first()?.clone()]),
        Some("nth") => Ok(vec![result.nth(index.expect("nth index"))?.clone()]),
        Some(other) => Err(QueryError::InvalidQuery {
            reason: format!("unknown parity operation {other}"),
        }),
    };
}

#[test]
fn shared_parity_corpus_matches_ranges_widths_and_failure_kinds() {
    let corpus: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../fixtures/conformance/locators/parity.json"
    ))
    .expect("parity corpus");

    let frame: Arc<ScreenFrame> = Arc::new(
        serde_json::from_str(include_str!(
            "../../../../../fixtures/conformance/locators/parity-frame.json"
        ))
        .expect("parity frame"),
    );

    let ScreenFrameSource::Actor {
        observed_monotonic_ns,
        boundaries,
        ..
    } = &frame.source
    else {
        panic!("actor source");
    };

    assert_eq!(*observed_monotonic_ns, u64::MAX);
    assert_eq!(boundaries.observed.next_event_seq, 9_007_199_254_740_993);

    let cases = corpus["cases"].as_array().expect("cases");
    assert!(cases.len() >= 12);

    for case in cases {
        let name = case["name"].as_str().expect("case name");
        let expect = &case["expect"];

        match parity_matches(&frame, case) {
            Ok(found) => {
                let expected = expect["matches"]
                    .as_array()
                    .unwrap_or_else(|| panic!("{name}: unexpected success"));
                assert_eq!(found.len(), expected.len(), "{name}");

                for (actual, expected) in found.iter().zip(expected) {
                    assert_eq!(actual.text.as_ref(), expected["text"], "{name}");
                    assert!(
                        Arc::ptr_eq(&actual.frame, &frame),
                        "{name}: frame recaptured"
                    );

                    let ranges: Vec<serde_json::Value> = actual
                        .ranges
                        .iter()
                        .map(|range| {
                            return serde_json::json!([
                                range.row,
                                range.start_column,
                                range.end_column
                            ]);
                        })
                        .collect();
                    assert_eq!(
                        serde_json::Value::Array(ranges),
                        expected["ranges"],
                        "{name}"
                    );

                    let widths: Vec<u8> = actual
                        .covered_cells
                        .iter()
                        .map(|cell| return cell.width)
                        .collect();
                    assert_eq!(serde_json::json!(widths), expected["widths"], "{name}");
                }
            }
            Err(error) => {
                let expected = &expect["error"];

                assert_eq!(error.kind(), expected["kind"], "{name}: {error}");

                let details = serde_json::to_value(&error).expect("details");
                for field in ["count", "index", "state_hash"] {
                    if let Some(value) = expected.get(field) {
                        assert_eq!(&details[field], value, "{name}: {field}");
                    }
                }

                if let Some(ranges) = expected.get("ranges") {
                    let actual: Vec<serde_json::Value> = details["ranges"]
                        .as_array()
                        .expect("ranges")
                        .iter()
                        .map(|range| {
                            return serde_json::json!([
                                range["row"],
                                range["start_column"],
                                range["end_column"]
                            ]);
                        })
                        .collect();
                    assert_eq!(&serde_json::Value::Array(actual), ranges, "{name}");
                }
            }
        }
    }
}

#[test]
fn style_filter_distinguishes_absent_unresolved_and_resolved_colors() {
    let unconstrained =
        serde_json::from_value::<StyleFilter>(json!({ "bold": true })).expect("filter");
    assert_eq!(unconstrained.resolved_foreground, None);
    assert_eq!(unconstrained.bold, Some(true));

    let unresolved = serde_json::from_value::<StyleFilter>(json!({ "resolved_foreground": null }))
        .expect("filter");
    assert_eq!(unresolved.resolved_foreground, Some(None));

    let resolved = serde_json::from_value::<StyleFilter>(
        json!({ "resolved_background": { "r": 1, "g": 2, "b": 3 } }),
    )
    .expect("filter");
    assert_eq!(
        resolved.resolved_background,
        Some(Some(Rgb { r: 1, g: 2, b: 3 }))
    );

    let palette = serde_json::from_value::<StyleFilter>(
        json!({ "foreground": { "kind": "palette", "value": 9 }, "wide_kind": "wide" }),
    )
    .expect("filter");
    assert_eq!(palette.foreground, Some(ColorSpec::Palette(9)));
    assert_eq!(palette.wide_kind, Some(WideKind::Wide));
}

#[test]
fn style_filter_rejects_null_unknown_and_mistyped_fields() {
    for value in [
        json!({ "bold": null }),
        json!({ "foreground": null }),
        json!({ "colour": "red" }),
        json!({ "bold": 1 }),
        json!({ "underline": 1.5 }),
        json!({ "resolved_foreground": { "r": 256, "g": 0, "b": 0 } }),
    ] {
        assert!(
            serde_json::from_value::<StyleFilter>(value.clone()).is_err(),
            "{value}"
        );
    }
}
