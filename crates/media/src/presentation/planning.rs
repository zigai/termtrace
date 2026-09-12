use super::{
    mapping::mapped_source_frame, plan_error, validation::validate_edit_plan, NANOS_PER_SECOND,
};
use crate::{
    native_capture::validate_visual_manifest_binding,
    semantic_video::{
        materialize as materialize_semantic_video, plan_session as plan_semantic_video,
    },
    Error, Result,
};
use std::{
    collections::{BTreeMap, HashMap},
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
use termtrace_engine::{
    replay::{
        inspect_completed_session, visit_session_states_while, StateSelection, StateSelector,
    },
    timeline::resolve_session_source_selection,
};
use termtrace_evidence::{
    artifact::{hash_regular_file, verify_artifact_members},
    event_log::{EventLog, EventLogCursor},
    session::{read_manifest, validate_completed_session},
};
use termtrace_model::{
    artifact::presentation::{
        AnnotationScope, CaptionLayout, ChangeMode, ChangeRegion, ChangeSource,
        ChangeVisualization, CursorMaskFrame, EditClip, EditPlan, EditSelector, MarkerRecord,
        NormalizedAnnotation, NormalizedChangeFrame, NormalizedClip, NormalizedSubtitle,
        PixelTransform, PresentationPlan, PresentationSource, PresentationSourceRequest,
        RationalSpeed, PRESENTATION_FORMAT_VERSION,
    },
    artifact::visual::{visual_directory_name_for, VisualManifest},
    execution::{event::Event, session::SessionManifest},
    terminal::replay::SourceRange,
    terminal::state::{CursorState, CursorStyle},
    value::geometry::TerminalGeometryReport,
};

/// Idle gaps omitted from video clips, with activity context retained on either side.
#[derive(Clone, Copy, Debug)]
pub struct VideoIdlePolicy {
    /// Minimum gap between output events that starts a new clip.
    pub threshold_ns: u64,
    /// Source time retained before and after output activity.
    pub context_ns: u64,
}

/// Select output-active ranges, or retain the entire selection when no policy is supplied.
pub fn video_clip_ranges(
    log: &EventLog,
    selected: SourceRange,
    idle: Option<VideoIdlePolicy>,
) -> Vec<SourceRange> {
    let mut ranges = IdleRanges::new(selected, idle);
    for record in &log.events {
        if matches!(record.event, Event::Output { .. }) {
            ranges.observe(record.monotonic_ns);
        }
    }

    return ranges.finish();
}

/// Select output-active video ranges with bounded canonical reads.
///
/// # Errors
/// Rejects invalid canonical evidence and source read failures.
pub fn video_clip_ranges_session(
    session: &Path,
    selected: SourceRange,
    idle: Option<VideoIdlePolicy>,
) -> Result<Vec<SourceRange>> {
    let mut ranges = IdleRanges::new(selected, idle);
    let mut cursor = EventLogCursor::open(session.join("events.jsonl"))?;

    while let Some(record) = cursor.next_record()? {
        if matches!(record.event, Event::Output { .. }) {
            ranges.observe(record.monotonic_ns);
        }
    }

    return Ok(ranges.finish());
}

struct IdleRanges {
    selected: SourceRange,
    idle: Option<VideoIdlePolicy>,
    ranges: Vec<SourceRange>,
    pending: Option<(u64, u64)>,
}

impl IdleRanges {
    fn new(selected: SourceRange, idle: Option<VideoIdlePolicy>) -> Self {
        return Self {
            selected,
            idle,
            ranges: Vec::new(),
            pending: None,
        };
    }

    fn observe(&mut self, timestamp: u64) {
        let Some(idle) = self.idle else {
            return;
        };

        if !self.selected.contains(timestamp) {
            return;
        }

        let start = timestamp
            .saturating_sub(idle.context_ns)
            .max(self.selected.start_monotonic_ns);

        if let Some((current_start, previous)) = self.pending {
            if timestamp.saturating_sub(previous) > idle.threshold_ns {
                self.ranges.push(SourceRange {
                    start_monotonic_ns: current_start,
                    end_monotonic_ns: previous
                        .saturating_add(idle.context_ns)
                        .min(self.selected.end_monotonic_ns),
                });

                self.pending = Some((start, timestamp));
            } else {
                self.pending = Some((current_start, timestamp));
            }
        } else {
            self.pending = Some((start, timestamp));
        }
    }

    fn finish(mut self) -> Vec<SourceRange> {
        if let (Some((start, previous)), Some(idle)) = (self.pending, self.idle) {
            self.ranges.push(SourceRange {
                start_monotonic_ns: start,
                end_monotonic_ns: previous
                    .saturating_add(idle.context_ns)
                    .min(self.selected.end_monotonic_ns),
            });
        } else {
            self.ranges.push(self.selected);
        }

        return self.ranges;
    }
}

/// Build video edits, applying start hold to the first source frame and end hold to the last clip.
///
/// # Errors
///
/// Returns an error if the frame rate is zero or no source ranges were selected.
pub fn video_clips(
    ranges: &[SourceRange],
    fps: u16,
    hold_start_ms: u64,
    hold_end_ms: u64,
) -> Result<Vec<EditClip>> {
    if fps == 0 {
        return Err(plan_error("video frame rate must be nonzero"));
    }

    let Some((&first, rest)) = ranges.split_first() else {
        return Err(plan_error("video range contains no source activity"));
    };

    let mut clips = Vec::with_capacity(ranges.len() + 1);

    if hold_start_ms > 0 {
        let frame_ns = NANOS_PER_SECOND.div_ceil(u64::from(fps));
        let split = first
            .start_monotonic_ns
            .saturating_add(frame_ns)
            .min(first.end_monotonic_ns);
        clips.push(edit_clip(
            SourceRange {
                start_monotonic_ns: first.start_monotonic_ns,
                end_monotonic_ns: split,
            },
            hold_start_ms,
        ));

        if split < first.end_monotonic_ns {
            clips.push(edit_clip(
                SourceRange {
                    start_monotonic_ns: split,
                    end_monotonic_ns: first.end_monotonic_ns,
                },
                0,
            ));
        }
    } else {
        clips.push(edit_clip(first, 0));
    }

    for &range in rest {
        clips.push(edit_clip(range, 0));
    }

    if let Some(last) = clips.last_mut() {
        last.hold_ms = last.hold_ms.saturating_add(hold_end_ms);
    }

    return Ok(clips);
}

fn edit_clip(range: SourceRange, hold_ms: u64) -> EditClip {
    return EditClip {
        from: EditSelector {
            monotonic_ns: Some(range.start_monotonic_ns),
            ..EditSelector::default()
        },
        to: EditSelector {
            monotonic_ns: Some(range.end_monotonic_ns),
            ..EditSelector::default()
        },
        speed: RationalSpeed::default(),
        hold_ms,
        caption: None,
    };
}

/// List every canonical marker without claiming native presentation.
pub fn markers(session: impl AsRef<Path>, fps: u16) -> Result<Vec<MarkerRecord>> {
    if fps == 0 {
        return Err(plan_error("marker frame rate must be nonzero"));
    }
    return marker_records(session.as_ref(), fps, 0);
}

fn marker_records(session: &Path, fps: u16, capture_start_ns: u64) -> Result<Vec<MarkerRecord>> {
    validate_completed_session(session)?;

    let mut occurrence_counts = HashMap::<String, u64>::new();
    let mut records = Vec::new();
    let mut cursor = EventLogCursor::open(session.join("events.jsonl"))?;

    while let Some(record) = cursor.next_record()? {
        if let Event::Marker { label, .. } = record.event {
            let occurrence = occurrence_counts.entry(label.clone()).or_default();
            *occurrence = occurrence.saturating_add(1);
            records.push(MarkerRecord {
                label,
                event_seq: record.seq,
                monotonic_ns: record.monotonic_ns,
                nominal_frame: frame_floor(
                    record.monotonic_ns.saturating_sub(capture_start_ns),
                    fps,
                ),
                occurrence_count: 0,
                occurrence_index: *occurrence,
                semantic_state_id: None,
                semantic_state_hash: None,
                presentation_eligible: false,
                native_evidence_level: "scheduled_for_frame".to_owned(),
            });
        }
    }

    for record in &mut records {
        record.occurrence_count = occurrence_counts.get(&record.label).copied().unwrap_or(0);
    }

    let mut pending = records.iter_mut().peekable();
    let mut presented = None::<(u64, String)>;

    visit_session_states_while(session, &AtomicBool::new(false), |state, _| {
        while pending
            .peek()
            .is_some_and(|marker| return marker.monotonic_ns < state.monotonic_ns)
        {
            if let Some(marker) = pending.next() {
                bind_marker_state(marker, presented.as_ref());
            }
        }

        if state.presented {
            presented = Some((state.state_id, state.state_hash.clone()));
        }

        return Ok(true);
    })?;

    for marker in pending {
        bind_marker_state(marker, presented.as_ref());
    }

    return Ok(records);
}

fn bind_marker_state(marker: &mut MarkerRecord, state: Option<&(u64, String)>) {
    marker.semantic_state_id = state.map(|(id, _)| return *id);
    marker.semantic_state_hash = state.map(|(_, hash)| return hash.clone());
    marker.presentation_eligible = state.is_some();
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
    let completion = validate_completed_session(session)?;
    let session_manifest = read_manifest(session)?;
    let ResolvedSource {
        source,
        fps,
        source_width,
        source_height,
        source_geometry,
        capture_start_ns: capture_start,
        capture_range,
        semantic_final_frame,
    } = resolve_source(session, edit, source_request, &session_manifest)?;

    let edit_plan_hash = hash_edit_identity(edit)?;
    let NormalizedClips {
        clips,
        output_frame_count,
    } = normalize_clips(
        session,
        edit,
        capture_start,
        fps,
        capture_range,
        semantic_final_frame,
    )?;

    let annotations = normalize_annotations(
        session,
        edit,
        &clips,
        output_frame_count,
        capture_start,
        fps,
    )?;

    let change_frames = normalize_semantic_changes(
        session,
        edit.change_visualization.as_ref(),
        &clips,
        capture_start,
        fps,
        source_width,
        source_height,
        source_geometry,
    )?;

    let cursor_masks = normalize_cursor_masks(
        session,
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

    let verified = validate_completed_session(session)?;
    if verified.session_id != completion.session_id
        || verified.final_event_hash != completion.final_event_hash
    {
        return Err(plan_error(
            "canonical source changed during presentation planning",
        ));
    }
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
        markers: marker_records(session, fps, capture_start)?,
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
    session_manifest: &SessionManifest,
) -> Result<ResolvedSource> {
    let cursor = EventLogCursor::open(session.join("events.jsonl"))?;
    let header = cursor.header();

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
                &header.colors.source,
            )?;

            verify_artifact_members(&visual, &manifest.members)?;

            if manifest.session_id != header.session_id {
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
                let range = resolve_clip(session, clip, 0, fps)?;
                semantic_start = semantic_start.min(range.start_monotonic_ns);
                semantic_end = semantic_end.max(range.end_monotonic_ns);
            }

            let semantic =
                plan_semantic_video(session, fps, semantic_start, semantic_end, raster_profile)?;

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
    session: &Path,
    edit: &EditPlan,
    capture_start_ns: u64,
    fps: u16,
    capture_range: Option<SourceRange>,
    semantic_final_frame: Option<(u64, u64)>,
) -> Result<NormalizedClips> {
    let mut output_start_frame = 0_u64;
    let mut clips = Vec::new();

    for clip in &edit.clips {
        let range = resolve_clip(session, clip, capture_start_ns, fps)?;

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
        let retimed_frame_count = source_frames
            .saturating_mul(u64::from(clip.speed.denominator))
            .div_ceil(u64::from(clip.speed.numerator));

        let hold_frame_count = clip.hold_ms.saturating_mul(u64::from(fps)).div_ceil(1000);
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
            let completion = validate_completed_session(&plan.session)?;
            if completion.session_id != plan.session_id
                || completion.final_event_hash != plan.source_event_hash
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
            let video = video.canonicalize().map_err(|source| {
                return Error::Filesystem {
                    path: video,
                    source,
                };
            })?;

            Ok(video)
        }
    };
}

fn normalize_annotations(
    session: &Path,
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
                let start = resolve_edit_selector(session, from, capture_start_ns, fps)?;
                let end = resolve_edit_selector(session, to, capture_start_ns, fps)?;

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
    session: &Path,
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

    visit_session_states_while(session, &AtomicBool::new(false), |state, _| {
        if !state.presented {
            return Ok(true);
        }

        let source_frame = frame_floor(state.monotonic_ns.saturating_sub(capture_start_ns), fps);
        let output_frames = output_frames_by_source
            .get(&source_frame)
            .cloned()
            .unwrap_or_default();

        if output_frames.is_empty() {
            return Ok(true);
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
            return Ok(true);
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

        return Ok(true);
    })?;

    return Ok(changes);
}

#[allow(
    clippy::too_many_arguments,
    reason = "cursor masks explicitly bind semantic state, video mapping, and measured geometry"
)]
fn normalize_cursor_masks(
    session: &Path,
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

    let mut presented_by_frame = BTreeMap::new();
    for clip in clips {
        for output_frame in clip.output_start_frame..clip.output_end_frame {
            let (source_frame, _) = mapped_source_frame(clip, output_frame);
            presented_by_frame.insert(source_frame, None);
        }
    }

    let mut pending = presented_by_frame.iter_mut().peekable();
    let mut cursor = None::<CursorState>;
    let mut presented = None;

    visit_session_states_while(session, &AtomicBool::new(false), |state, _| {
        if let Some(next) = state.delta.cursor.clone() {
            cursor = Some(next);
        }

        if state.presented {
            let source_frame =
                frame_floor(state.monotonic_ns.saturating_sub(capture_start_ns), fps);

            while pending
                .peek()
                .is_some_and(|(frame, _)| return **frame < source_frame)
            {
                if let Some((_, value)) = pending.next() {
                    value.clone_from(&presented);
                }
            }

            if let Some(cursor) = &cursor {
                presented = Some((state.state_id, state.state_hash.clone(), cursor.clone()));
            }
        }

        return Ok(true);
    })?;

    for (_, value) in pending {
        value.clone_from(&presented);
    }

    let x_origin = source_width.saturating_sub(geometry.text_width) / 2;
    let y_origin = source_height.saturating_sub(geometry.text_height) / 2;
    let mut masks = Vec::new();

    for clip in clips {
        for output_frame in clip.output_start_frame..clip.output_end_frame {
            let (source_frame, _) = mapped_source_frame(clip, output_frame);
            let Some(Some((state_id, state_hash, cursor))) = presented_by_frame.get(&source_frame)
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
        if has_captions && edit.layout.caption == CaptionLayout::Footer { 48 } else { 0 },
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
    session: &Path,
    clip: &EditClip,
    capture_start_ns: u64,
    fps: u16,
) -> Result<SourceRange> {
    let from = resolve_edit_selector(session, &clip.from, capture_start_ns, fps)?;
    let to = resolve_edit_selector(session, &clip.to, capture_start_ns, fps)?;

    return resolve_session_source_selection(session, Some(from), Some(to), None, None)?
        .ok_or_else(|| return plan_error("edit clip must have explicit endpoints"));
}

pub(super) fn ranges_overlap(left: SourceRange, right: SourceRange) -> bool {
    return left.start_monotonic_ns < right.end_monotonic_ns
        && right.start_monotonic_ns < left.end_monotonic_ns;
}

fn resolve_edit_selector(
    session: &Path,
    selector: &EditSelector,
    capture_start_ns: u64,
    fps: u16,
) -> Result<u64> {
    if let Some(monotonic_ns) = selector.monotonic_ns {
        return Ok(monotonic_ns);
    }

    if let Some(event_seq) = selector.event_seq {
        let mut cursor = EventLogCursor::open(session.join("events.jsonl"))?;
        while let Some(record) = cursor.next_record()? {
            if record.seq == event_seq {
                return Ok(record.monotonic_ns);
            }
        }

        return Err(plan_error(format!("event sequence {event_seq} is absent")));
    }

    if let Some(state_id) = selector.state_id {
        return Ok(
            inspect_completed_session(session, StateSelector::StateId(state_id))?
                .record
                .monotonic_ns,
        );
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

    let mut found = None;
    let mut cursor = EventLogCursor::open(session.join("events.jsonl"))?;

    while let Some(record) = cursor.next_record()? {
        if matches!(&record.event, Event::Marker { label, .. } if label == marker)
            && found.replace(record.monotonic_ns).is_some()
        {
            return Err(plan_error(format!("marker `{marker}` is not unique")));
        }
    }

    return found.ok_or_else(|| return plan_error(format!("marker `{marker}` is absent")));
}

pub(super) fn frame_floor(monotonic_ns: u64, fps: u16) -> u64 {
    return monotonic_ns.saturating_mul(u64::from(fps)) / NANOS_PER_SECOND;
}

pub(super) fn frame_ceil(monotonic_ns: u64, fps: u16) -> u64 {
    return monotonic_ns
        .saturating_mul(u64::from(fps))
        .div_ceil(NANOS_PER_SECOND);
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
