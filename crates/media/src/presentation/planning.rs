use std::{
    collections::{BTreeMap, HashMap},
    io::ErrorKind,
    path::{Path, PathBuf},
};

use termtrace_engine::{
    replay::{StateRecord, StateSelection, replay},
    timeline::resolve_source_selection,
};
use termtrace_evidence::{
    artifact::{hash_regular_file, verify_artifact_members},
    event_log::EventLog,
    session::{read_manifest, validate_completed_log},
};
use termtrace_model::{
    artifact::presentation::{
        AnnotationScope, CaptionLayout, ChangeMode, ChangeRegion, ChangeSource,
        ChangeVisualization, CursorMaskFrame, EditClip, EditPlan, EditSelector, MarkerRecord,
        NormalizedAnnotation, NormalizedChangeFrame, NormalizedClip, NormalizedSubtitle,
        PRESENTATION_FORMAT_VERSION, PixelTransform, PresentationPlan, PresentationSource,
        PresentationSourceRequest,
    },
    artifact::visual::{VisualManifest, visual_directory_name_for},
    execution::{event::Event, session::SessionManifest},
    terminal::replay::SourceRange,
    terminal::state::{CursorState, CursorStyle},
    value::geometry::TerminalGeometryReport,
};

use crate::{
    Error, Result,
    native_capture::validate_visual_manifest_binding,
    semantic_video::{materialize as materialize_semantic_video, plan as plan_semantic_video},
};

use super::{
    NANOS_PER_SECOND,
    mapping::mapped_source_frame,
    plan_error,
    validation::{validate_edit_plan, validate_edit_selector},
};

/// List every canonical marker without claiming native presentation.
pub fn markers(session: impl AsRef<Path>, fps: u16) -> Result<Vec<MarkerRecord>> {
    if fps == 0 {
        return Err(plan_error("marker frame rate must be nonzero"));
    }
    let log = EventLog::read(session.as_ref().join("events.jsonl"))?;
    let states = replay(&log)?.states;
    return marker_records(&log, &states, fps, 0);
}

fn marker_records(
    log: &EventLog,
    states: &[StateRecord],
    fps: u16,
    capture_start_ns: u64,
) -> Result<Vec<MarkerRecord>> {
    let mut occurrence_counts = HashMap::new();
    for record in &log.events {
        if let Event::Marker { label, .. } = &record.event {
            let occurrence = occurrence_counts
                .entry(label.as_str())
                .or_insert((0_u64, 0_u64));
            occurrence.0 = occurrence.0.saturating_add(1);
        }
    }
    let presented_states = states
        .iter()
        .filter(|state| return state.presented)
        .collect::<Vec<_>>();
    let mut records = Vec::new();
    for record in &log.events {
        let Event::Marker { label, .. } = &record.event else {
            continue;
        };
        let occurrence = occurrence_counts
            .get_mut(label.as_str())
            .ok_or_else(|| return plan_error("marker occurrence accounting is inconsistent"))?;
        occurrence.1 = occurrence.1.saturating_add(1);
        let (occurrence_count, occurrence_index) = *occurrence;
        let state_index = presented_states
            .partition_point(|state| return state.monotonic_ns <= record.monotonic_ns);
        let semantic_state = state_index
            .checked_sub(1)
            .and_then(|index| return presented_states.get(index).copied());
        records.push(MarkerRecord {
            label: label.clone(),
            event_seq: record.seq,
            monotonic_ns: record.monotonic_ns,
            nominal_frame: frame_floor(record.monotonic_ns.saturating_sub(capture_start_ns), fps),
            occurrence_count,
            occurrence_index,
            semantic_state_id: semantic_state.map(|state| return state.state_id),
            semantic_state_hash: semantic_state.map(|state| return state.state_hash.clone()),
            presentation_eligible: semantic_state.is_some(),
            native_evidence_level: "scheduled_for_frame".to_owned(),
        });
    }
    return Ok(records);
}

/// Resolve a strict edit plan against the default verified native capture.
pub fn plan(session: impl AsRef<Path>, edit: &EditPlan) -> Result<PresentationPlan> {
    return plan_with_source(session, edit, PresentationSourceRequest::Native);
}

/// Resolve a strict edit plan against an explicit native or semantic source.
///
/// # Errors
///
/// Returns an error when canonical evidence, the selected source, or any edit
/// selector or transform is invalid.
pub fn plan_with_source(
    session: impl AsRef<Path>,
    edit: &EditPlan,
    source_request: PresentationSourceRequest,
) -> Result<PresentationPlan> {
    validate_edit_plan(edit)?;
    let session = session.as_ref();
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(session, &log)?;
    let session_manifest = read_manifest(session)?;
    let states = replay(&log)?.states;
    let ResolvedSource {
        source,
        fps,
        source_width,
        source_height,
        source_geometry,
        capture_start_ns: capture_start,
        capture_range,
        semantic_final_frame,
    } = resolve_source(
        session,
        edit,
        source_request,
        &log,
        &states,
        &session_manifest,
    )?;
    let edit_plan_hash = hash_edit_identity(edit)?;
    let NormalizedClips {
        clips,
        output_frame_count,
    } = normalize_clips(
        &log,
        &states,
        edit,
        capture_start,
        fps,
        capture_range,
        semantic_final_frame,
    )?;
    let annotations = normalize_annotations(
        &log,
        &states,
        edit,
        &clips,
        output_frame_count,
        capture_start,
        fps,
    )?;
    let change_frames = normalize_semantic_changes(
        &states,
        edit.change_visualization.as_ref(),
        &clips,
        capture_start,
        fps,
        source_width,
        source_height,
        source_geometry,
    )?;
    let cursor_masks = normalize_cursor_masks(
        &states,
        &edit.transforms,
        &clips,
        capture_start,
        fps,
        source_width,
        source_height,
        source_geometry,
    )?;
    let (output_width, output_height) = validate_transform_geometry(
        edit,
        source_width,
        source_height,
        edit.clips.iter().any(|clip| return clip.caption.is_some()),
    )?;
    return Ok(PresentationPlan {
        version: PRESENTATION_FORMAT_VERSION,
        edit_plan_hash,
        source,
        fps,
        source_width,
        source_height,
        source_geometry,
        output_width,
        output_height,
        render_config: edit.render_config.clone(),
        clips,
        markers: marker_records(&log, &states, fps, capture_start)?,
        annotations,
        subtitles: edit
            .subtitles
            .iter()
            .map(|subtitle| {
                return Ok(NormalizedSubtitle {
                    source_path: subtitle.ass.clone(),
                    resource_hash: hash_file(&subtitle.ass)?,
                });
            })
            .collect::<Result<Vec<_>>>()?,
        change_visualization: edit.change_visualization.clone(),
        change_frames,
        change_algorithm: edit.change_visualization.as_ref().map(|change| {
            return match change.source {
                ChangeSource::SemanticCells => "semantic-state-delta-to-source-frame-v1",
                ChangeSource::EncodedPixels => "ffmpeg-adjacent-decoded-rgb-difference-v1",
            }
            .to_owned();
        }),
        cursor_masks,
        layout: edit.layout.clone(),
        delivery: edit.delivery.clone(),
        transforms: edit.transforms.clone(),
        output_frame_count,
    });
}

struct ResolvedSource {
    source: PresentationSource,
    fps: u16,
    source_width: u16,
    source_height: u16,
    source_geometry: TerminalGeometryReport,
    capture_start_ns: u64,
    capture_range: Option<SourceRange>,
    semantic_final_frame: Option<(u64, u64)>,
}

struct NormalizedClips {
    clips: Vec<NormalizedClip>,
    output_frame_count: u64,
}

fn resolve_source(
    session: &Path,
    edit: &EditPlan,
    source_request: PresentationSourceRequest,
    log: &EventLog,
    states: &[StateRecord],
    session_manifest: &SessionManifest,
) -> Result<ResolvedSource> {
    return match source_request {
        PresentationSourceRequest::Native => {
            let visual = session.join(visual_directory_name_for(None, StateSelection::All));
            let manifest_path = visual.join("manifest.json");
            let manifest_bytes = fs_err::read(&manifest_path).map_err(|source| {
                if source.kind() == ErrorKind::NotFound {
                    return missing_native_capture_error(session, &manifest_path);
                }
                return Error::Filesystem {
                    path: manifest_path.clone(),
                    source,
                };
            })?;
            let manifest: VisualManifest =
                serde_json::from_slice(&manifest_bytes).map_err(|source| {
                    return Error::InvalidJson {
                        path: manifest_path.clone(),
                        json_path: "$".to_owned(),
                        source,
                    };
                })?;
            validate_visual_manifest_binding(
                &manifest,
                session_manifest
                    .ghostty_visual
                    .as_ref()
                    .map(|profile| return profile.effective_config_hash.as_str()),
                &log.header.colors.source,
            )?;
            verify_artifact_members(&visual, &manifest.members)?;
            if manifest.session_id != log.header.session_id {
                return Err(plan_error(
                    "native capture session identity does not match canonical evidence",
                ));
            }
            let source_video = visual.join("session.mkv");
            let source_video_hash = manifest
                .members
                .iter()
                .find(|member| return member.path == "session.mkv")
                .map(|member| return member.blake3.clone())
                .ok_or_else(|| return plan_error("native capture manifest has no session video"))?;
            let capture_start_ns = manifest
                .source_range
                .map_or(0, |capture| return capture.start_monotonic_ns);
            Ok(ResolvedSource {
                source: PresentationSource::Native {
                    video: source_video,
                    video_hash: source_video_hash,
                },
                fps: manifest.fps,
                source_width: manifest.width,
                source_height: manifest.height,
                source_geometry: manifest.geometry_report,
                capture_start_ns,
                capture_range: manifest.source_range,
                semantic_final_frame: None,
            })
        }
        PresentationSourceRequest::Semantic {
            fps,
            raster_profile,
        } => {
            if edit.clips.iter().any(|clip| {
                return clip.from.native_frame.is_some() || clip.to.native_frame.is_some();
            }) {
                return Err(plan_error(
                    "nativeFrame edit selectors are invalid for a semantic video source",
                ));
            }
            let mut semantic_start = u64::MAX;
            let mut semantic_end = 0_u64;
            for clip in &edit.clips {
                let range = resolve_clip(log, states, clip, 0, fps)?;
                semantic_start = semantic_start.min(range.start_monotonic_ns);
                semantic_end = semantic_end.max(range.end_monotonic_ns);
            }
            let semantic = plan_semantic_video(
                session,
                log,
                fps,
                semantic_start,
                semantic_end,
                raster_profile,
            )?;
            let geometry = TerminalGeometryReport {
                cols: semantic.canvas.cols,
                rows: semantic.canvas.rows,
                cell_width: semantic.canvas.cell_width,
                cell_height: semantic.canvas.cell_height,
                text_width: semantic.canvas.width,
                text_height: semantic.canvas.height,
            };
            let final_frame = (semantic.end_monotonic_ns, semantic.frame_count);
            Ok(ResolvedSource {
                source: PresentationSource::Semantic {
                    plan: semantic.clone(),
                },
                fps: semantic.fps,
                source_width: semantic.canvas.width,
                source_height: semantic.canvas.height,
                source_geometry: geometry,
                capture_start_ns: semantic.start_monotonic_ns,
                capture_range: None,
                semantic_final_frame: Some(final_frame),
            })
        }
    };
}

fn hash_edit_identity(edit: &EditPlan) -> Result<String> {
    let mut hashed_edit = edit.clone();
    let mut subtitle_hashes = Vec::new();
    for (index, subtitle) in hashed_edit.subtitles.iter_mut().enumerate() {
        subtitle_hashes.push(hash_file(&subtitle.ass)?);
        subtitle.ass = PathBuf::from(format!("resource-{index}.ass"));
    }
    let edit_identity = serde_json::json!({
        "edit": hashed_edit,
        "subtitle_hashes": subtitle_hashes,
    });
    return Ok(
        blake3::hash(&serde_json::to_vec(&edit_identity).map_err(|source| {
            return Error::InvalidJson {
                path: PathBuf::from("<edit-plan>"),
                json_path: "$".to_owned(),
                source,
            };
        })?)
        .to_hex()
        .to_string(),
    );
}

fn normalize_clips(
    log: &EventLog,
    states: &[StateRecord],
    edit: &EditPlan,
    capture_start_ns: u64,
    fps: u16,
    capture_range: Option<SourceRange>,
    semantic_final_frame: Option<(u64, u64)>,
) -> Result<NormalizedClips> {
    let mut output_start_frame = 0_u64;
    let mut clips = Vec::new();
    for clip in &edit.clips {
        let range = resolve_clip(log, states, clip, capture_start_ns, fps)?;
        if clips.iter().any(|existing: &NormalizedClip| {
            return ranges_overlap(range, existing.source_range);
        }) {
            return Err(plan_error("edit clips must not overlap in source time"));
        }
        if capture_range.is_some_and(|capture| {
            return range.start_monotonic_ns < capture.start_monotonic_ns
                || range.end_monotonic_ns > capture.end_monotonic_ns;
        }) {
            return Err(plan_error(
                "edit clip lies outside the native capture range",
            ));
        }
        let source_start_frame = frame_floor(range.start_monotonic_ns - capture_start_ns, fps);
        let source_end_frame = semantic_final_frame.map_or_else(
            || return frame_ceil(range.end_monotonic_ns - capture_start_ns, fps),
            |(semantic_end, final_frame)| {
                return if range.end_monotonic_ns == semantic_end {
                    final_frame
                } else {
                    frame_ceil(range.end_monotonic_ns - capture_start_ns, fps)
                };
            },
        );
        let source_frames = source_end_frame.saturating_sub(source_start_frame);
        let retimed_frame_count = ceil_ratio(
            source_frames.saturating_mul(u64::from(clip.speed.denominator)),
            u64::from(clip.speed.numerator),
        );
        let hold_frame_count = ceil_ratio(clip.hold_ms.saturating_mul(u64::from(fps)), 1000);
        let output_end_frame = output_start_frame
            .saturating_add(retimed_frame_count)
            .saturating_add(hold_frame_count);
        clips.push(NormalizedClip {
            source_range: range,
            source_start_frame,
            source_end_frame,
            output_start_frame,
            output_end_frame,
            retimed_frame_count,
            hold_frame_count,
            speed: clip.speed,
            caption: clip.caption.clone(),
        });
        output_start_frame = output_end_frame;
    }
    if output_start_frame == 0 || output_start_frame > u64::from(fps) * 60 * 60 * 24 {
        return Err(plan_error(
            "presentation duration must contain between one frame and 24 hours",
        ));
    }
    return Ok(NormalizedClips {
        clips,
        output_frame_count: output_start_frame,
    });
}

pub(super) fn validate_planned_source(source: &PresentationSource) -> Result<()> {
    return match source {
        PresentationSource::Native { video, video_hash } => {
            if hash_file(video)? != *video_hash {
                return Err(plan_error(
                    "native source video changed after edit planning",
                ));
            }
            Ok(())
        }
        PresentationSource::Semantic { plan } => {
            let log = EventLog::read(plan.session.join("events.jsonl"))?;
            validate_completed_log(&plan.session, &log)?;
            let final_event = log.events.last().ok_or_else(|| {
                return plan_error("semantic source event log is empty after planning");
            })?;
            if log.header.session_id != plan.session_id
                || final_event.hash != plan.source_event_hash
            {
                return Err(plan_error(
                    "semantic canonical source changed after edit planning",
                ));
            }
            Ok(())
        }
    };
}

pub(super) fn materialize_planned_source(
    plan: &PresentationPlan,
    staging: &Path,
) -> Result<PathBuf> {
    return match &plan.source {
        PresentationSource::Native { video, .. } => video.canonicalize().map_err(|source| {
            return Error::Filesystem {
                path: video.clone(),
                source,
            };
        }),
        PresentationSource::Semantic { plan } => {
            let (video, _manifest) = materialize_semantic_video(plan, staging)?;
            Ok(video)
        }
    };
}

fn normalize_annotations(
    log: &EventLog,
    states: &[StateRecord],
    edit: &EditPlan,
    clips: &[NormalizedClip],
    output_frame_count: u64,
    capture_start_ns: u64,
    fps: u16,
) -> Result<Vec<NormalizedAnnotation>> {
    let mut normalized = Vec::new();
    for (annotation_index, annotation) in edit.annotations.iter().enumerate() {
        let spans = match &annotation.scope {
            AnnotationScope::WholeVideo => vec![(0, output_frame_count)],
            AnnotationScope::Clip { clip_index } => {
                let clip = clips
                    .get(*clip_index)
                    .ok_or_else(|| return plan_error("annotation clip index is out of bounds"))?;
                vec![(clip.output_start_frame, clip.output_end_frame)]
            }
            AnnotationScope::OutputFrame { output_frame } => {
                if *output_frame >= output_frame_count {
                    return Err(plan_error(format!(
                        "annotation output frame {output_frame} is out of bounds"
                    )));
                }
                vec![(*output_frame, output_frame + 1)]
            }
            AnnotationScope::Range { from, to } => {
                let start = resolve_edit_selector(log, states, from, capture_start_ns, fps)?;
                let end = resolve_edit_selector(log, states, to, capture_start_ns, fps)?;
                if start >= end {
                    return Err(plan_error("annotation source range is empty or reversed"));
                }
                let start_frame = frame_floor(start.saturating_sub(capture_start_ns), fps);
                let end_frame = frame_ceil(end.saturating_sub(capture_start_ns), fps);
                output_spans_for_source_frames(clips, start_frame, end_frame)
            }
        };
        if spans.is_empty() {
            return Err(plan_error(format!(
                "annotation {annotation_index} does not intersect an output clip"
            )));
        }
        for (output_start_frame, output_end_frame) in spans {
            normalized.push(NormalizedAnnotation {
                annotation_index,
                output_start_frame,
                output_end_frame,
                text: annotation.text.clone(),
                template: annotation.template.clone(),
                position: annotation.position.clone(),
                style: annotation.style.clone(),
                z_order: annotation.z_order,
            });
        }
    }
    normalized.sort_by_key(|annotation| {
        return (
            annotation.z_order,
            annotation.annotation_index,
            annotation.output_start_frame,
        );
    });
    return Ok(normalized);
}

fn output_spans_for_source_frames(
    clips: &[NormalizedClip],
    source_start_frame: u64,
    source_end_frame: u64,
) -> Vec<(u64, u64)> {
    let mut spans = Vec::new();
    for clip in clips {
        let mut open = None;
        for output_frame in clip.output_start_frame..clip.output_end_frame {
            let (source_frame, _) = mapped_source_frame(clip, output_frame);
            let selected = source_start_frame <= source_frame && source_frame < source_end_frame;
            match (open, selected) {
                (None, true) => open = Some(output_frame),
                (Some(start), false) => {
                    spans.push((start, output_frame));
                    open = None;
                }
                _ => {}
            }
        }
        if let Some(start) = open {
            spans.push((start, clip.output_end_frame));
        }
    }
    return spans;
}

#[allow(
    clippy::too_many_arguments,
    reason = "the semantic/native coordinate boundary is clearer with explicit measured inputs"
)]
fn normalize_semantic_changes(
    states: &[StateRecord],
    visualization: Option<&ChangeVisualization>,
    clips: &[NormalizedClip],
    capture_start_ns: u64,
    fps: u16,
    source_width: u16,
    source_height: u16,
    geometry: TerminalGeometryReport,
) -> Result<Vec<NormalizedChangeFrame>> {
    if !visualization.is_some_and(|change| return change.source == ChangeSource::SemanticCells) {
        return Ok(Vec::new());
    }
    let x_origin = source_width.saturating_sub(geometry.text_width) / 2;
    let y_origin = source_height.saturating_sub(geometry.text_height) / 2;
    let mut output_frames_by_source = BTreeMap::<u64, Vec<u64>>::new();
    for clip in clips {
        for output_frame in clip.output_start_frame..clip.output_end_frame {
            let (source_frame, _) = mapped_source_frame(clip, output_frame);
            output_frames_by_source
                .entry(source_frame)
                .or_default()
                .push(output_frame);
        }
    }
    let mut changes = Vec::new();
    for state in states.iter().filter(|state| return state.presented) {
        let source_frame = frame_floor(state.monotonic_ns.saturating_sub(capture_start_ns), fps);
        let output_frames = output_frames_by_source
            .get(&source_frame)
            .cloned()
            .unwrap_or_default();
        if output_frames.is_empty() {
            continue;
        }
        let changed_regions = state
            .delta
            .rows
            .iter()
            .flat_map(|row| {
                return row.cells.iter().map(|cell| {
                    return ChangeRegion {
                        x: x_origin.saturating_add(cell.column.saturating_mul(geometry.cell_width)),
                        y: y_origin.saturating_add(row.row.saturating_mul(geometry.cell_height)),
                        width: geometry.cell_width,
                        height: geometry.cell_height,
                    };
                });
            })
            .collect::<Vec<_>>();
        if changed_regions.is_empty() && state.ghostty_dirty_rows.is_empty() {
            continue;
        }
        changes.push(NormalizedChangeFrame {
            state_id: state.state_id,
            state_hash: state.state_hash.clone(),
            source_frame,
            output_frames,
            changed_regions,
            dirty_rows: state.ghostty_dirty_rows.clone(),
            evidence_level: "semantic_scheduled_for_frame".to_owned(),
        });
    }
    return Ok(changes);
}

#[allow(
    clippy::too_many_arguments,
    reason = "cursor masks explicitly bind semantic state, video mapping, and measured geometry"
)]
fn normalize_cursor_masks(
    states: &[StateRecord],
    transforms: &[PixelTransform],
    clips: &[NormalizedClip],
    capture_start_ns: u64,
    fps: u16,
    source_width: u16,
    source_height: u16,
    geometry: TerminalGeometryReport,
) -> Result<Vec<CursorMaskFrame>> {
    if !transforms
        .iter()
        .any(|transform| return matches!(transform, PixelTransform::CursorMask))
    {
        return Ok(Vec::new());
    }
    let mut cursor = None::<CursorState>;
    let mut presented_by_frame = BTreeMap::new();
    for state in states {
        if let Some(next) = state.delta.cursor.clone() {
            cursor = Some(next);
        }
        if state.presented {
            let source_frame =
                frame_floor(state.monotonic_ns.saturating_sub(capture_start_ns), fps);
            if let Some(cursor) = &cursor {
                presented_by_frame.insert(
                    source_frame,
                    (state.state_id, state.state_hash.clone(), cursor.clone()),
                );
            }
        }
    }
    let x_origin = source_width.saturating_sub(geometry.text_width) / 2;
    let y_origin = source_height.saturating_sub(geometry.text_height) / 2;
    let mut masks = Vec::new();
    for clip in clips {
        for output_frame in clip.output_start_frame..clip.output_end_frame {
            let (source_frame, _) = mapped_source_frame(clip, output_frame);
            let Some((_, (state_id, state_hash, cursor))) =
                presented_by_frame.range(..=source_frame).next_back()
            else {
                continue;
            };
            let (Some(column), Some(row)) = (cursor.column, cursor.row) else {
                continue;
            };
            if !cursor.visible || !cursor.in_viewport {
                continue;
            }
            let mut region = ChangeRegion {
                x: x_origin.saturating_add(column.saturating_mul(geometry.cell_width)),
                y: y_origin.saturating_add(row.saturating_mul(geometry.cell_height)),
                width: geometry.cell_width,
                height: geometry.cell_height,
            };
            match cursor.style {
                CursorStyle::Bar => region.width = (geometry.cell_width / 8).max(2),
                CursorStyle::Underline => {
                    region.height = (geometry.cell_height / 8).max(2);
                    region.y = region
                        .y
                        .saturating_add(geometry.cell_height.saturating_sub(region.height));
                }
                CursorStyle::Block | CursorStyle::HollowBlock => {}
            }
            masks.push(CursorMaskFrame {
                output_frame,
                source_frame,
                state_id: *state_id,
                state_hash: state_hash.clone(),
                region,
                evidence_level: "semantic_scheduled_for_frame".to_owned(),
            });
        }
    }
    if masks.len() > 1_000_000 {
        return Err(plan_error(
            "cursor masking exceeds one million output frames",
        ));
    }
    return Ok(masks);
}

pub(super) fn validate_transform_geometry(
    edit: &EditPlan,
    source_width: u16,
    source_height: u16,
    has_captions: bool,
) -> Result<(u16, u16)> {
    let side_diff = edit
        .change_visualization
        .as_ref()
        .is_some_and(|change| return change.mode == ChangeMode::SideDiff);
    let mut width = if side_diff {
        source_width
            .checked_mul(2)
            .ok_or_else(|| return plan_error("side-diff canvas width overflowed"))?
    } else {
        source_width
    };
    let mut height = source_height.saturating_add(
        if has_captions && edit.layout.caption == CaptionLayout::Footer {
            48
        } else {
            0
        },
    );
    let mut spatial_transform_seen = false;
    for transform in &edit.transforms {
        match transform {
            PixelTransform::CursorMask if spatial_transform_seen => {
                return Err(plan_error(
                    "cursor_mask must precede crop, zoom, and resize transforms",
                ));
            }
            PixelTransform::CursorMask => {}
            PixelTransform::Crop {
                x,
                y,
                width: crop_width,
                height: crop_height,
            } => {
                spatial_transform_seen = true;
                if x.saturating_add(*crop_width) > width || y.saturating_add(*crop_height) > height
                {
                    return Err(plan_error("crop rectangle exceeds the current canvas"));
                }
                width = *crop_width;
                height = *crop_height;
            }
            PixelTransform::Zoom {
                numerator,
                denominator,
            } => {
                spatial_transform_seen = true;
                width = scaled_even_dimension(width, *numerator, *denominator)?;
                height = scaled_even_dimension(height, *numerator, *denominator)?;
            }
            PixelTransform::Resize {
                width: resize_width,
                height: resize_height,
            } => {
                spatial_transform_seen = true;
                width = *resize_width;
                height = *resize_height;
            }
            PixelTransform::Callout {
                x,
                y,
                width: callout_width,
                height: callout_height,
                ..
            } => {
                if x.saturating_add(*callout_width) > width
                    || y.saturating_add(*callout_height) > height
                {
                    return Err(plan_error("callout rectangle exceeds the current canvas"));
                }
            }
        }
    }
    if width % 2 != 0 || height % 2 != 0 {
        return Err(plan_error(
            "final H.264 yuv420p presentation dimensions must be even",
        ));
    }
    return Ok((width, height));
}

fn scaled_even_dimension(value: u16, numerator: u16, denominator: u16) -> Result<u16> {
    let scaled = u32::from(value).saturating_mul(u32::from(numerator)) / u32::from(denominator);
    let even = scaled / 2 * 2;
    if even == 0 || even > 8192 {
        return Err(plan_error(
            "zoomed presentation dimension must be an even value from 2 through 8192",
        ));
    }
    return u16::try_from(even)
        .map_err(|_error| return plan_error("zoomed presentation dimension overflowed"));
}

fn resolve_clip(
    log: &EventLog,
    states: &[StateRecord],
    clip: &EditClip,
    capture_start_ns: u64,
    fps: u16,
) -> Result<SourceRange> {
    let from = resolve_edit_selector(log, states, &clip.from, capture_start_ns, fps)?;
    let to = resolve_edit_selector(log, states, &clip.to, capture_start_ns, fps)?;
    return resolve_source_selection(log, Some(from), Some(to), None, None)?
        .ok_or_else(|| return plan_error("edit clip must have explicit endpoints"));
}

pub(super) fn ranges_overlap(left: SourceRange, right: SourceRange) -> bool {
    return left.start_monotonic_ns < right.end_monotonic_ns
        && right.start_monotonic_ns < left.end_monotonic_ns;
}

fn resolve_edit_selector(
    log: &EventLog,
    states: &[StateRecord],
    selector: &EditSelector,
    capture_start_ns: u64,
    fps: u16,
) -> Result<u64> {
    validate_edit_selector(selector)?;
    if let Some(monotonic_ns) = selector.monotonic_ns {
        return Ok(monotonic_ns);
    }
    if let Some(event_seq) = selector.event_seq {
        return log
            .events
            .iter()
            .find(|record| return record.seq == event_seq)
            .map(|record| return record.monotonic_ns)
            .ok_or_else(|| return plan_error(format!("event sequence {event_seq} is absent")));
    }
    if let Some(state_id) = selector.state_id {
        return states
            .iter()
            .find(|state| return state.state_id == state_id)
            .map(|state| return state.monotonic_ns)
            .ok_or_else(|| return plan_error("no semantic state matches the selector"));
    }
    if let Some(native_frame) = selector.native_frame {
        if fps == 0 {
            return Err(plan_error("native frame selector requires nonzero FPS"));
        }
        return Ok(capture_start_ns
            .saturating_add(native_frame.saturating_mul(NANOS_PER_SECOND) / u64::from(fps)));
    }
    let marker = selector
        .marker
        .as_deref()
        .ok_or_else(|| return plan_error("selector has no coordinate"))?;
    let matching = log
        .events
        .iter()
        .filter(|record| {
            return matches!(&record.event, Event::Marker { label, .. } if label == marker);
        })
        .collect::<Vec<_>>();
    return match matching.as_slice() {
        [record] => Ok(record.monotonic_ns),
        [] => Err(plan_error(format!("marker `{marker}` is absent"))),
        _ => Err(plan_error(format!("marker `{marker}` is not unique"))),
    };
}

pub(super) fn frame_floor(monotonic_ns: u64, fps: u16) -> u64 {
    return monotonic_ns.saturating_mul(u64::from(fps)) / NANOS_PER_SECOND;
}

pub(super) fn frame_ceil(monotonic_ns: u64, fps: u16) -> u64 {
    return ceil_ratio(
        monotonic_ns.saturating_mul(u64::from(fps)),
        NANOS_PER_SECOND,
    );
}

pub(super) fn ceil_ratio(numerator: u64, denominator: u64) -> u64 {
    return numerator.saturating_add(denominator - 1) / denominator;
}

pub(super) fn hash_file(path: &Path) -> Result<String> {
    return Ok(hash_regular_file(path).map(|(_, hash)| return hash)?);
}

pub(super) fn missing_native_capture_error(session: &Path, manifest: &Path) -> Error {
    return plan_error(format!(
        "native video source requires a verified capture manifest at `{}`; create it with `termtrace artifact capture --session SESSION` for `{}` after checking the native-video tooling capability with `termtrace doctor --require native-video --explain`, or select `--renderer semantic`",
        manifest.display(),
        session.display(),
    ));
}
