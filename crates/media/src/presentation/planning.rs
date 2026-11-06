use super::{
    mapping::{mapped_source_frame, output_spans_for_source_frames},
    plan_error,
    validation::{edit_error, validate_edit_plan},
    NANOS_PER_SECOND,
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
    replay::{visit_session_states_while, StateSelection},
    timeline::resolve_source_range,
};
use termtrace_evidence::{
    artifact::{hash_regular_file, verify_artifact_members},
    event_log::{EventLog, EventLogCursor},
    session::{read_manifest, validate_completed_session},
};
use termtrace_model::execution::layout::SessionLayout;
use termtrace_model::{
    artifact::presentation::{
        AnnotationScope, CaptionLayout, ChangeMode, ChangeRegion, ChangeSource,
        ChangeVisualization, CursorMaskFrame, EditClip, EditFailure, EditFailureCode, EditPlan,
        EditSelector, MarkerRecord, NormalizedAnnotation, NormalizedChangeFrame, NormalizedClip,
        NormalizedSubtitle, PixelTransform, PresentationPlan, PresentationSource,
        PresentationSourceRequest, RationalSpeed, PRESENTATION_FORMAT_VERSION,
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
    let mut cursor = EventLogCursor::open(SessionLayout::new(session).event_log())?;

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
///
/// # Errors
///
/// Returns an error when `fps` is zero or the session's canonical markers cannot be read and
/// verified.
pub fn markers(session: impl AsRef<Path>, fps: u16) -> Result<Vec<MarkerRecord>> {
    if fps == 0 {
        return Err(plan_error("marker frame rate must be nonzero"));
    }
    return marker_records(session.as_ref(), fps, 0);
}

fn marker_records(session: &Path, fps: u16, capture_start_ns: i128) -> Result<Vec<MarkerRecord>> {
    validate_completed_session(session)?;

    let mut occurrence_counts = HashMap::<String, u64>::new();
    let mut records = Vec::new();
    let mut cursor = EventLogCursor::open(SessionLayout::new(session).event_log())?;

    while let Some(record) = cursor.next_record()? {
        if let Event::Marker { label, .. } = record.event {
            let occurrence = occurrence_counts.entry(label.clone()).or_default();
            *occurrence = occurrence.saturating_add(1);
            records.push(MarkerRecord {
                label,
                event_seq: record.seq,
                monotonic_ns: record.monotonic_ns,
                nominal_frame: frame_floor_ns(
                    source_elapsed_ns(record.monotonic_ns, capture_start_ns),
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
///
/// # Errors
///
/// Returns an error when the edit plan is invalid for the session or its verified native capture
/// cannot be resolved.
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
    return plan_with_capture_directory(session.as_ref(), edit, source_request, None);
}

/// Normalize the same edit pipeline against an explicitly selected verified capture directory.
///
/// Native frame ordinals are raw capture ordinals, but this presentation timeline
/// only admits their canonical timestamps at or after time zero. A pre-roll ordinal
/// is rejected, never clamped or rewritten. Native correlation remains nominal CFR /
/// acquisition-clock approximate, not frame-exact original/semantic correlation.
///
/// # Errors
/// Rejects invalid evidence, capture members, selectors (including pre-zero native
/// ordinals), geometry, or variable-rate pixels.
pub fn plan_with_capture_directory(
    session: &Path,
    edit: &EditPlan,
    source_request: PresentationSourceRequest,
    capture_directory: Option<&Path>,
) -> Result<PresentationPlan> {
    validate_edit_plan(edit)?;

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
        native_frame_count,
    } = resolve_source(
        session,
        edit,
        source_request,
        &session_manifest,
        capture_directory,
    )?;

    if fps == 0 {
        return Err(plan_error(
            "variable-rate original pixels are unsupported; a verified CFR capture is required",
        ));
    }

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
        native_frame_count,
    )?;

    let annotations = normalize_annotations(
        session,
        edit,
        &clips,
        output_frame_count,
        capture_start,
        fps,
    )?;

    let semantic_mapping = SemanticFrameMapping {
        session,
        clips: &clips,
        capture_start_ns: capture_start,
        fps,
        source_width,
        source_height,
        geometry: source_geometry,
    };
    let change_frames =
        normalize_semantic_changes(&semantic_mapping, edit.change_visualization.as_ref())?;

    let cursor_masks = normalize_cursor_masks(&semantic_mapping, &edit.transforms)?;

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
    capture_start_ns: i128,
    capture_range: Option<SourceRange>,
    semantic_final_frame: Option<(u64, u64)>,
    native_frame_count: Option<u64>,
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
    capture_directory: Option<&Path>,
) -> Result<ResolvedSource> {
    let cursor = EventLogCursor::open(SessionLayout::new(session).event_log())?;
    let header = cursor.header();

    return match source_request {
        PresentationSourceRequest::Native => {
            let visual = capture_directory.map_or_else(
                || {
                    return session.join(visual_directory_name_for(None, StateSelection::All));
                },
                Path::to_path_buf,
            );

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

            let completion = validate_completed_session(session)?;
            if manifest.source_event_hash.as_deref() != Some(completion.final_event_hash.as_str()) {
                return Err(plan_error(
                    "capture canonical boundary does not match completed source",
                ));
            }

            if manifest.fps == 0 {
                return Err(plan_error("variable-rate original pixels are unsupported; a verified CFR capture is required"));
            }

            if !matches!(
                manifest.capture_kind.as_str(),
                "video" | "original_execution"
            ) {
                return Err(plan_error("capture is not a supported video source"));
            }

            let source_video = visual.join("session.mkv");
            let source_video_hash = manifest
                .members
                .iter()
                .find(|member| return member.path == "session.mkv")
                .map(|member| return member.blake3.clone())
                .ok_or_else(|| return plan_error("native capture manifest has no session video"))?;

            let capture_start_ns =
                native_capture_clock_origin(&manifest, session_manifest, &visual)?;

            let native_frame_count = native_cfr_frame_count(&manifest)?;

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
                native_frame_count: Some(native_frame_count),
            })
        }
        PresentationSourceRequest::Semantic {
            fps,
            raster_profile,
        } => {
            if let Some(index) = edit.clips.iter().position(|clip| {
                return clip.from.native_frame.is_some() || clip.to.native_frame.is_some();
            }) {
                return Err(Error::Edit(
                    EditFailure::new(
                        EditFailureCode::InvalidSelector,
                        format!("clips[{index}]"),
                        "nativeFrame edit selectors are invalid for a semantic video source",
                    )
                    .clip(index),
                ));
            }

            let mut semantic_start = u64::MAX;

            let mut semantic_end = 0_u64;
            for (index, clip) in edit.clips.iter().enumerate() {
                let range = resolve_clip(session, clip, index, 0, fps)?;
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
                capture_start_ns: i128::from(semantic.start_monotonic_ns),
                capture_range: None,
                semantic_final_frame: Some(final_frame),
                native_frame_count: None,
            })
        }
    };
}

/// Native acquisition starts before Ghostty replay/application release. Preserve that
/// signed clock offset instead of treating pre-roll pixels as canonical time zero.
pub(super) fn native_capture_clock_origin(
    manifest: &VisualManifest,
    session: &SessionManifest,
    directory: &Path,
) -> Result<i128> {
    let (origin, range_start) = if manifest.capture_kind == "original_execution" {
        if manifest.replay_origin_unix_ns != 0
            || !manifest
                .members
                .iter()
                .any(|m| return m.path == "acquisition-map.jsonl")
            || !manifest
                .members
                .iter()
                .any(|m| return m.path == "execution-timing.json")
        {
            return Err(plan_error(
                "original execution capture lacks verified acquisition timing",
            ));
        }

        #[derive(serde::Deserialize)]
        struct Timing {
            execution_origin_unix_ns: u128,
            correlation: String,
        }

        let path = directory.join("execution-timing.json");
        let bytes = fs_err::read(&path).map_err(|source| {
            return Error::Filesystem {
                path: path.clone(),
                source,
            };
        })?;

        let timing: Timing = serde_json::from_slice(&bytes).map_err(|source| {
            return Error::InvalidJson {
                path,
                json_path: "$".into(),
                source,
            };
        })?;

        if timing.execution_origin_unix_ns != session.started_unix_ns
            || timing.correlation != "acquisition_clock_only_no_presentation_claim"
        {
            return Err(plan_error(
                "original execution clock is not bound to canonical execution",
            ));
        }

        (session.started_unix_ns, 0)
    } else {
        if manifest.replay_origin_unix_ns == 0 {
            return Err(plan_error("native replay has no playback clock origin"));
        }

        (
            manifest.replay_origin_unix_ns,
            manifest
                .source_range
                .map_or(0, |r| return r.start_monotonic_ns),
        )
    };

    if manifest.capture_started_unix_ns > origin
        || origin - manifest.capture_started_unix_ns > 86_400_000_000_000
    {
        return Err(plan_error(
            "capture clock must begin before execution/replay, with at most 24 hours pre-roll",
        ));
    }

    let acquired = i128::try_from(manifest.capture_started_unix_ns)
        .map_err(|_error| return plan_error("capture clock exceeds signed time range"))?;

    let origin = i128::try_from(origin)
        .map_err(|_error| return plan_error("execution clock exceeds signed time range"))?;

    return Ok(i128::from(range_start) + acquired - origin);
}

pub(super) fn native_cfr_frame_count(manifest: &VisualManifest) -> Result<u64> {
    let stream = manifest
        .video_probe
        .as_ref()
        .and_then(|p| return p.get("streams"))
        .and_then(|v| return v.get(0))
        .ok_or_else(|| return plan_error("native CFR video probe missing"))?;

    let fps = format!("{}/1", manifest.fps);
    if stream["r_frame_rate"].as_str() != Some(fps.as_str())
        || stream["avg_frame_rate"].as_str() != Some(fps.as_str())
        || stream["codec_name"] != "h264"
        || !matches!(stream["pix_fmt"].as_str(), Some("gbrp" | "rgb24" | "bgr0"))
        || stream["has_b_frames"].as_u64() != Some(0)
    {
        return Err(plan_error("native adapter requires verified CFR H.264 RGB capture; VFR/unknown adapters unsupported"));
    }
    return stream["nb_read_packets"]
        .as_str()
        .and_then(|v| return v.parse::<u64>().ok())
        .filter(|n| return *n > 0)
        .ok_or_else(|| return plan_error("native CFR probe has no verified packet/frame count"));
}

fn source_elapsed_ns(monotonic_ns: u64, capture_start_ns: i128) -> u128 {
    return u128::try_from((i128::from(monotonic_ns) - capture_start_ns).max(0)).unwrap_or(0);
}

pub(super) fn hash_edit_identity(edit: &EditPlan) -> Result<String> {
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
    capture_start_ns: i128,
    fps: u16,
    capture_range: Option<SourceRange>,
    semantic_final_frame: Option<(u64, u64)>,
    native_frame_count: Option<u64>,
) -> Result<NormalizedClips> {
    let mut output_start_frame = 0_u64;
    let mut clips = Vec::new();

    for (index, clip) in edit.clips.iter().enumerate() {
        let range = resolve_clip(session, clip, index, capture_start_ns, fps)?;
        let clip_failure = |code, reason: &str| {
            return Error::Edit(
                EditFailure::new(code, format!("clips[{index}]"), reason).clip(index),
            );
        };

        if clips.iter().any(|existing: &NormalizedClip| {
            return ranges_overlap(range, existing.source_range);
        }) {
            return Err(clip_failure(
                EditFailureCode::OverlappingClips,
                "edit clips must not overlap in source time",
            ));
        }

        if capture_range.is_some_and(|capture| {
            return range.start_monotonic_ns < capture.start_monotonic_ns
                || range.end_monotonic_ns > capture.end_monotonic_ns;
        }) {
            return Err(clip_failure(
                EditFailureCode::ClipOutOfRange,
                "edit clip lies outside the native capture range",
            ));
        }

        let source_start_frame = clip.from.native_frame.unwrap_or_else(|| {
            return frame_floor_ns(
                source_elapsed_ns(range.start_monotonic_ns, capture_start_ns),
                fps,
            );
        });

        let source_end_frame = clip.to.native_frame.unwrap_or_else(|| {
            return semantic_final_frame.map_or_else(
                || {
                    return frame_ceil_ns(
                        source_elapsed_ns(range.end_monotonic_ns, capture_start_ns),
                        fps,
                    );
                },
                |(semantic_end, final_frame)| {
                    return if range.end_monotonic_ns == semantic_end {
                        final_frame
                    } else {
                        frame_ceil_ns(
                            source_elapsed_ns(range.end_monotonic_ns, capture_start_ns),
                            fps,
                        )
                    };
                },
            );
        });

        if native_frame_count.is_some_and(|count| return source_end_frame > count) {
            return Err(clip_failure(
                EditFailureCode::ClipOutOfRange,
                "native clip exceeds verified CFR source frame count",
            ));
        }

        let source_frames = source_end_frame.saturating_sub(source_start_frame);
        let retimed_frame_count = u64::try_from(
            (u128::from(source_frames) * u128::from(clip.speed.denominator))
                .div_ceil(u128::from(clip.speed.numerator)),
        )
        .map_err(|_error| return plan_error("retimed frame count overflow"))?;

        let hold_frame_count =
            u64::try_from((u128::from(clip.hold_ms) * u128::from(fps)).div_ceil(1000))
                .map_err(|_error| return plan_error("hold frame count overflow"))?;

        let output_end_frame = output_start_frame
            .checked_add(retimed_frame_count)
            .and_then(|n| return n.checked_add(hold_frame_count))
            .ok_or_else(|| return plan_error("output frame count overflow"))?;
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
        return Err(edit_error(
            EditFailureCode::ClipOutOfRange,
            "clips",
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
    capture_start_ns: i128,
    fps: u16,
) -> Result<Vec<NormalizedAnnotation>> {
    let mut normalized = Vec::new();
    for (annotation_index, annotation) in edit.annotations.iter().enumerate() {
        let field = format!("annotations[{annotation_index}].scope");
        let invalid = |reason: String| {
            return edit_error(EditFailureCode::InvalidAnnotation, field.clone(), reason);
        };
        let spans = match &annotation.scope {
            AnnotationScope::WholeVideo => vec![(0, output_frame_count)],
            AnnotationScope::Clip { clip_index } => {
                let clip = clips.get(*clip_index).ok_or_else(|| {
                    return invalid("annotation clip index is out of bounds".to_owned());
                })?;

                vec![(clip.output_start_frame, clip.output_end_frame)]
            }
            AnnotationScope::OutputFrame { output_frame } => {
                if *output_frame >= output_frame_count {
                    return Err(invalid(format!(
                        "annotation output frame {output_frame} is out of bounds"
                    )));
                }

                vec![(*output_frame, output_frame + 1)]
            }
            AnnotationScope::Range { from, to } => {
                let start = resolve_edit_selector(
                    session,
                    from,
                    &format!("{field}.from"),
                    capture_start_ns,
                    fps,
                )?;

                let end = resolve_edit_selector(
                    session,
                    to,
                    &format!("{field}.to"),
                    capture_start_ns,
                    fps,
                )?;

                if start >= end {
                    return Err(invalid(
                        "annotation source range is empty or reversed".to_owned(),
                    ));
                }

                let start_frame = frame_floor_ns(source_elapsed_ns(start, capture_start_ns), fps);
                let end_frame = frame_ceil_ns(source_elapsed_ns(end, capture_start_ns), fps);

                output_spans_for_source_frames(clips, start_frame, end_frame)
                    .into_iter()
                    .map(|span| return (span.start, span.end))
                    .collect()
            }
        };
        if spans.is_empty() {
            return Err(invalid(format!(
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

struct SemanticFrameMapping<'source> {
    session: &'source Path,
    clips: &'source [NormalizedClip],
    capture_start_ns: i128,
    fps: u16,
    source_width: u16,
    source_height: u16,
    geometry: TerminalGeometryReport,
}

impl SemanticFrameMapping<'_> {
    fn source_frame(&self, monotonic_ns: u64) -> u64 {
        return frame_floor_ns(
            source_elapsed_ns(monotonic_ns, self.capture_start_ns),
            self.fps,
        );
    }

    fn text_origin(&self) -> (u16, u16) {
        return (
            self.source_width.saturating_sub(self.geometry.text_width) / 2,
            self.source_height.saturating_sub(self.geometry.text_height) / 2,
        );
    }
}

fn normalize_semantic_changes(
    mapping: &SemanticFrameMapping<'_>,
    visualization: Option<&ChangeVisualization>,
) -> Result<Vec<NormalizedChangeFrame>> {
    if !visualization.is_some_and(|change| return change.source == ChangeSource::SemanticCells) {
        return Ok(Vec::new());
    }

    let geometry = mapping.geometry;
    let (x_origin, y_origin) = mapping.text_origin();
    let mut output_frames_by_source = BTreeMap::<u64, Vec<u64>>::new();

    for clip in mapping.clips {
        for output_frame in clip.output_start_frame..clip.output_end_frame {
            let (source_frame, _) = mapped_source_frame(clip, output_frame);
            output_frames_by_source
                .entry(source_frame)
                .or_default()
                .push(output_frame);
        }
    }

    let mut changes = Vec::new();

    visit_session_states_while(mapping.session, &AtomicBool::new(false), |state, _| {
        if !state.presented {
            return Ok(true);
        }

        let source_frame = mapping.source_frame(state.monotonic_ns);

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

fn normalize_cursor_masks(
    mapping: &SemanticFrameMapping<'_>,
    transforms: &[PixelTransform],
) -> Result<Vec<CursorMaskFrame>> {
    if !transforms
        .iter()
        .any(|transform| return matches!(transform, PixelTransform::CursorMask))
    {
        return Ok(Vec::new());
    }

    let mut presented_by_frame = BTreeMap::new();
    for clip in mapping.clips {
        for output_frame in clip.output_start_frame..clip.output_end_frame {
            let (source_frame, _) = mapped_source_frame(clip, output_frame);
            presented_by_frame.insert(source_frame, None);
        }
    }

    let mut pending = presented_by_frame.iter_mut().peekable();
    let mut cursor = None::<CursorState>;
    let mut presented = None;

    visit_session_states_while(mapping.session, &AtomicBool::new(false), |state, _| {
        if let Some(next) = state.delta.cursor.clone() {
            cursor = Some(next);
        }

        if state.presented {
            let source_frame = mapping.source_frame(state.monotonic_ns);

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

    let geometry = mapping.geometry;
    let (x_origin, y_origin) = mapping.text_origin();
    let mut masks = Vec::new();

    for clip in mapping.clips {
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
    for (index, transform) in edit.transforms.iter().enumerate() {
        let invalid = |reason: &str| {
            return edit_error(
                EditFailureCode::InvalidTransform,
                format!("transforms[{index}]"),
                reason,
            );
        };
        match transform {
            PixelTransform::CursorMask if spatial_transform_seen => {
                return Err(invalid(
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
                    return Err(invalid("crop rectangle exceeds the current canvas"));
                }

                width = *crop_width;
                height = *crop_height;
            }
            PixelTransform::Zoom {
                numerator,
                denominator,
            } => {
                spatial_transform_seen = true;
                width = scaled_even_dimension(width, *numerator, *denominator)
                    .map_err(|reason| return invalid(reason))?;

                height = scaled_even_dimension(height, *numerator, *denominator)
                    .map_err(|reason| return invalid(reason))?;
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
                    return Err(invalid("callout rectangle exceeds the current canvas"));
                }
            }
        }
    }

    return Ok((
        even_canvas_dimension(width)?,
        even_canvas_dimension(height)?,
    ));
}

/// H.264 yuv420p needs even dimensions; an odd canvas gains one padded pixel.
fn even_canvas_dimension(value: u16) -> Result<u16> {
    return value
        .checked_next_multiple_of(2)
        .ok_or_else(|| return plan_error("presentation canvas dimension overflowed"));
}

fn scaled_even_dimension(
    value: u16,
    numerator: u16,
    denominator: u16,
) -> Result<u16, &'static str> {
    let scaled = u32::from(value).saturating_mul(u32::from(numerator)) / u32::from(denominator);
    let even = scaled / 2 * 2;

    if even == 0 || even > 8192 {
        return Err("zoomed presentation dimension must be an even value from 2 through 8192");
    }
    return u16::try_from(even).map_err(|_error| return "zoomed presentation dimension overflowed");
}

fn resolve_clip(
    session: &Path,
    clip: &EditClip,
    index: usize,
    capture_start_ns: i128,
    fps: u16,
) -> Result<SourceRange> {
    let with_clip = |error| {
        return match error {
            Error::Edit(failure) => Error::Edit(failure.clip(index)),
            other => other,
        };
    };
    let from = resolve_edit_selector(
        session,
        &clip.from,
        &format!("clips[{index}].from"),
        capture_start_ns,
        fps,
    )
    .map_err(with_clip)?;
    let to = resolve_edit_selector(
        session,
        &clip.to,
        &format!("clips[{index}].to"),
        capture_start_ns,
        fps,
    )
    .map_err(with_clip)?;

    if from >= to {
        return Err(Error::Edit(
            EditFailure::new(
                EditFailureCode::ClipOutOfRange,
                format!("clips[{index}]"),
                format!("edit clip must end after it starts, got {from}..{to} ns"),
            )
            .clip(index),
        ));
    }

    let final_ns = session_final_ns(session)?;
    let selector_out_of_range = |endpoint: &str, reason: String| {
        return Error::Edit(
            EditFailure::new(
                EditFailureCode::SelectorOutOfRange,
                format!("clips[{index}].{endpoint}"),
                reason,
            )
            .clip(index),
        );
    };

    if from > final_ns {
        return Err(selector_out_of_range(
            "from",
            format!("clip starts at {from} ns after the final event at {final_ns} ns"),
        ));
    }

    if to.saturating_sub(1) > final_ns {
        return Err(selector_out_of_range(
            "to",
            format!("clip ends at {to} ns, past the end of the session at {final_ns} ns"),
        ));
    }
    return resolve_source_range(Some(from), Some(to), final_ns)?
        .ok_or_else(|| return plan_error("edit clip must have explicit endpoints"));
}

fn session_final_ns(session: &Path) -> Result<u64> {
    let mut cursor = EventLogCursor::open(SessionLayout::new(session).event_log())?;

    let mut final_ns = 0;
    while let Some(record) = cursor.next_record()? {
        final_ns = record.monotonic_ns;
    }

    let _pending = cursor.finish()?;

    return Ok(final_ns);
}

pub(super) fn ranges_overlap(left: SourceRange, right: SourceRange) -> bool {
    return left.start_monotonic_ns < right.end_monotonic_ns
        && right.start_monotonic_ns < left.end_monotonic_ns;
}

/// Nanoseconds after the capture origin at which native frame `frame` starts at `fps`:
/// `ceil(frame * 1e9 / fps)`, so that the frame floor of the result is `frame` again.
pub(super) fn native_frame_ns(frame: u64, fps: u16) -> i128 {
    return (i128::from(frame) * i128::from(NANOS_PER_SECOND) + i128::from(fps) - 1)
        / i128::from(fps);
}

fn resolve_edit_selector(
    session: &Path,
    selector: &EditSelector,
    field: &str,
    capture_start_ns: i128,
    fps: u16,
) -> Result<u64> {
    let out_of_range =
        |reason: String| return edit_error(EditFailureCode::SelectorOutOfRange, field, reason);

    if let Some(monotonic_ns) = selector.monotonic_ns {
        return Ok(monotonic_ns);
    }

    if let Some(event_seq) = selector.event_seq {
        let mut cursor = EventLogCursor::open(SessionLayout::new(session).event_log())?;
        while let Some(record) = cursor.next_record()? {
            if record.seq == event_seq {
                return Ok(record.monotonic_ns);
            }
        }

        return Err(out_of_range(format!(
            "event sequence {event_seq} is absent"
        )));
    }

    if let Some(state_id) = selector.state_id {
        let mut selected = None;
        // Every output event ends in a boundary state, so the first boundary at or after a
        // position shares its source event and time.
        visit_session_states_while(session, &AtomicBool::new(false), |state, _| {
            if state.state_id >= state_id {
                selected = Some(state.monotonic_ns);
            }

            return Ok(state.state_id < state_id);
        })?;

        return selected.ok_or_else(|| {
            return out_of_range(format!("state {state_id} is absent"));
        });
    }

    if let Some(native_frame) = selector.native_frame {
        if fps == 0 {
            return Err(plan_error("native frame selector requires nonzero FPS"));
        }

        let canonical_ns = capture_start_ns + native_frame_ns(native_frame, fps);
        if canonical_ns < 0 {
            return Err(out_of_range(format!(
                "nativeFrame {native_frame} maps to canonical {canonical_ns} ns before time zero \
                 (capture origin {capture_start_ns} ns, {fps} FPS); pre-roll raw-frame selection \
                 is unsupported by presentation edits. Select a nativeFrame at or after canonical \
                 time zero, or use a nonnegative monotonicNs or marker selector; the ordinal is not clamped or rewritten"
            )));
        }
        return u64::try_from(canonical_ns).map_err(|_error| {
            return out_of_range(format!(
                "nativeFrame {native_frame} maps to canonical {canonical_ns} ns outside the u64 \
                 timestamp range; select a lower nativeFrame ordinal"
            ));
        });
    }

    let marker = selector
        .marker
        .as_deref()
        .ok_or_else(|| return plan_error("selector has no coordinate"))?;

    let mut found = None;
    let mut cursor = EventLogCursor::open(SessionLayout::new(session).event_log())?;

    while let Some(record) = cursor.next_record()? {
        if matches!(&record.event, Event::Marker { label, .. } if label == marker)
            && found.replace(record.monotonic_ns).is_some()
        {
            return Err(Error::Edit(
                EditFailure::new(
                    EditFailureCode::MarkerNotUnique,
                    field,
                    format!("marker `{marker}` is not unique"),
                )
                .marker(marker),
            ));
        }
    }

    return found.ok_or_else(|| {
        return Error::Edit(
            EditFailure::new(
                EditFailureCode::MarkerNotFound,
                field,
                format!("marker `{marker}` is absent"),
            )
            .marker(marker),
        );
    });
}

#[cfg(test)]
pub(super) fn frame_floor(monotonic_ns: u64, fps: u16) -> u64 {
    return frame_floor_ns(u128::from(monotonic_ns), fps);
}

fn frame_floor_ns(monotonic_ns: u128, fps: u16) -> u64 {
    // Supported clock offsets are bounded to 24h; u64 canonical ns * u16 FPS fits u128.
    return u64::try_from(monotonic_ns * u128::from(fps) / u128::from(NANOS_PER_SECOND))
        .unwrap_or(u64::MAX);
}

#[cfg(test)]
pub(super) fn frame_ceil(monotonic_ns: u64, fps: u16) -> u64 {
    return frame_ceil_ns(u128::from(monotonic_ns), fps);
}

fn frame_ceil_ns(monotonic_ns: u128, fps: u16) -> u64 {
    return u64::try_from((monotonic_ns * u128::from(fps)).div_ceil(u128::from(NANOS_PER_SECOND)))
        .unwrap_or(u64::MAX);
}

pub(super) fn hash_file(path: &Path) -> Result<String> {
    return Ok(hash_regular_file(path).map(|(_, hash)| return hash)?);
}

pub(super) fn missing_native_capture_error(session: &Path, manifest: &Path) -> Error {
    return plan_error(format!(
        "native video source requires a verified capture manifest at `{}`; create it with `termtrace artifact capture SESSION` for `{}` after checking the native-video tooling capability with `termtrace doctor --require native-video`, or select `--renderer semantic`",
        manifest.display(),
        session.display(),
    ));
}

#[cfg(test)]
mod capture_clock_tests {
    use super::super::EDIT_PLAN_VERSION;
    use super::*;

    #[test]
    fn cfr_native_clock_accounts_for_pre_roll_without_float_rounding() {
        // First acquisition was 950ms before application/replay release.
        let origin = -950_000_000i128;

        assert_eq!(frame_floor_ns(source_elapsed_ns(0, origin), 30), 28);
        assert_eq!(
            frame_ceil_ns(source_elapsed_ns(200_000_000, origin), 30),
            35
        );

        // Raw pre-roll frame selectors cannot be disguised as canonical time zero.
        let selector = EditSelector {
            native_frame: Some(0),
            ..Default::default()
        };

        assert!(
            resolve_edit_selector(Path::new("unused"), &selector, "clips[0].from", origin, 30)
                .is_err()
        );

        let selector = EditSelector {
            native_frame: Some(30),
            ..Default::default()
        };

        assert_eq!(
            resolve_edit_selector(Path::new("unused"), &selector, "clips[0].from", origin, 30)
                .unwrap(),
            50_000_000
        );

        // Large event timestamps do not saturate during ns*FPS arithmetic.
        let t = u64::MAX - 1_000_000;

        assert_eq!(
            u128::from(frame_floor_ns(source_elapsed_ns(t, origin), 60)),
            (u128::from(t) + 950_000_000) * 60 / 1_000_000_000
        );
    }

    #[test]
    fn pre_zero_native_ordinals_fail_actionably_without_rewriting_authored_edit() {
        let edit = EditPlan {
            version: EDIT_PLAN_VERSION,
            clips: vec![EditClip {
                from: EditSelector {
                    native_frame: Some(0),
                    ..Default::default()
                },
                to: EditSelector {
                    native_frame: Some(30),
                    ..Default::default()
                },
                speed: RationalSpeed::default(),
                hold_ms: 0,
                caption: None,
            }],
            ..Default::default()
        };
        let authored_bytes = serde_json::to_vec(&edit).unwrap();
        let authored_hash = hash_edit_identity(&edit).unwrap();
        let from = &edit.clips.first().unwrap().from;
        let error =
            resolve_edit_selector(Path::new("unused"), from, "clips[0].from", -950_000_000, 30)
                .expect_err("capture pre-roll must not be fabricated into canonical time zero");

        let message = error.to_string();

        for fact in [
            "nativeFrame 0",
            "canonical -950000000 ns",
            "before time zero",
            "pre-roll raw-frame selection",
            "monotonicNs",
            "marker",
            "not clamped or rewritten",
        ] {
            assert!(
                message.contains(fact),
                "missing actionable fact {fact}: {message}"
            );
        }

        assert_eq!(serde_json::to_vec(&edit).unwrap(), authored_bytes);
        assert_eq!(hash_edit_identity(&edit).unwrap(), authored_hash);

        let before_zero = EditSelector {
            native_frame: Some(28),
            ..Default::default()
        };

        assert!(resolve_edit_selector(
            Path::new("unused"),
            &before_zero,
            "clips[0].from",
            -950_000_000,
            30
        )
        .is_err());

        let after_zero = EditSelector {
            native_frame: Some(29),
            ..Default::default()
        };

        assert_eq!(
            resolve_edit_selector(
                Path::new("unused"),
                &after_zero,
                "clips[0].from",
                -950_000_000,
                30
            )
            .unwrap(),
            16_666_667
        );
    }

    #[test]
    fn existing_native_ordinal_ceil_rounding_admits_the_exact_zero_boundary() {
        let zero = EditSelector {
            native_frame: Some(0),
            ..Default::default()
        };

        assert_eq!(
            resolve_edit_selector(Path::new("unused"), &zero, "clips[0].from", 0, 60).unwrap(),
            0
        );

        let selector = EditSelector {
            native_frame: Some(1),
            ..Default::default()
        };

        assert_eq!(
            resolve_edit_selector(Path::new("unused"), &selector, "clips[0].from", 0, 60).unwrap(),
            16_666_667
        );
        assert_eq!(
            resolve_edit_selector(
                Path::new("unused"),
                &selector,
                "clips[0].from",
                -16_666_667,
                60
            )
            .unwrap(),
            0
        );
        assert_eq!(selector.native_frame, Some(1));
    }
}
