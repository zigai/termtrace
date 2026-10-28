//! Probe delivery codecs and render one `WebM` through the public SDK with the inherited `PATH`,
//! printing the capability report, the `webm-delivery` requirement, and the render result as
//! one JSON object.

use serde_json::json;
use std::{env, error::Error, path::PathBuf, sync::atomic::AtomicBool};
use termtrace_sdk::{
    artifacts::{
        ArtifactService, EditClip, EditPlan, EditSelector, EditSource, VideoMode, VideoOptions,
        VideoRenderer,
    },
    system::SystemService,
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args_os().skip(1);
    let session = PathBuf::from(arguments.next().ok_or("expected a session")?);
    let out = PathBuf::from(arguments.next().ok_or("expected an output path")?);
    let binary = PathBuf::from(env::var_os("TERMTRACE_BIN").ok_or("expected TERMTRACE_BIN")?);
    let cancelled = AtomicBool::new(false);

    let report = SystemService::probe_capabilities(&cancelled);
    let required = SystemService::require("webm-delivery", &cancelled);
    let edit = EditPlan::new(vec![EditClip::new(
        EditSelector::marker("start"),
        EditSelector::marker("end"),
    )]);
    let options = VideoOptions {
        renderer: VideoRenderer::Semantic,
        ..VideoOptions::default()
    };
    let request = options.request(
        session,
        EditSource::Plan {
            edit: Box::new(edit),
            resource_base: None,
        },
        VideoMode::Render { out },
    )?;
    let rendered = ArtifactService::video(
        &request,
        &binary,
        &mut |id| return SystemService::require(id, &cancelled),
        &mut |_| {},
    );

    println!(
        "{}",
        json!({
            "report": report,
            "require_error": required.err().map(|error| return error.to_string()),
            "render_error": rendered.err().map(|error| return error.to_string()),
        })
    );

    return Ok(());
}
