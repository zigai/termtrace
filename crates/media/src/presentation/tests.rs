use std::{
    path::{Path, PathBuf},
    process::Command,
};

use termtrace_model::{terminal::replay::SourceRange, value::geometry::TerminalGeometryReport};

use super::{
    ffmpeg::{append_change_visualization, cursor_mask_ranges, probe_delivery_video},
    mapping::{semantic_change_states_at, write_presentation_map},
    pipeline::select_preview_frames,
    planning::{
        ceil_ratio, frame_ceil, frame_floor, hash_file, missing_native_capture_error,
        ranges_overlap, validate_transform_geometry,
    },
    validation::{validate_ass_resource, validate_edit_plan, validate_edit_selector},
    *,
};

#[test]
fn missing_native_capture_guides_capture_or_semantic_fallback() {
    let error = missing_native_capture_error(
        Path::new("/evidence/session"),
        Path::new("/evidence/session/visual/manifest.json"),
    );
    let message = error.to_string();
    assert!(message.contains("/evidence/session/visual/manifest.json"));
    assert!(message.contains("termtrace artifact capture --session SESSION"));
    assert!(message.contains("--require native-video --explain"));
    assert!(message.contains("--renderer semantic"));
}

#[test]
fn rational_retiming_and_hold_round_up_to_frames() {
    assert_eq!(frame_floor(1_500_000_000, 24), 36);
    assert_eq!(frame_ceil(1_500_000_001, 24), 37);
    assert_eq!(ceil_ratio(25, 4), 7);
    assert_eq!(ceil_ratio(801 * 24, 1000), 20);
}

#[test]
fn edit_selectors_require_one_source_coordinate() {
    let selector = EditSelector {
        marker: Some("before".to_owned()),
        monotonic_ns: Some(10),
        ..EditSelector::default()
    };
    assert!(validate_edit_selector(&selector).is_err());
}

#[test]
fn edit_plan_bounds_speed_caption_and_source_overlap() {
    let clip = EditClip {
        from: EditSelector {
            monotonic_ns: Some(0),
            ..EditSelector::default()
        },
        to: EditSelector {
            monotonic_ns: Some(1_000_000_000),
            ..EditSelector::default()
        },
        speed: RationalSpeed {
            numerator: 65,
            denominator: 1,
        },
        hold_ms: 0,
        caption: Some("bounded".to_owned()),
    };
    let edit = EditPlan {
        version: EDIT_PLAN_VERSION,
        clips: vec![clip],
        ..EditPlan::default()
    };
    assert!(validate_edit_plan(&edit).is_err());
    assert!(ranges_overlap(
        SourceRange {
            start_monotonic_ns: 0,
            end_monotonic_ns: 10,
        },
        SourceRange {
            start_monotonic_ns: 9,
            end_monotonic_ns: 20,
        }
    ));
    assert!(!ranges_overlap(
        SourceRange {
            start_monotonic_ns: 0,
            end_monotonic_ns: 10,
        },
        SourceRange {
            start_monotonic_ns: 10,
            end_monotonic_ns: 20,
        }
    ));
}

#[test]
fn presentation_map_distinguishes_retimed_and_hold_frames() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let path = temporary.path().join("presentation-map.jsonl");
    let plan = PresentationPlan {
        version: 1,
        edit_plan_hash: "a".repeat(64),
        source: PresentationSource::Native {
            video: PathBuf::from("source.mkv"),
            video_hash: "b".repeat(64),
        },
        fps: 24,
        clips: vec![NormalizedClip {
            source_range: SourceRange {
                start_monotonic_ns: 0,
                end_monotonic_ns: 1_000_000_000,
            },
            source_start_frame: 0,
            source_end_frame: 24,
            output_start_frame: 0,
            output_end_frame: 8,
            retimed_frame_count: 6,
            hold_frame_count: 2,
            speed: RationalSpeed {
                numerator: 4,
                denominator: 1,
            },
            caption: Some("fast clip".to_owned()),
        }],
        output_frame_count: 8,
        ..PresentationPlan::default()
    };
    write_presentation_map(&path, &plan, &[]).expect("write frame map");
    let entries = fs_err::read_to_string(path)
        .expect("read map")
        .lines()
        .map(|line| {
            return serde_json::from_str::<PresentationMapEntry>(line).expect("map entry");
        })
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 8);
    assert_eq!(entries[5].source_frame, 20);
    assert_eq!(entries[6].operation, "hold");
    assert!(entries.iter().all(|entry| return entry.captioned));
}

#[test]
fn failed_export_removes_its_staging_directory() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let source = temporary.path().join("invalid.mkv");
    fs_err::write(&source, b"not a video").expect("invalid source fixture");
    let plan = PresentationPlan {
        version: PRESENTATION_FORMAT_VERSION,
        edit_plan_hash: "a".repeat(64),
        source: PresentationSource::Native {
            video_hash: hash_file(&source).expect("source hash"),
            video: source,
        },
        fps: 24,
        clips: vec![NormalizedClip {
            source_range: SourceRange {
                start_monotonic_ns: 0,
                end_monotonic_ns: 1,
            },
            source_start_frame: 0,
            source_end_frame: 1,
            output_start_frame: 0,
            output_end_frame: 1,
            retimed_frame_count: 1,
            hold_frame_count: 0,
            speed: RationalSpeed::default(),
            caption: None,
        }],
        output_frame_count: 1,
        ..PresentationPlan::default()
    };
    let destination = temporary.path().join("presentation");
    pipeline::export_with_capability_check(&plan, &destination, false)
        .expect_err("invalid source video should fail export");
    assert!(!destination.exists());
    let staging = fs_err::read_dir(temporary.path())
        .expect("temporary directory entries")
        .map(|entry| return entry.expect("temporary directory entry").file_name())
        .filter(|name| {
            return name.as_encoded_bytes().starts_with(b".presentation-");
        })
        .collect::<Vec<_>>();
    assert!(
        staging.is_empty(),
        "failed export leaked staging: {staging:?}"
    );
}

#[test]
#[ignore = "requires FFmpeg with libx264"]
fn exports_multi_clip_h264_presentation_atomically() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let source = temporary.path().join("source.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=size=64x64:rate=24:duration=1",
            "-c:v",
            "ffv1",
        ])
        .arg(&source)
        .status()
        .expect("run FFmpeg fixture");
    assert!(status.success());
    let plan = PresentationPlan {
        version: 1,
        edit_plan_hash: "a".repeat(64),
        source: PresentationSource::Native {
            video_hash: hash_file(&source).expect("source hash"),
            video: source,
        },
        fps: 24,
        clips: vec![NormalizedClip {
            source_range: SourceRange {
                start_monotonic_ns: 0,
                end_monotonic_ns: 500_000_000,
            },
            source_start_frame: 0,
            source_end_frame: 12,
            output_start_frame: 0,
            output_end_frame: 8,
            retimed_frame_count: 6,
            hold_frame_count: 2,
            speed: RationalSpeed {
                numerator: 2,
                denominator: 1,
            },
            caption: None,
        }],
        output_frame_count: 8,
        source_width: 64,
        source_height: 64,
        output_width: 64,
        output_height: 64,
        annotations: vec![NormalizedAnnotation {
            annotation_index: 0,
            output_start_frame: 0,
            output_end_frame: 8,
            text: None,
            template: Some("frame {output_frame} source {source_frame}".to_owned()),
            position: AnnotationPosition::default(),
            style: AnnotationStyle::default(),
            z_order: 0,
        }],
        change_visualization: Some(ChangeVisualization {
            source: ChangeSource::EncodedPixels,
            mode: ChangeMode::Heatmap,
            hold_frames: 0,
            pixel_threshold: Some(8),
        }),
        change_algorithm: Some("ffmpeg-adjacent-decoded-rgb-difference-v1".to_owned()),
        layout: PresentationLayout {
            caption: CaptionLayout::None,
            branding: Some("example".to_owned()),
        },
        transforms: vec![PixelTransform::Callout {
            x: 2,
            y: 26,
            width: 60,
            height: 30,
            label: Some("region".to_owned()),
        }],
        ..PresentationPlan::default()
    };
    let destination = temporary.path().join("presentation");
    let outcome = export(&plan, &destination).expect("export presentation");
    assert!(outcome.video.is_file());
    assert!(outcome.directory.join("manifest.json").is_file());
    assert!(outcome.directory.join("presentation-map.jsonl").is_file());
    assert!(outcome.directory.join("annotations.ass").is_file());
    assert!(
        fs_err::read_to_string(outcome.directory.join("annotations.ass"))
            .expect("read generated annotations")
            .contains("frame 7 source 11")
    );
    let probed = probe_delivery_video(&outcome.video).expect("probe delivery video");
    assert_eq!(probed["streams"][0]["width"].as_u64(), Some(64));
    assert_eq!(probed["streams"][0]["height"].as_u64(), Some(64));
    let manifest_bytes =
        fs_err::read(outcome.directory.join("manifest.json")).expect("read manifest");
    let manifest: PresentationManifest =
        serde_json::from_slice(&manifest_bytes).expect("parse manifest");
    assert_eq!(manifest.plan.output_frame_count, plan.output_frame_count);
    assert!(!manifest.output_video_hash.is_empty());
    assert!(!manifest.members.is_empty());
    assert!(outcome.directory.join("branding.txt").is_file());
    let staged_regular = outcome.directory.join("fonts/JetBrainsMono-Regular.ttf");
    assert_eq!(
        fs_err::read(staged_regular).expect("read staged regular font"),
        termtrace_render::semantic_font_set()
            .expect("semantic font set")
            .regular_face_bytes()
    );
    for unexpected in [
        "JetBrainsMono-Bold.ttf",
        "JetBrainsMono-Italic.ttf",
        "JetBrainsMono-BoldItalic.ttf",
        "SymbolsNerdFont-Regular.ttf",
    ] {
        assert!(!outcome.directory.join("fonts").join(unexpected).exists());
    }
    assert!(
        fs_err::read_to_string(outcome.directory.join("source-frame-hashes.framemd5"))
            .expect("read decoded source hashes")
            .contains("#format: frame checksums")
    );
    let preview_outcome = preview(
        &plan,
        temporary.path().join("preview"),
        PreviewOptions {
            columns: 2,
            tile_width: 80,
            max_frames: 4,
            change_only: true,
            preview_width: 160,
        },
    )
    .expect("render preview");
    assert!(preview_outcome.contact_sheet.is_file());
    assert!(preview_outcome.video.is_file());
    assert!(preview_outcome.directory.join("manifest.json").is_file());
}

#[test]
fn semantic_change_filters_retain_state_provenance_and_hold() {
    let plan = PresentationPlan {
        fps: 24,
        source_width: 80,
        source_height: 40,
        source_geometry: TerminalGeometryReport {
            cols: 10,
            rows: 2,
            cell_width: 8,
            cell_height: 20,
            text_width: 80,
            text_height: 40,
        },
        output_frame_count: 5,
        change_visualization: Some(ChangeVisualization {
            source: ChangeSource::SemanticCells,
            mode: ChangeMode::Outline,
            hold_frames: 2,
            pixel_threshold: None,
        }),
        change_frames: vec![NormalizedChangeFrame {
            state_id: 7,
            state_hash: "a".repeat(64),
            source_frame: 2,
            output_frames: vec![2],
            changed_regions: vec![ChangeRegion {
                x: 8,
                y: 0,
                width: 8,
                height: 20,
            }],
            dirty_rows: vec![0],
            evidence_level: "semantic_scheduled_for_frame".to_owned(),
        }],
        ..PresentationPlan::default()
    };
    let mut filters = Vec::new();
    let output =
        append_change_visualization(&mut filters, &plan, "input").expect("build semantic overlay");
    assert_eq!(output, "change0");
    assert!(filters[0].contains("between(n,2,4)"));
    assert_eq!(semantic_change_states_at(&plan, 4), vec![7]);
}

#[test]
fn transform_geometry_is_ordered_bounded_and_even() {
    let edit = EditPlan {
        transforms: vec![
            PixelTransform::Crop {
                x: 8,
                y: 4,
                width: 32,
                height: 24,
            },
            PixelTransform::Zoom {
                numerator: 2,
                denominator: 1,
            },
            PixelTransform::Callout {
                x: 2,
                y: 2,
                width: 20,
                height: 10,
                label: None,
            },
        ],
        ..EditPlan::default()
    };
    assert_eq!(
        validate_transform_geometry(&edit, 80, 40, false).expect("valid transforms"),
        (64, 48)
    );
    let invalid = EditPlan {
        transforms: vec![
            PixelTransform::Crop {
                x: 0,
                y: 0,
                width: 32,
                height: 24,
            },
            PixelTransform::CursorMask,
        ],
        ..EditPlan::default()
    };
    assert!(validate_transform_geometry(&invalid, 80, 40, false).is_err());
}

#[test]
fn facade_render_configuration_survives_normalization_inputs() {
    let edit = EditPlan {
        version: EDIT_PLAN_VERSION,
        clips: vec![EditClip {
            from: EditSelector {
                monotonic_ns: Some(0),
                ..EditSelector::default()
            },
            to: EditSelector {
                monotonic_ns: Some(1_000_000_000),
                ..EditSelector::default()
            },
            speed: RationalSpeed::default(),
            hold_ms: 1_000,
            caption: None,
        }],
        delivery: PresentationDelivery {
            codec: "h264_rgb".to_owned(),
            pixel_format: "rgb24".to_owned(),
            crf: 0,
            encoder_preset: "veryslow".to_owned(),
        },
        transforms: vec![PixelTransform::Resize {
            width: 1_920,
            height: 1_080,
        }],
        render_config: Some(VideoRenderConfig {
            preset: "lossless".to_owned(),
            renderer: "semantic".to_owned(),
            fps: 60,
            resolution: Some([1_920, 1_080]),
            scale: None,
            start_marker: Some("demo".to_owned()),
            end_marker: Some("done".to_owned()),
            trim_idle: true,
            idle_threshold_ms: Some(2_000),
            hold_start_ms: 500,
            hold_end_ms: 1_000,
            cursor_style: "hidden".to_owned(),
        }),
        ..EditPlan::default()
    };
    validate_edit_plan(&edit).expect("facade edit plan should validate");
    assert_eq!(
        validate_transform_geometry(&edit, 800, 600, false).expect("resize should be exact"),
        (1_920, 1_080)
    );
    assert_eq!(
        edit.render_config
            .as_ref()
            .map(|c| return c.preset.as_str()),
        Some("lossless")
    );
    assert_eq!(
        edit.render_config
            .as_ref()
            .map(|c| return c.renderer.as_str()),
        Some("semantic")
    );
    assert_eq!(edit.delivery.crf, 0);
    assert_eq!(edit.delivery.pixel_format, "rgb24");
}

#[test]
fn cursor_masks_coalesce_only_consecutive_identical_regions() {
    let make = |output_frame, x| {
        return CursorMaskFrame {
            output_frame,
            source_frame: output_frame,
            state_id: output_frame,
            state_hash: "a".repeat(64),
            region: ChangeRegion {
                x,
                y: 0,
                width: 2,
                height: 16,
            },
            evidence_level: "semantic_scheduled_for_frame".to_owned(),
        };
    };
    assert_eq!(
        cursor_mask_ranges(&[make(0, 4), make(1, 4), make(2, 8)]),
        vec![
            (
                ChangeRegion {
                    x: 4,
                    y: 0,
                    width: 2,
                    height: 16
                },
                0,
                1
            ),
            (
                ChangeRegion {
                    x: 8,
                    y: 0,
                    width: 2,
                    height: 16
                },
                2,
                2
            )
        ]
    );
}

#[test]
fn preview_selection_is_bounded_deterministic_and_change_aware() {
    let plan = PresentationPlan {
        output_frame_count: 10,
        change_visualization: Some(ChangeVisualization {
            source: ChangeSource::SemanticCells,
            mode: ChangeMode::Outline,
            hold_frames: 1,
            pixel_threshold: None,
        }),
        change_frames: vec![NormalizedChangeFrame {
            state_id: 1,
            state_hash: "a".repeat(64),
            source_frame: 3,
            output_frames: vec![3, 8],
            changed_regions: Vec::new(),
            dirty_rows: Vec::new(),
            evidence_level: "semantic_scheduled_for_frame".to_owned(),
        }],
        ..PresentationPlan::default()
    };
    let selected = select_preview_frames(
        &plan,
        PreviewOptions {
            max_frames: 3,
            change_only: true,
            ..PreviewOptions::default()
        },
    )
    .expect("select changes");
    assert_eq!(selected, vec![3, 4, 9]);
}

#[test]
fn validate_ass_resource_accepts_valid_file_and_rejects_invalid() {
    let temporary = tempfile::tempdir().expect("temporary directory");
    let valid_ass = temporary.path().join("valid.ass");
    fs_err::write(
        &valid_ass,
        "[Script Info]\nScriptType: v4.00+\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,1,0,2,10,10,10,1\n\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:00.00,0:00:01.00,Default,,0,0,0,,Hello world\n",
    )
    .expect("write valid ASS");
    assert!(validate_ass_resource(&valid_ass).is_ok());

    let invalid_ass = temporary.path().join("invalid.ass");
    fs_err::write(&invalid_ass, "plain text without sections").expect("write invalid ASS");
    assert!(validate_ass_resource(&invalid_ass).is_err());
}
