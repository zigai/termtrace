//! Record the terminal showcase once with original pixels, then edit it into MP4, GIF and WebM.
//!
//! Usage: `demo OUTPUT_DIRECTORY` (the directory must not exist). Requires Ghostty, Xvfb,
//! dbus-run-session, FFmpeg and the JetBrains Mono font. Set `TERMTRACE_BIN` to the CLI to use.

use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    thread,
    time::Duration,
};
use termtrace_sdk::{
    artifacts::{
        ArtifactService, CaptionLayout, EditClip, EditPlan, EditSelector, EditSource,
        PresentationLayout, PresentationSource, RationalSpeed, VideoMode, VideoOptions,
        VideoOutcome, VideoRenderer,
    },
    input::{ActRequest, MutationGuard},
    screen::ScreenPredicate,
    session::{
        CellGrid, NativeVisualProfile, OriginalGeometry, OriginalLaunchSpec, PixelCanvas, Session,
        StopRequest, VisualProfile,
    },
    system::SystemService,
    ProcessSignal, Terminal,
};

type Outcome<T = ()> = Result<T, Box<dyn Error>>;

const TIMEOUT: Duration = Duration::from_secs(20);
const SIGNALS: [(ProcessSignal, &str); 4] = [
    (ProcessSignal::User1, "SIGUSR1"),
    (ProcessSignal::User2, "SIGUSR2"),
    (ProcessSignal::Hangup, "SIGHUP"),
    (ProcessSignal::Interrupt, "SIGINT"),
];

fn contains(text: &str) -> ScreenPredicate {
    return ScreenPredicate::Contains {
        text: text.to_owned(),
    };
}

fn read_json(path: &Path) -> Outcome<Value> {
    return Ok(serde_json::from_slice(&fs::read(path)?)?);
}

fn canonical_files(session: &Path) -> Outcome<BTreeMap<PathBuf, Vec<u8>>> {
    let mut files = BTreeMap::new();
    for name in ["session.json", "events.jsonl", "completion.json"] {
        files.insert(PathBuf::from(name), fs::read(session.join(name))?);
    }
    for entry in fs::read_dir(session.join("visual-original"))? {
        let path = entry?.path();
        if path.is_file() {
            files.insert(path.clone(), fs::read(&path)?);
        }
    }

    return Ok(files);
}

fn drive(live: &Session) -> Outcome {
    let ready = live.wait_screen(contains("SHOWCASE_READY"), Duration::ZERO, TIMEOUT)?;
    live.mark("showcase-ready", &MutationGuard::default())?;
    // The showcase reads its terminal raw, so the line ends with a newline, not Enter.
    live.act(
        ActRequest::new()
            .if_state(ready.frame.state_hash)
            .text("continue-from-file\n")
            .until_screen(contains("WAITING_FOR_SIGNALS"), TIMEOUT),
    )?;
    for (signal, name) in SIGNALS {
        // Presentation pacing so each step stays on screen; the waits synchronize.
        thread::sleep(Duration::from_millis(400));
        live.signal(signal, &MutationGuard::default())?;
        live.wait_screen(contains(&format!("SIGNAL:{name}")), Duration::ZERO, TIMEOUT)?;
    }
    thread::sleep(Duration::from_millis(400));
    live.mark("signals-observed", &MutationGuard::default())?;

    return Ok(());
}

fn video(
    binary: &Path,
    session: &Path,
    edit: &EditSource,
    mode: VideoMode,
) -> Outcome<VideoOutcome> {
    let options = VideoOptions {
        renderer: VideoRenderer::OriginalExecution,
        fps: Some(24),
        ..VideoOptions::default()
    };
    let request = options.request(session.to_path_buf(), edit.clone(), mode)?;
    let cancelled = AtomicBool::new(false);

    return Ok(ArtifactService::video(
        &request,
        binary,
        &mut |id| return SystemService::require(id, &cancelled),
        &mut |_| {},
    )?);
}

fn main() -> Outcome {
    let output = PathBuf::from(
        env::args_os()
            .nth(1)
            .ok_or("usage: demo OUTPUT_DIRECTORY")?,
    );
    let binary = PathBuf::from(env::var_os("TERMTRACE_BIN").ok_or("expected TERMTRACE_BIN")?);
    let repository = fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.."))?;
    fs::create_dir(&output)?;
    let terminal = Terminal::new(&binary, &output);

    let profile = VisualProfile {
        version: 1,
        native: Some(NativeVisualProfile {
            font_family: Some("JetBrains Mono".to_owned()),
            font_size_points: Some("12".to_owned()),
            dpi: Some(96),
            ..NativeVisualProfile::default()
        }),
        semantic: None,
    };
    let report = terminal.preflight_visual_profile(&profile, 80, 24)?;
    let native = report.native.clone().ok_or("native profile report")?;
    assert_eq!(native.dpi, 96);
    assert!(native
        .discovered_font_families
        .iter()
        .any(|family| family == "JetBrains Mono"));
    assert_eq!(
        (native.geometry.required_cols, native.geometry.required_rows),
        (80, 24)
    );

    let mut spec = OriginalLaunchSpec::new([
        "python3".into(),
        repository
            .join("examples/apps/terminal_showcase.py")
            .into_os_string(),
    ]);
    spec.geometry = OriginalGeometry::Grid(CellGrid { cols: 80, rows: 24 });
    spec.canvas = Some(PixelCanvas {
        width: 1280,
        height: 720,
    });
    spec.visual_profile = Some(profile);
    spec.startup_timeout = Duration::from_secs(60);
    let mut original = terminal.start_original(&spec, &AtomicBool::new(false))?;
    if let Err(error) = drive(original.session()) {
        original.force_close()?;
        return Err(error);
    }

    let recording = original.finish(StopRequest::default(), Duration::from_secs(60))?;
    let capture = serde_json::to_value(&recording.capture)?;
    assert_eq!(capture["provenance"], json!({"kind": "original_execution"}));
    let session = original.session().path().to_path_buf();
    let manifest = read_json(&session.join("session.json"))?;
    assert_eq!(
        manifest["visual_profile"]["profile_hash"],
        json!(report.profile_hash)
    );
    let completion = read_json(&session.join("completion.json"))?;
    let capture_manifest = read_json(&session.join("visual-original/manifest.json"))?;
    assert_eq!(capture_manifest["capture_kind"], "original_execution");
    assert_eq!(
        capture_manifest["effective_config_hash"],
        json!(native.effective_config_hash)
    );
    assert_eq!(capture_manifest["geometry_report"]["cols"], 80);
    assert_eq!(capture_manifest["geometry_report"]["rows"], 24);
    let original_video_hash = capture_manifest["members"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|member| return member["path"] == "session.mkv")
        .map(|member| return member["blake3"].clone())
        .ok_or("captured video member")?;
    let before = canonical_files(&session)?;

    let edit = EditSource::Plan {
        edit: Box::new(EditPlan {
            layout: PresentationLayout {
                caption: CaptionLayout::Footer,
                branding: None,
            },
            ..EditPlan::new(vec![EditClip {
                speed: RationalSpeed::new(1, 1),
                hold_ms: 500,
                caption: Some("State-guarded input and terminal signals".to_owned()),
                ..EditClip::new(
                    EditSelector::marker("showcase-ready"),
                    EditSelector::marker("signals-observed"),
                )
            }])
        }),
        resource_base: None,
    };
    let renders = output.join("renders");
    fs::create_dir(&renders)?;
    let VideoOutcome::Previewed(preview) = video(
        &binary,
        &session,
        &edit,
        VideoMode::Preview {
            directory: renders.join("preview"),
            options: Default::default(),
        },
    )?
    else {
        return Err("preview mode returned another outcome".into());
    };

    for extension in ["mp4", "gif", "webm"] {
        let out = renders.join(format!("demo.{extension}"));
        let VideoOutcome::Rendered(render) =
            video(&binary, &session, &edit, VideoMode::Render { out })?
        else {
            return Err("render mode returned another outcome".into());
        };
        assert_eq!(render.plan.edit_plan_hash, preview.plan.edit_plan_hash);
        match &render.artifact {
            None => {
                // The MP4 sidecar names its source by path and hash; it must be the original capture.
                let PresentationSource::Native { video_hash, .. } = &render.plan.source else {
                    return Err("the MP4 was not rendered from native pixels".into());
                };
                assert_eq!(json!(video_hash), original_video_hash);
            }
            Some(artifact) => {
                let artifact = serde_json::to_value(artifact)?;
                assert_eq!(
                    artifact["provenance"],
                    json!({"kind": "presentation_output", "source_kind": "original_execution"})
                );
                assert_eq!(
                    artifact["source"]["session_manifest_hash"],
                    completion["session_manifest_hash"]
                );
            }
        }
    }

    assert!(
        canonical_files(&session)? == before,
        "canonical evidence changed"
    );
    let outputs = ["mp4", "gif", "webm"].map(|extension| {
        return renders.join(format!("demo.{extension}"));
    });
    let summary = json!({
        "session": session,
        "profile_hash": report.profile_hash,
        "edit_plan_hash": preview.plan.edit_plan_hash,
        "contact_sheet": preview.contact_sheet,
        "outputs": outputs,
    });
    println!("{}", serde_json::to_string_pretty(&summary)?);

    return Ok(());
}
