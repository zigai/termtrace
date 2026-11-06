use super::{plan_error, planning::hash_edit_identity};
use crate::{Error, Result};
use std::{
    path::{Path, PathBuf},
    process::Command,
    str,
};
use termtrace_model::artifact::{
    presentation::{
        AnnotationScope, ChangeMode, ChangeSource, ChangeVisualization, EditFailure,
        EditFailureCode, EditPlan, EditSelector, NormalizedEdit, PixelTransform,
        PresentationSource, PreviewOptions, TextAnnotation, EDIT_PLAN_VERSION,
    },
    sdk::ArtifactSourceKind,
};

/// Read a JSON edit plan and normalize it, resolving relative subtitle paths beside the plan.
///
/// # Errors
///
/// Returns an error when the file cannot be read or decoded, or when [`normalize_edit`] rejects
/// the plan.
pub fn read_edit_plan(path: impl AsRef<Path>) -> Result<NormalizedEdit> {
    let path = path.as_ref();
    let bytes = fs_err::read(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

    let plan: EditPlan = serde_json::from_slice(&bytes).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    let base = path.parent().unwrap_or_else(|| return Path::new("."));

    return normalize_edit(&plan, Some(base));
}

/// Resolve subtitle paths, validate the edit, and compute its identity.
///
/// Absolute subtitle paths are used as given. A relative path is joined to `resource_base`;
/// without one it fails with `relative_resource_without_base` rather than depending on the
/// process working directory. Each ASS resource is read and validated now and hashed into the
/// path-independent `edit_plan_hash`.
///
/// # Errors
///
/// Returns [`Error::Edit`] for a relative path without a base and for every edit-contract
/// violation, and filesystem errors when a subtitle resource cannot be read.
pub fn normalize_edit(edit: &EditPlan, resource_base: Option<&Path>) -> Result<NormalizedEdit> {
    let mut edit = edit.clone();
    for (index, subtitle) in edit.subtitles.iter_mut().enumerate() {
        if subtitle.ass.is_absolute() {
            continue;
        }

        let base = resource_base.ok_or_else(|| {
            return edit_error(
                EditFailureCode::RelativeResourceWithoutBase,
                format!("subtitles[{index}].ass"),
                format!(
                    "relative subtitle path `{}` needs an explicit resource base directory",
                    subtitle.ass.display()
                ),
            );
        })?;
        subtitle.ass = base.join(&subtitle.ass);
    }

    validate_edit_plan(&edit)?;
    let edit_plan_hash = hash_edit_identity(&edit)?;

    return Ok(NormalizedEdit {
        edit,
        edit_plan_hash,
    });
}

pub(super) fn edit_error(
    code: EditFailureCode,
    field: impl Into<String>,
    reason: impl Into<String>,
) -> Error {
    return Error::Edit(EditFailure::new(code, field, reason));
}

fn clip_error(
    code: EditFailureCode,
    clip_index: usize,
    field: &str,
    reason: impl Into<String>,
) -> Error {
    return Error::Edit(
        EditFailure::new(code, format!("clips[{clip_index}]{field}"), reason).clip(clip_index),
    );
}

pub(super) fn validate_edit_plan(plan: &EditPlan) -> Result<()> {
    if plan.version != EDIT_PLAN_VERSION || plan.clips.is_empty() {
        return Err(edit_error(
            EditFailureCode::InvalidEditPlan,
            if plan.version == EDIT_PLAN_VERSION { "clips" } else { "version" },
            "edit plan must use version 2 and contain at least one clip",
        ));
    }

    for (index, clip) in plan.clips.iter().enumerate() {
        validate_edit_selector(&clip.from, &format!("clips[{index}].from"))
            .map_err(|error| return with_clip(error, index))?;
        validate_edit_selector(&clip.to, &format!("clips[{index}].to"))
            .map_err(|error| return with_clip(error, index))?;

        if clip.speed.numerator == 0 || clip.speed.denominator == 0 {
            return Err(clip_error(
                EditFailureCode::InvalidSpeed,
                index,
                ".speed",
                "clip speed numerator and denominator must be nonzero",
            ));
        }

        if u64::from(clip.speed.numerator) > u64::from(clip.speed.denominator) * 64
            || u64::from(clip.speed.denominator) > u64::from(clip.speed.numerator) * 64
        {
            return Err(clip_error(
                EditFailureCode::InvalidSpeed,
                index,
                ".speed",
                "clip speed must be between 1/64 and 64",
            ));
        }

        if clip.caption.as_ref().is_some_and(|caption| {
            return caption.is_empty() || caption.chars().count() > 4096 || caption.contains('\0');
        }) {
            return Err(clip_error(
                EditFailureCode::InvalidCaption,
                index,
                ".caption",
                "clip captions must contain 1-4096 non-NUL characters",
            ));
        }
    }

    for (index, annotation) in plan.annotations.iter().enumerate() {
        validate_annotation(annotation, index, plan.clips.len())?;
    }

    for (index, subtitle) in plan.subtitles.iter().enumerate() {
        let field = format!("subtitles[{index}].ass");
        if subtitle.ass.as_os_str().is_empty() {
            return Err(edit_error(
                EditFailureCode::InvalidSubtitle,
                field,
                "ASS subtitle path must not be empty",
            ));
        }

        validate_ass_resource(&subtitle.ass).map_err(|error| {
            return match error {
                Error::InvalidReplay { reason, .. } => {
                    edit_error(EditFailureCode::InvalidSubtitle, field.clone(), reason)
                }
                Error::Filesystem { path, source } if path == subtitle.ass => {
                    let error = Error::Filesystem { path, source };

                    edit_error(
                        EditFailureCode::InvalidSubtitle,
                        field.clone(),
                        error.to_string(),
                    )
                }
                other => other,
            };
        })?;
    }

    for (index, transform) in plan.transforms.iter().enumerate() {
        validate_pixel_transform(transform).map_err(|reason| {
            return edit_error(
                EditFailureCode::InvalidTransform,
                format!("transforms[{index}]"),
                reason,
            );
        })?;
    }

    if let Some(change) = &plan.change_visualization {
        validate_change_visualization(change).map_err(|reason| {
            return edit_error(
                EditFailureCode::InvalidChangeVisualization,
                "change_visualization",
                reason,
            );
        })?;
    }

    let delivery_format_valid = matches!(
        (
            plan.delivery.codec.as_str(),
            plan.delivery.pixel_format.as_str()
        ),
        ("h264", "yuv420p" | "yuv444p") | ("h264_rgb", "rgb24")
    );

    if !delivery_format_valid
        || plan.delivery.crf > 51
        || !matches!(
            plan.delivery.encoder_preset.as_str(),
            "medium" | "slow" | "veryslow"
        )
    {
        return Err(edit_error(
            EditFailureCode::InvalidEditPlan,
            "delivery",
            "delivery requires H.264 with a compatible pixel format, CRF 0-51, and a supported encoder preset",
        ));
    }

    if plan.layout.branding.as_ref().is_some_and(|branding| {
        return branding.is_empty() || branding.chars().count() > 256 || branding.contains('\0');
    }) {
        return Err(edit_error(
            EditFailureCode::InvalidEditPlan,
            "layout.branding",
            "branding must contain 1-256 non-NUL characters",
        ));
    }
    return Ok(());
}

fn with_clip(error: Error, clip_index: usize) -> Error {
    return match error {
        Error::Edit(failure) => Error::Edit(failure.clip(clip_index)),
        other => other,
    };
}

pub(super) fn validate_edit_selector(selector: &EditSelector, field: &str) -> Result<()> {
    let count = [
        selector.marker.is_some(),
        selector.monotonic_ns.is_some(),
        selector.event_seq.is_some(),
        selector.state_id.is_some(),
        selector.native_frame.is_some(),
    ]
    .into_iter()
    .filter(|selected| return *selected)
    .count();

    if count != 1 {
        return Err(edit_error(
            EditFailureCode::InvalidSelector,
            field,
            "edit selector requires exactly one marker, monotonicNs, eventSeq, stateId, or nativeFrame",
        ));
    }

    if selector.marker.as_deref().is_some_and(str::is_empty) {
        return Err(edit_error(
            EditFailureCode::InvalidSelector,
            field,
            "edit marker must not be empty",
        ));
    }
    return Ok(());
}

fn validate_annotation(annotation: &TextAnnotation, index: usize, clip_count: usize) -> Result<()> {
    let invalid = |field: &str, reason: String| {
        return edit_error(
            EditFailureCode::InvalidAnnotation,
            format!("annotations[{index}]{field}"),
            reason,
        );
    };

    if annotation.text.is_some() == annotation.template.is_some() {
        return Err(invalid(
            "",
            "annotation requires exactly one of text or template".to_owned(),
        ));
    }

    let value = annotation
        .text
        .as_deref()
        .or(annotation.template.as_deref())
        .unwrap_or("");

    if value.is_empty() || value.chars().count() > 4096 || value.contains('\0') {
        return Err(invalid(
            "",
            "annotation text/template must contain 1-4096 non-NUL characters".to_owned(),
        ));
    }

    if let Some(template) = &annotation.template {
        validate_annotation_template(template).map_err(|reason| {
            return invalid(".template", reason);
        })?;
    }

    if let AnnotationScope::Clip { clip_index } = annotation.scope
        && clip_index >= clip_count
    {
        return Err(invalid(
            ".scope",
            format!("annotation clip index {clip_index} is out of bounds for {clip_count} clips"),
        ));
    }

    if let AnnotationScope::Range { from, to } = &annotation.scope {
        validate_edit_selector(from, &format!("annotations[{index}].scope.from"))?;
        validate_edit_selector(to, &format!("annotations[{index}].scope.to"))?;
    }

    if annotation.position.x.unsigned_abs() > 32_768
        || annotation.position.y.unsigned_abs() > 32_768
    {
        return Err(invalid(
            ".position",
            "annotation position offsets must be within 32768 pixels".to_owned(),
        ));
    }

    if !(8..=128).contains(&annotation.style.font_size)
        || !valid_rgb(&annotation.style.foreground)
        || !valid_rgb(&annotation.style.background)
        || annotation.style.border > 16
    {
        return Err(invalid(
            ".style",
            "annotation style is outside bounded values".to_owned(),
        ));
    }
    return Ok(());
}

fn validate_change_visualization(change: &ChangeVisualization) -> Result<(), &'static str> {
    if change.source == ChangeSource::SemanticCells && change.pixel_threshold.is_some() {
        return Err("pixel threshold is valid only for encoded-pixel change visualization");
    }

    if change.source == ChangeSource::EncodedPixels && change.mode == ChangeMode::DirtyRows {
        return Err("dirty-row change visualization requires semantic-cell evidence");
    }

    if change.source == ChangeSource::EncodedPixels && change.mode == ChangeMode::BoundingBox {
        return Err(
            "encoded-pixel bounding boxes are unavailable; use outline, heatmap, trail, or side_diff",
        );
    }

    if change.hold_frames > 240 {
        return Err("change overlay hold must not exceed 240 frames");
    }
    return Ok(());
}

fn validate_pixel_transform(transform: &PixelTransform) -> Result<(), &'static str> {
    match transform {
        PixelTransform::Crop { width, height, .. }
        | PixelTransform::Callout { width, height, .. }
            if *width == 0 || *height == 0 =>
        {
            return Err("crop/callout dimensions must be nonzero");
        }
        PixelTransform::Resize { width, height }
            if *width < 2
                || *height < 2
                || *width > 8192
                || *height > 8192
                || *width % 2 != 0
                || *height % 2 != 0 =>
        {
            return Err("resize dimensions must be even values from 2 through 8192");
        }
        PixelTransform::Zoom {
            numerator,
            denominator,
        } if *numerator == 0
            || *denominator == 0
            || u32::from(*numerator) > u32::from(*denominator) * 16
            || u32::from(*denominator) > u32::from(*numerator) * 16 =>
        {
            return Err("zoom ratio must be positive and between 1/16 and 16");
        }
        PixelTransform::Callout {
            label: Some(label), ..
        } if label.is_empty() || label.chars().count() > 256 || label.contains('\0') => {
            return Err("callout labels must contain 1-256 non-NUL characters");
        }
        _ => {}
    }

    return Ok(());
}

pub(super) fn validate_preview_options(options: PreviewOptions) -> Result<()> {
    if !(1..=10).contains(&options.columns)
        || !(80..=1920).contains(&options.tile_width)
        || !(1..=100).contains(&options.max_frames)
        || !(160..=1920).contains(&options.preview_width)
        || !options.preview_width.is_multiple_of(2)
    {
        return Err(plan_error(
            "preview dimensions or frame count are out of bounds",
        ));
    }
    return Ok(());
}

pub(super) fn presentation_source_identity_hash(source: &PresentationSource) -> Result<String> {
    match source {
        PresentationSource::Native { video_hash, .. } => return Ok(video_hash.clone()),
        PresentationSource::Semantic { plan } => {
            let identity = serde_json::json!({
                "version": plan.version,
                "session_id": plan.session_id,
                "source_event_hash": &plan.source_event_hash,
                "start_monotonic_ns": plan.start_monotonic_ns,
                "end_monotonic_ns": plan.end_monotonic_ns,
                "fps": plan.fps,
                "frame_count": plan.frame_count,
                "canvas": &plan.canvas,
                "renderer": &plan.renderer,
                "fonts": &plan.fonts,
                "font_set_blake3": &plan.font_set_blake3,
                "scheduling_policy": &plan.scheduling_policy,
            });

            let bytes = serde_json::to_vec(&identity).map_err(|source| {
                return Error::InvalidJson {
                    path: PathBuf::from("<semantic-source-plan>"),
                    json_path: "$".to_owned(),
                    source,
                };
            })?;

            return Ok(blake3::hash(&bytes).to_hex().to_string());
        }
    }
}

pub(super) fn presentation_source_truth_layer(
    source: &PresentationSource,
    kind: ArtifactSourceKind,
) -> Result<&'static str> {
    match (source, kind) {
        (PresentationSource::Semantic { .. }, ArtifactSourceKind::SemanticRendering) => {
            return Ok("semantic_rendering");
        }
        (PresentationSource::Native { .. }, ArtifactSourceKind::NativeReplay) => {
            return Ok("native_replay");
        }
        (PresentationSource::Native { .. }, ArtifactSourceKind::OriginalExecution) => {
            return Ok("original_execution");
        }
        _ => {
            return Err(plan_error(
                "presentation source does not match the selected source kind",
            ))
        }
    }
}

pub(super) fn validate_ass_resource(path: &Path) -> Result<()> {
    let metadata = fs_err::metadata(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

    if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
        return Err(plan_error(format!(
            "ASS resource `{}` must be a regular file no larger than 16 MiB",
            path.display()
        )));
    }

    let bytes = fs_err::read(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

    str::from_utf8(&bytes).map_err(|error| {
        return plan_error(format!("ASS subtitle resource must be UTF-8: {error}"));
    })?;

    let escaped = super::filter_path_value(path)?;
    let filter = format!("ass='{escaped}'");
    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "color=size=64x64:rate=24:duration=0.04",
            "-vf",
            &filter,
            "-f",
            "null",
            "-",
        ])
        .output()
        .map_err(|source| {
            return Error::Filesystem {
                path: PathBuf::from("ffmpeg"),
                source,
            };
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);

        return Err(plan_error(format!(
            "ASS subtitle resource failed FFmpeg validation: {}",
            stderr.trim()
        )));
    }
    return Ok(());
}

fn validate_annotation_template(template: &str) -> Result<(), String> {
    let mut token: Option<String> = None;
    for character in template.chars() {
        match (character, &mut token) {
            ('{', None) => token = Some(String::new()),
            ('{', Some(_)) => return Err("annotation template has a nested token".to_owned()),
            ('}', None) => {
                return Err("annotation template has an unmatched closing brace".to_owned());
            }
            ('}', Some(value)) => {
                if !matches!(
                    value.as_str(),
                    "output_frame" | "source_frame" | "source_time" | "marker" | "timecode"
                ) {
                    return Err(format!(
                        "annotation template token `{{{value}}}` is unsupported"
                    ));
                }

                token = None;
            }
            (character, Some(value)) => value.push(character),
            (_, None) => {}
        }
    }

    if token.is_some() {
        return Err("annotation template has an unclosed token".to_owned());
    }
    return Ok(());
}

fn valid_rgb(value: &str) -> bool {
    return value.len() == 7
        && value.starts_with('#')
        && value
            .as_bytes()
            .get(1..)
            .is_some_and(|hex| return hex.iter().all(u8::is_ascii_hexdigit));
}
