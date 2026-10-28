//! Typed editing APIs through the public SDK: the shared editing fixture, speeds, negative
//! cases, and a semantic plan, preview, and MP4/GIF/WebM render.

use serde_json::Value;
use std::{
    env,
    error::Error,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    thread,
    time::Duration,
};
use termtrace_sdk::{
    artifacts::{
        source_for_output_frame, AnnotationAnchor, AnnotationPosition, AnnotationScope,
        AnnotationStyle, ArtifactService, CaptionLayout, ChangeMode, ChangeSource,
        ChangeVisualization, EditClip, EditPlan, EditSelector, EditSource, PixelTransform,
        PresentationDelivery, PresentationLayout, PresentationPlan, RationalSpeed, SubtitleTrack,
        TextAnnotation, VideoMode, VideoOptions, VideoOutcome, VideoRenderer, VideoRequest,
    },
    input::MutationGuard,
    screen::ScreenPredicate,
    session::{LaunchSpec, StopRequest},
    system::SystemService,
    Terminal,
};

type Outcome<T = ()> = Result<T, Box<dyn Error>>;

fn fixture_edit() -> EditPlan {
    let style = |font_size, foreground: &str, background: &str, background_opacity, border| {
        return AnnotationStyle {
            font_size,
            foreground: foreground.to_owned(),
            background: background.to_owned(),
            background_opacity,
            border,
        };
    };
    let annotation =
        |text: Option<&str>, template: Option<&str>, scope, position, style, z_order| {
            return TextAnnotation {
                text: text.map(str::to_owned),
                template: template.map(str::to_owned),
                scope,
                position,
                style,
                z_order,
            };
        };
    let at = |anchor, x, y| return AnnotationPosition { anchor, x, y };

    return EditPlan {
        annotations: vec![
            annotation(
                Some("Example"),
                None,
                AnnotationScope::WholeVideo,
                AnnotationPosition::default(),
                AnnotationStyle::default(),
                0,
            ),
            annotation(
                None,
                Some("frame {output_frame}"),
                AnnotationScope::Clip { clip_index: 1 },
                at(AnnotationAnchor::BottomRight, 8, 8),
                style(16, "#ffff00", "#202020", 200, 0),
                1,
            ),
            annotation(
                Some("Range"),
                None,
                AnnotationScope::Range {
                    from: EditSelector::marker("intro"),
                    to: EditSelector::marker("middle"),
                },
                at(AnnotationAnchor::Center, 0, 0),
                AnnotationStyle::default(),
                2,
            ),
            annotation(
                Some("Start"),
                None,
                AnnotationScope::OutputFrame { output_frame: 0 },
                at(AnnotationAnchor::TopRight, 16, 16),
                AnnotationStyle::default(),
                3,
            ),
        ],
        subtitles: vec![SubtitleTrack {
            ass: PathBuf::from("editing-captions.ass"),
        }],
        change_visualization: Some(ChangeVisualization {
            source: ChangeSource::SemanticCells,
            mode: ChangeMode::BoundingBox,
            hold_frames: 2,
            pixel_threshold: None,
        }),
        layout: PresentationLayout {
            caption: CaptionLayout::Footer,
            branding: Some("Example".to_owned()),
        },
        delivery: PresentationDelivery {
            crf: 20,
            encoder_preset: "slow".to_owned(),
            ..PresentationDelivery::default()
        },
        transforms: vec![
            PixelTransform::CursorMask,
            PixelTransform::Crop {
                x: 0,
                y: 0,
                width: 480,
                height: 240,
            },
            PixelTransform::Zoom {
                numerator: 2,
                denominator: 1,
            },
            PixelTransform::Callout {
                x: 16,
                y: 16,
                width: 200,
                height: 100,
                label: Some("note".to_owned()),
            },
        ],
        ..EditPlan::new(vec![
            EditClip {
                speed: RationalSpeed::new(2, 3),
                hold_ms: 500,
                caption: Some("First part".to_owned()),
                ..EditClip::new(
                    EditSelector::marker("intro"),
                    EditSelector::marker("middle"),
                )
            },
            EditClip {
                speed: RationalSpeed::new(3, 2),
                ..EditClip::new(
                    EditSelector::marker("middle"),
                    EditSelector::marker("outro"),
                )
            },
        ])
    };
}

fn record(terminal: &Terminal) -> Outcome<PathBuf> {
    let mut session = terminal.start(
        LaunchSpec::new([
            OsString::from("/bin/sh"),
            OsString::from("-c"),
            OsString::from(
                "trap 'exit 0' TERM; i=0; while :; do i=$((i+1)); printf 'line %s\\n' $i; sleep 0.4; done",
            ),
        ])
        .geometry(80, 24),
    )?;
    session.wait_screen(
        ScreenPredicate::Contains {
            text: "line 1".to_owned(),
        },
        Duration::ZERO,
        Duration::from_secs(5),
    )?;

    for (delay, label) in [
        (300, "intro"),
        (600, "middle"),
        (100, "repeat"),
        (100, "repeat"),
        (500, "outro"),
        (300, ""),
    ] {
        thread::sleep(Duration::from_millis(delay));
        if !label.is_empty() {
            session.mark(label, &MutationGuard::default())?;
        }
    }

    session.stop(StopRequest::default(), Some(Duration::from_secs(10)))?;

    return Ok(session.path().to_path_buf());
}

fn video(
    session: &Path,
    edit: EditSource,
    mode: VideoMode,
    options: &VideoOptions,
) -> Outcome<VideoOutcome> {
    let binary = PathBuf::from(env::var_os("TERMTRACE_BIN").ok_or("expected TERMTRACE_BIN")?);
    let request: VideoRequest = options.request(session.to_path_buf(), edit, mode)?;
    let cancelled = AtomicBool::new(false);

    return Ok(ArtifactService::video(
        &request,
        &binary,
        &mut |id| return SystemService::require(id, &cancelled),
        &mut |_| {},
    )?);
}

fn plan(session: &Path, edit: EditSource) -> Outcome<PresentationPlan> {
    let options = VideoOptions {
        renderer: VideoRenderer::Semantic,
        fps: Some(30),
        ..VideoOptions::default()
    };
    let VideoOutcome::Planned(plan) = video(session, edit, VideoMode::Plan, &options)? else {
        return Err("plan mode returned another outcome".into());
    };

    return Ok(*plan);
}

fn without_subtitle_paths(mut plan: PresentationPlan) -> PresentationPlan {
    for subtitle in &mut plan.subtitles {
        subtitle.source_path = PathBuf::new();
    }

    return plan;
}

fn main() -> Outcome {
    let root = PathBuf::from(
        env::args_os()
            .nth(1)
            .ok_or("expected output directory argument")?,
    );
    let binary = PathBuf::from(env::var_os("TERMTRACE_BIN").ok_or("expected TERMTRACE_BIN")?);
    let resources = fs::canonicalize(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../fixtures/conformance/sdk"),
    )?;
    let fixture: Value = serde_json::from_slice(&fs::read(resources.join("editing.json"))?)?;
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root)?;
    let root = fs::canonicalize(root)?;

    // Built with typed constructors, normalized by the single Rust normalizer.
    let built = EditSource::Plan {
        edit: Box::new(fixture_edit()),
        resource_base: Some(resources.clone()),
    };
    let normalized = ArtifactService::normalize_edit(&built)?;
    assert_eq!(
        normalized.edit_plan_hash, fixture["edit_plan_hash"],
        "edit_plan_hash"
    );
    assert_eq!(
        fixture_edit(),
        serde_json::from_value(fixture["edit"].clone())?
    );

    let unbased = ArtifactService::normalize_edit(&EditSource::Plan {
        edit: Box::new(fixture_edit()),
        resource_base: None,
    })
    .expect_err("relative subtitles need a base");
    let expected = &fixture["relative_without_base"];
    assert_eq!(unbased.code(), expected["code"]);
    assert_eq!(
        serde_json::to_value(unbased.edit_failure().ok_or("edit facts")?)?["field"],
        expected["facts"]["field"]
    );

    let session = record(&Terminal::new(&binary, root.join("sessions")))?;

    // The same edit as a JSON file in another directory plans identically.
    let files = root.join("files");
    fs::create_dir_all(&files)?;
    fs::copy(
        resources.join("editing-captions.ass"),
        files.join("editing-captions.ass"),
    )?;
    fs::write(
        files.join("edit.json"),
        serde_json::to_vec(&fixture["edit"])?,
    )?;
    let from_builder = plan(&session, built.clone())?;
    let from_file = plan(
        &session,
        EditSource::File {
            path: files.join("edit.json"),
        },
    )?;
    assert_eq!(from_builder.edit_plan_hash, normalized.edit_plan_hash);
    assert_eq!(
        without_subtitle_paths(from_builder.clone()),
        without_subtitle_paths(from_file)
    );

    let first = source_for_output_frame(&from_builder, 0).ok_or("output frame 0")?;
    assert_eq!(first.clip_index, 0);

    let speeds = &fixture["speeds"];
    for case in speeds["cases"].as_array().ok_or("speed cases")? {
        let speed = RationalSpeed::new(
            u32::try_from(case["speed"][0].as_u64().ok_or("numerator")?)?,
            u32::try_from(case["speed"][1].as_u64().ok_or("denominator")?)?,
        );
        let to = speeds["to_ns"].as_u64().ok_or("to")?;
        // A trailing clip keeps the measured clip off the semantic source's final frame.
        let edit = EditPlan::new(vec![
            EditClip {
                speed,
                ..EditClip::new(
                    EditSelector::monotonic_ns(speeds["from_ns"].as_u64().ok_or("from")?),
                    EditSelector::monotonic_ns(to),
                )
            },
            EditClip::new(
                EditSelector::monotonic_ns(to),
                EditSelector::monotonic_ns(speeds["tail_to_ns"].as_u64().ok_or("tail")?),
            ),
        ]);
        let planned = plan(
            &session,
            EditSource::Plan {
                edit: Box::new(edit),
                resource_base: None,
            },
        )?;
        let clip = planned.clips.first().ok_or("clip")?;
        assert_eq!(
            clip.source_end_frame - clip.source_start_frame,
            speeds["source_frames"]
        );
        assert_eq!(
            clip.retimed_frame_count, case["retimed_frame_count"],
            "{case}"
        );
    }

    for case in fixture["negative"].as_array().ok_or("negative cases")? {
        let directory = files.join(case["name"].as_str().ok_or("name")?);
        fs::create_dir_all(&directory)?;
        for (name, contents) in case["files"].as_object().into_iter().flatten() {
            fs::write(
                directory.join(name),
                contents.as_str().ok_or("file contents")?,
            )?;
        }

        fs::write(
            directory.join("edit.json"),
            serde_json::to_vec(&case["edit"])?,
        )?;
        let error = plan(
            &session,
            EditSource::File {
                path: directory.join("edit.json"),
            },
        )
        .expect_err("negative case must fail");
        let error = error
            .downcast_ref::<termtrace_sdk::Error>()
            .ok_or_else(|| return format!("{}: untyped failure {error}", case["name"]))?;
        assert_eq!(error.code(), case["code"], "{}", case["name"]);
        let facts = serde_json::to_value(error.edit_failure().ok_or("edit facts")?)?;
        for key in ["field", "clip_index", "marker"] {
            assert_eq!(facts[key], case["facts"][key], "{} {key}", case["name"]);
        }
    }

    // Semantic preview and every delivery format from the same typed edit.
    let semantic = VideoOptions {
        renderer: VideoRenderer::Semantic,
        fps: Some(30),
        ..VideoOptions::default()
    };
    let preview = video(
        &session,
        built.clone(),
        VideoMode::Preview {
            directory: root.join("preview"),
            options: Default::default(),
        },
        &semantic,
    )?;
    let VideoOutcome::Previewed(preview) = preview else {
        return Err("preview mode returned another outcome".into());
    };
    assert!(preview.contact_sheet.is_file());

    for extension in ["mp4", "gif", "webm"] {
        let out = root.join(format!("edited.{extension}"));
        let VideoOutcome::Rendered(render) = video(
            &session,
            built.clone(),
            VideoMode::Render { out: out.clone() },
            &semantic,
        )?
        else {
            return Err("render mode returned another outcome".into());
        };
        assert_eq!(render.output, out);
        assert_eq!(render.artifact.is_some(), extension != "mp4");
        assert!(render.sidecar.join("manifest.json").is_file());
    }

    let existing = root.join("edited.mp4");
    let before = fs::read(&existing)?;
    let refused = video(
        &session,
        built,
        VideoMode::Render {
            out: existing.clone(),
        },
        &semantic,
    )
    .expect_err("an existing output needs overwrite");
    assert!(refused.to_string().contains("already exists"), "{refused}");
    assert_eq!(fs::read(&existing)?, before);

    println!("editing consumer: {}", normalized.edit_plan_hash);

    return Ok(());
}
