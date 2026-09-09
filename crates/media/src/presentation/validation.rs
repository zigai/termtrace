use super::plan_error;
use crate::{Error, Result};
use std::{
    path::{Path, PathBuf},
    process::Command,
    str,
};
use termtrace_model::artifact::presentation::{
    AnnotationScope, ChangeMode, ChangeSource, ChangeVisualization, EditPlan, EditSelector,
    PixelTransform, PresentationSource, PreviewOptions, TextAnnotation, EDIT_PLAN_VERSION,
};

/// Read and validate a JSON edit plan, resolving relative subtitle paths beside the plan.
///
/// # Errors
///
/// Returns an error when the file cannot be read or decoded, or when any edit selector,
/// transform, annotation, subtitle resource, or version violates the presentation contract.
pub fn read_edit_plan(path: impl AsRef<Path>) -> Result<EditPlan> {
    let path = path.as_ref();
    let bytes = fs_err::read(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    let mut plan: EditPlan = serde_json::from_slice(&bytes).map_err(|source| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source,
        };
    })?;

    let base = path.parent().unwrap_or_else(|| return Path::new("."));
    for subtitle in &mut plan.subtitles {
        if subtitle.ass.is_relative() {
            subtitle.ass = base.join(&subtitle.ass);
        }
    }

    validate_edit_plan(&plan)?;
    return Ok(plan);
}

pub(super) fn validate_edit_plan(plan: &EditPlan) -> Result<()> {
    if plan.version != EDIT_PLAN_VERSION || plan.clips.is_empty() {
        return Err(plan_error(
            "edit plan must use version 1 and contain at least one clip",
        ));
    }

    for clip in &plan.clips {
        validate_edit_selector(&clip.from)?;
        validate_edit_selector(&clip.to)?;

        if clip.speed.numerator == 0 || clip.speed.denominator == 0 {
            return Err(plan_error(
                "clip speed numerator and denominator must be nonzero",
            ));
        }

        if u64::from(clip.speed.numerator) > u64::from(clip.speed.denominator) * 64
            || u64::from(clip.speed.denominator) > u64::from(clip.speed.numerator) * 64
        {
            return Err(plan_error("clip speed must be between 1/64 and 64"));
        }

        if clip.caption.as_ref().is_some_and(|caption| {
            return caption.is_empty() || caption.chars().count() > 4096 || caption.contains('\0');
        }) {
            return Err(plan_error(
                "clip captions must contain 1-4096 non-NUL characters",
            ));
        }
    }

    for annotation in &plan.annotations {
        validate_annotation(annotation, plan.clips.len())?;
    }

    for subtitle in &plan.subtitles {
        if subtitle.ass.as_os_str().is_empty() {
            return Err(plan_error("ASS subtitle path must not be empty"));
        }

        validate_ass_resource(&subtitle.ass)?;
    }

    for transform in &plan.transforms {
        validate_pixel_transform(transform)?;
    }

    if let Some(change) = &plan.change_visualization {
        validate_change_visualization(change)?;
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
        return Err(plan_error(
            "delivery requires H.264 with a compatible pixel format, CRF 0-51, and a supported encoder preset",
        ));
    }

    if plan.layout.branding.as_ref().is_some_and(|branding| {
        return branding.is_empty() || branding.chars().count() > 256 || branding.contains('\0');
    }) {
        return Err(plan_error("branding must contain 1-256 non-NUL characters"));
    }
    return Ok(());
}

pub(super) fn validate_edit_selector(selector: &EditSelector) -> Result<()> {
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
        return Err(plan_error(
            "edit selector requires exactly one marker, monotonicNs, eventSeq, stateId, or nativeFrame",
        ));
    }

    if selector.marker.as_deref().is_some_and(str::is_empty) {
        return Err(plan_error("edit marker must not be empty"));
    }
    return Ok(());
}

fn validate_annotation(annotation: &TextAnnotation, clip_count: usize) -> Result<()> {
    if annotation.text.is_some() == annotation.template.is_some() {
        return Err(plan_error(
            "annotation requires exactly one of text or template",
        ));
    }

    let value = annotation
        .text
        .as_deref()
        .or(annotation.template.as_deref())
        .unwrap_or("");

    if value.is_empty() || value.chars().count() > 4096 || value.contains('\0') {
        return Err(plan_error(
            "annotation text/template must contain 1-4096 non-NUL characters",
        ));
    }

    if let Some(template) = &annotation.template {
        validate_annotation_template(template)?;
    }

    if let AnnotationScope::Clip { clip_index } = annotation.scope
        && clip_index >= clip_count
    {
        return Err(plan_error("annotation clip index is out of bounds"));
    }

    if let AnnotationScope::Range { from, to } = &annotation.scope {
        validate_edit_selector(from)?;
        validate_edit_selector(to)?;
    }

    if annotation.position.x.unsigned_abs() > 32_768
        || annotation.position.y.unsigned_abs() > 32_768
    {
        return Err(plan_error(
            "annotation position offsets must be within 32768 pixels",
        ));
    }

    if !(8..=128).contains(&annotation.style.font_size)
        || !valid_rgb(&annotation.style.foreground)
        || !valid_rgb(&annotation.style.background)
        || annotation.style.border > 16
    {
        return Err(plan_error("annotation style is outside bounded values"));
    }
    return Ok(());
}

fn validate_change_visualization(change: &ChangeVisualization) -> Result<()> {
    if change.source == ChangeSource::SemanticCells && change.pixel_threshold.is_some() {
        return Err(plan_error(
            "pixel threshold is valid only for encoded-pixel change visualization",
        ));
    }

    if change.source == ChangeSource::EncodedPixels && change.mode == ChangeMode::DirtyRows {
        return Err(plan_error(
            "dirty-row change visualization requires semantic-cell evidence",
        ));
    }

    if change.source == ChangeSource::EncodedPixels && change.mode == ChangeMode::BoundingBox {
        return Err(plan_error(
            "encoded-pixel bounding boxes are unavailable; use outline, heatmap, trail, or side_diff",
        ));
    }

    if change.hold_frames > 240 {
        return Err(plan_error("change overlay hold must not exceed 240 frames"));
    }
    return Ok(());
}

fn validate_pixel_transform(transform: &PixelTransform) -> Result<()> {
    match transform {
        PixelTransform::Crop { width, height, .. }
        | PixelTransform::Callout { width, height, .. }
            if *width == 0 || *height == 0 =>
        {
            return Err(plan_error("crop/callout dimensions must be nonzero"));
        }
        PixelTransform::Resize { width, height }
            if *width < 2
                || *height < 2
                || *width > 8192
                || *height > 8192
                || *width % 2 != 0
                || *height % 2 != 0 =>
        {
            return Err(plan_error(
                "resize dimensions must be even values from 2 through 8192",
            ));
        }
        PixelTransform::Zoom {
            numerator,
            denominator,
        } if *numerator == 0
            || *denominator == 0
            || u32::from(*numerator) > u32::from(*denominator) * 16
            || u32::from(*denominator) > u32::from(*numerator) * 16 =>
        {
            return Err(plan_error(
                "zoom ratio must be positive and between 1/16 and 16",
            ));
        }
        PixelTransform::Callout {
            label: Some(label), ..
        } if label.is_empty() || label.chars().count() > 256 || label.contains('\0') => {
            return Err(plan_error(
                "callout labels must contain 1-256 non-NUL characters",
            ));
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

pub(super) fn presentation_source_truth_layer(source: &PresentationSource) -> &'static str {
    match source {
        PresentationSource::Native { .. } => return "native_encoded_evidence",
        PresentationSource::Semantic { .. } => return "semantic_rendering",
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

fn validate_annotation_template(template: &str) -> Result<()> {
    let mut token: Option<String> = None;

    for character in template.chars() {
        match (character, &mut token) {
            ('{', None) => token = Some(String::new()),
            ('{', Some(_)) => return Err(plan_error("annotation template has a nested token")),
            ('}', None) => {
                return Err(plan_error(
                    "annotation template has an unmatched closing brace",
                ));
            }
            ('}', Some(value)) => {
                if !matches!(
                    value.as_str(),
                    "output_frame" | "source_frame" | "source_time" | "marker" | "timecode"
                ) {
                    return Err(plan_error(format!(
                        "annotation template token `{{{value}}}` is unsupported"
                    )));
                }

                token = None;
            }
            (character, Some(value)) => value.push(character),
            (_, None) => {}
        }
    }

    if token.is_some() {
        return Err(plan_error("annotation template has an unclosed token"));
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
