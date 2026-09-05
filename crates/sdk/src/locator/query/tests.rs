use super::*;
use termtrace_model::{terminal::observation::CaptureQuality, terminal::screen::SpanCell};

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
    let ScreenFrameData::Spans { rows } = &mut frame.data else {
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
    let ScreenFrameData::Spans { rows } = &mut frame.data else {
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
                let spec = if exact {
                    LocatorSpec::exact(value)?
                } else {
                    LocatorSpec::text(value)?
                };
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
                    } else {
                        return None;
                    }
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
