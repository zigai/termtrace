//! Edit normalization, presentation videos, and frame mapping.

use crate::host::{Arg, Model};
use crate::{to_napi_err, JsTermtrace};
use napi_derive::napi;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use termtrace_model::value::ExactPath;
use termtrace_sdk::artifacts::{
    output_frames_for_source_frame as output_frames, source_for_output_frame as output_source,
    ArtifactService, EditSource, NormalizedEdit, OutputFrameSource, PresentationPlan,
    PreviewOptions, VideoMode, VideoOptions, VideoOutcome, VideoPreview, VideoRender,
};
use termtrace_sdk::system::SystemService;

async fn video(
    binary: PathBuf,
    session: PathBuf,
    edit: EditSource,
    mode: VideoMode,
    options: VideoOptions,
) -> napi::Result<VideoOutcome> {
    return tokio::task::spawn_blocking(move || {
        let request = options.request(session, edit, mode).map_err(to_napi_err)?;
        let cancelled = AtomicBool::new(false);

        return ArtifactService::video(
            &request,
            &binary,
            &mut |id| return SystemService::require(id, &cancelled),
            &mut |_| {},
        )
        .map_err(to_napi_err);
    })
    .await
    .map_err(|error| return napi::Error::from_reason(error.to_string()))?;
}

#[napi]
impl JsTermtrace {
    /// Resolve the presentation plan of `edit` over a completed `session` without rendering.
    /// `renderer` is `native` (verified Ghostty pixels, captured first when missing; never
    /// semantic) or `semantic`; `fps` defaults to 30.
    ///
    /// # Errors
    ///
    /// Rejects with an edit failure `code` and `details` for a rejected edit, and for blocked
    /// capabilities, invalid evidence, or capture failures.
    #[napi]
    pub async fn video_plan(
        &self,
        session: Arg<ExactPath>,
        edit: Arg<EditSource>,
        options: Arg<VideoOptions>,
    ) -> napi::Result<Model<PresentationPlan>> {
        let binary = self.terminal.binary().to_path_buf();
        let outcome = video(binary, session.0 .0, edit.0, VideoMode::Plan, options.0).await?;
        let VideoOutcome::Planned(plan) = outcome else {
            return Err(napi::Error::from_reason("plan returned another outcome"));
        };

        return Ok(Model(*plan));
    }

    /// Render a contact sheet and low-resolution review video of `edit` into the new
    /// `directory`. `options.preview` sets the storyboard.
    ///
    /// # Errors
    ///
    /// Rejects like `videoPlan`, and when the directory exists or rendering fails.
    #[napi]
    pub async fn video_preview(
        &self,
        session: Arg<ExactPath>,
        edit: Arg<EditSource>,
        directory: Arg<ExactPath>,
        options: Arg<VideoOptions>,
    ) -> napi::Result<Model<VideoPreview>> {
        let binary = self.terminal.binary().to_path_buf();
        let mode = VideoMode::Preview {
            directory: directory.0 .0,
            options: PreviewOptions::default(),
        };
        let outcome = video(binary, session.0 .0, edit.0, mode, options.0).await?;
        let VideoOutcome::Previewed(preview) = outcome else {
            return Err(napi::Error::from_reason("preview returned another outcome"));
        };

        return Ok(Model(*preview));
    }

    /// Render `edit` to `out`, whose lowercase `.mp4`, `.gif`, or `.webm` extension selects
    /// the format, keeping a manifested `.termtrace` sidecar beside it. An existing output
    /// fails unless `options.overwrite` is set for an MP4; GIF and `WebM` are never replaced.
    ///
    /// # Errors
    ///
    /// Rejects like `videoPlan`, and when the output exists or rendering fails.
    #[napi]
    pub async fn video_render(
        &self,
        session: Arg<ExactPath>,
        edit: Arg<EditSource>,
        out: Arg<ExactPath>,
        options: Arg<VideoOptions>,
    ) -> napi::Result<Model<VideoRender>> {
        let binary = self.terminal.binary().to_path_buf();
        let mode = VideoMode::Render { out: out.0 .0 };
        let outcome = video(binary, session.0 .0, edit.0, mode, options.0).await?;
        let VideoOutcome::Rendered(render) = outcome else {
            return Err(napi::Error::from_reason("render returned another outcome"));
        };

        return Ok(Model(*render));
    }
}

/// Resolve an edit's subtitle paths, validate it, and return it with its `edit_plan_hash`.
///
/// # Errors
///
/// Rejects with an edit failure `code` (for example `invalid_speed` or
/// `relative_resource_without_base`) and `details` holding `field`, `clip_index`, and `marker`.
#[napi]
pub async fn normalize_edit(edit: Arg<EditSource>) -> napi::Result<Model<NormalizedEdit>> {
    return tokio::task::spawn_blocking(move || {
        return ArtifactService::normalize_edit(&edit.0)
            .map(Model)
            .map_err(to_napi_err);
    })
    .await
    .map_err(|error| return napi::Error::from_reason(error.to_string()))?;
}

/// Clip, source frame, and operation (`source` or `hold`) shown on `outputFrame`, or `null`
/// past the end.
#[napi]
pub fn source_for_output_frame(
    plan: Arg<PresentationPlan>,
    output_frame: Arg<u64>,
) -> Option<Model<OutputFrameSource>> {
    return output_source(&plan.0, output_frame.0).map(Model);
}

/// Half-open `[start, end]` output-frame spans that show `sourceFrame`, in output order.
#[napi(ts_return_type = "Array<[bigint, bigint]>")]
pub fn output_frames_for_source_frame(
    plan: Arg<PresentationPlan>,
    source_frame: Arg<u64>,
) -> Model<Vec<(u64, u64)>> {
    return Model(
        output_frames(&plan.0, source_frame.0)
            .into_iter()
            .map(|span| return (span.start, span.end))
            .collect(),
    );
}
