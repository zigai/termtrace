use std::{io::Write as _, path::Path};

use termtrace_evidence::artifact::{finish_writer, secure_writer};
use termtrace_model::{
    artifact::presentation::{
        ChangeSource, NormalizedClip, PixelTransform, PresentationMapEntry, PresentationPlan,
    },
    artifact::semantic_video::SemanticFrameSpan,
};

use crate::{Error, Result};

pub(super) fn write_presentation_map(
    path: &Path,
    plan: &PresentationPlan,
    semantic_spans: &[SemanticFrameSpan],
) -> Result<()> {
    let mut writer = secure_writer(path)?;
    let mut span_cursor = 0usize;
    let mut mask_cursor = 0usize;
    for (clip_index, clip) in plan.clips.iter().enumerate() {
        for output_frame in clip.output_start_frame..clip.output_end_frame {
            let (source_frame, operation) = mapped_source_frame(clip, output_frame);
            while semantic_spans
                .get(span_cursor)
                .is_some_and(|span| return span.end_frame <= source_frame)
            {
                span_cursor += 1;
            }
            if semantic_spans
                .get(span_cursor)
                .is_some_and(|span| return span.start_frame > source_frame)
            {
                span_cursor =
                    semantic_spans.partition_point(|span| return span.end_frame <= source_frame);
            }
            let semantic_state = semantic_spans.get(span_cursor).filter(|span| {
                return span.start_frame <= source_frame && source_frame < span.end_frame;
            });

            while plan
                .cursor_masks
                .get(mask_cursor)
                .is_some_and(|mask| return mask.output_frame < output_frame)
            {
                mask_cursor += 1;
            }
            let cursor_mask = plan
                .cursor_masks
                .get(mask_cursor)
                .filter(|mask| return mask.output_frame == output_frame);

            let entry = PresentationMapEntry {
                output_frame,
                clip_index,
                source_frame,
                source_state_id: semantic_state.map(|span| return span.state_id),
                source_state_hash: semantic_state.map(|span| return span.state_hash.clone()),
                source_state_monotonic_ns: semantic_state
                    .map(|span| return span.state_monotonic_ns),
                operation: operation.to_owned(),
                captioned: clip.caption.is_some(),
                annotation_indices: plan
                    .annotations
                    .iter()
                    .filter(|annotation| {
                        return annotation.output_start_frame <= output_frame
                            && output_frame < annotation.output_end_frame;
                    })
                    .map(|annotation| return annotation.annotation_index)
                    .collect(),
                transforms: frame_transform_names(plan, output_frame, cursor_mask.is_some()),
                change_state_ids: semantic_change_states_at(plan, output_frame),
                encoded_pixel_change: plan.change_visualization.as_ref().is_some_and(|change| {
                    return change.source == ChangeSource::EncodedPixels && output_frame > 0;
                }),
                cursor_mask_state_id: cursor_mask.map(|mask| return mask.state_id),
            };
            serde_json::to_writer(&mut writer, &entry).map_err(|source| {
                return Error::InvalidJson {
                    path: path.to_path_buf(),
                    json_path: format!("output frame {output_frame}"),
                    source,
                };
            })?;
            writer.write_all(b"\n").map_err(|source| {
                return Error::Filesystem {
                    path: path.to_path_buf(),
                    source,
                };
            })?;
        }
    }
    return Ok(finish_writer(writer, path)?);
}

pub(super) fn read_semantic_frame_spans(path: &Path) -> Result<Vec<SemanticFrameSpan>> {
    let contents = fs_err::read_to_string(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    return contents
        .lines()
        .enumerate()
        .map(|(index, line)| {
            return serde_json::from_str(line).map_err(|source| {
                return Error::InvalidJson {
                    path: path.to_path_buf(),
                    json_path: format!("line {}", index.saturating_add(1)),
                    source,
                };
            });
        })
        .collect();
}

pub(super) fn semantic_change_states_at(plan: &PresentationPlan, output_frame: u64) -> Vec<u64> {
    let hold = plan
        .change_visualization
        .as_ref()
        .map_or(0, |change| return u64::from(change.hold_frames));
    return plan
        .change_frames
        .iter()
        .filter(|change| {
            return change.output_frames.iter().any(|frame| {
                return *frame <= output_frame && output_frame <= frame.saturating_add(hold);
            });
        })
        .map(|change| return change.state_id)
        .collect();
}

pub(super) fn mapped_source_frame(clip: &NormalizedClip, output_frame: u64) -> (u64, &'static str) {
    let offset = output_frame - clip.output_start_frame;
    if offset < clip.retimed_frame_count {
        let source_offset = offset.saturating_mul(u64::from(clip.speed.numerator))
            / u64::from(clip.speed.denominator);
        return (
            clip.source_start_frame
                .saturating_add(source_offset)
                .min(clip.source_end_frame.saturating_sub(1)),
            "source",
        );
    }
    return (clip.source_end_frame.saturating_sub(1), "hold");
}

fn transform_name(transform: &PixelTransform) -> String {
    return match transform {
        PixelTransform::CursorMask => "cursor_mask",
        PixelTransform::Crop { .. } => "crop",
        PixelTransform::Zoom { .. } => "zoom",
        PixelTransform::Resize { .. } => "resize",
        PixelTransform::Callout { .. } => "callout",
    }
    .to_owned();
}

fn frame_transform_names(
    plan: &PresentationPlan,
    output_frame: u64,
    has_cursor_mask: bool,
) -> Vec<String> {
    let mut names = plan
        .transforms
        .iter()
        .filter(|transform| {
            return !matches!(transform, PixelTransform::CursorMask) || has_cursor_mask;
        })
        .map(transform_name)
        .collect::<Vec<_>>();
    if plan.layout.branding.is_some() {
        names.push("branding".to_owned());
    }
    let semantic_states = semantic_change_states_at(plan, output_frame);
    if !semantic_states.is_empty()
        || plan.change_visualization.as_ref().is_some_and(|change| {
            return change.source == ChangeSource::EncodedPixels && output_frame > 0;
        })
    {
        names.push("change_visualization".to_owned());
    }
    return names;
}
