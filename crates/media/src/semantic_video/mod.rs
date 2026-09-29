//! Deterministic semantic-state video scheduling and source materialization.

use crate::{capability::probe_semantic_video_encode, error::finish_with_cleanup, Error, Result};
use std::{
    collections::HashMap,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::AtomicBool,
};
use termtrace_engine::{replay::visit_session_states_while, Error as EngineError};
use termtrace_evidence::{
    artifact::{finish_writer, hash_regular_file, secure_file, secure_writer, write_secure},
    event_log::{EventLog, EventLogCursor},
    session::validate_completed_session,
};
use termtrace_model::execution::layout::SessionLayout;
use termtrace_model::{
    artifact::{
        save::SemanticRasterProfile,
        semantic_video::{
            SemanticFrameSpan, SemanticVideoCanvas, SemanticVideoSourceManifest,
            SemanticVideoSourcePlan, SEMANTIC_VIDEO_VERSION,
        },
    },
    execution::event::{Event, OperationStatus},
    protocol::capability::SEMANTIC_VIDEO_CAPABILITY,
    terminal::replay::StateRecord,
    terminal::state::SemanticState,
};
use termtrace_render::{
    render_video_rgba, semantic_font_set, Error as RenderError, SEMANTIC_CELL_HEIGHT,
    SEMANTIC_CELL_WIDTH, SEMANTIC_PNG_RENDERER,
};

const NANOS_PER_SECOND: u64 = 1_000_000_000;

/// Plan a bounded semantic video source from one completed session.
///
/// # Errors
///
/// Returns an error for an unsupported frame rate, missing terminal event, or
/// semantic canvas dimensions that cannot be represented by the video schema.
pub fn plan(
    session: impl AsRef<Path>,
    log: &EventLog,
    fps: u16,
    start_monotonic_ns: u64,
    end_monotonic_ns: u64,
    raster_profile: SemanticRasterProfile,
) -> Result<SemanticVideoSourcePlan> {
    let final_event = log.events.last().ok_or_else(|| {
        return video_error("semantic video requires at least one canonical event");
    })?;

    return finish_plan(
        session.as_ref(),
        fps,
        start_monotonic_ns,
        end_monotonic_ns,
        raster_profile,
        SourceGeometry {
            session_id: log.header.session_id,
            final_event_hash: final_event.hash.clone(),
            final_monotonic_ns: final_event.monotonic_ns,
            maximum: maximum_applied_geometry(log),
        },
    );
}

/// Plan a semantic video by streaming a completed source and its applied geometry.
///
/// # Errors
/// Rejects incomplete or changing sources, invalid timing, replay errors, and unsupported canvases.
pub fn plan_session(
    session: &Path,
    fps: u16,
    start_monotonic_ns: u64,
    end_monotonic_ns: u64,
    raster_profile: SemanticRasterProfile,
) -> Result<SemanticVideoSourcePlan> {
    let completion = validate_completed_session(session)?;
    let mut cursor = EventLogCursor::open(SessionLayout::new(session).event_log())?;
    let mut maximum = (cursor.header().cols, cursor.header().rows);

    let mut final_monotonic_ns = 0;
    while let Some(record) = cursor.next_record()? {
        final_monotonic_ns = record.monotonic_ns;
    }

    visit_session_states_while(session, &AtomicBool::new(false), |_, state| {
        maximum.0 = maximum.0.max(state.cols);
        maximum.1 = maximum.1.max(state.rows);

        return Ok(true);
    })?;

    let verified = validate_completed_session(session)?;
    if verified.session_id != completion.session_id
        || verified.final_event_hash != completion.final_event_hash
    {
        return Err(video_error("semantic video source changed during planning"));
    }
    return finish_plan(
        session,
        fps,
        start_monotonic_ns,
        end_monotonic_ns,
        raster_profile,
        SourceGeometry {
            session_id: completion.session_id,
            final_event_hash: completion.final_event_hash,
            final_monotonic_ns,
            maximum,
        },
    );
}

struct SourceGeometry {
    session_id: uuid::Uuid,
    final_event_hash: String,
    final_monotonic_ns: u64,
    maximum: (u16, u16),
}

fn finish_plan(
    session: &Path,
    fps: u16,
    start_monotonic_ns: u64,
    end_monotonic_ns: u64,
    raster_profile: SemanticRasterProfile,
    source: SourceGeometry,
) -> Result<SemanticVideoSourcePlan> {
    if !(1..=120).contains(&fps) {
        return Err(video_error("semantic video FPS must be between 1 and 120"));
    }

    if start_monotonic_ns >= end_monotonic_ns || end_monotonic_ns > source.final_monotonic_ns {
        return Err(video_error(
            "semantic video source range must be nonempty and inside the completed session",
        ));
    }

    let (cols, rows) = source.maximum;
    let width = cols.checked_mul(SEMANTIC_CELL_WIDTH).ok_or_else(|| {
        return video_error("semantic video width exceeds the supported pixel range");
    })?;

    let height = rows.checked_mul(SEMANTIC_CELL_HEIGHT).ok_or_else(|| {
        return video_error("semantic video height exceeds the supported pixel range");
    })?;

    let frame_count =
        frame_ceil(end_monotonic_ns.saturating_sub(start_monotonic_ns), fps).saturating_add(1);

    let font_set = semantic_font_set().map_err(render_error)?;

    return Ok(SemanticVideoSourcePlan {
        version: SEMANTIC_VIDEO_VERSION,
        session: session.to_path_buf(),
        session_id: source.session_id,
        source_event_hash: source.final_event_hash,
        start_monotonic_ns,
        end_monotonic_ns,
        fps,
        frame_count,
        canvas: SemanticVideoCanvas {
            cols,
            rows,
            cell_width: SEMANTIC_CELL_WIDTH,
            cell_height: SEMANTIC_CELL_HEIGHT,
            width,
            height,
            policy: "max_grid_top_left_v1".to_owned(),
        },
        renderer: SEMANTIC_PNG_RENDERER.to_owned(),
        raster_profile,
        fonts: font_set.resources().cloned().collect(),
        font_set_blake3: font_set.font_set_blake3().to_owned(),
        scheduling_policy: "latest_presented_at_frame_start_v1".to_owned(),
    });
}

/// Materialize a lossless semantic source video and its exact state-span map.
///
/// Frames are replayed and streamed directly to FFmpeg. Only the pending
/// semantic state and current RGBA frame are retained.
///
/// # Errors
///
/// Returns an error when canonical evidence changed, replay or rendering fails,
/// or FFmpeg/FFprobe cannot produce and validate the source.
pub fn materialize(
    source: &SemanticVideoSourcePlan,
    directory: impl AsRef<Path>,
) -> Result<(PathBuf, SemanticVideoSourceManifest)> {
    validate_source_plan(source)?;

    let directory = directory.as_ref();
    let completion = validate_completed_session(&source.session)?;

    if completion.session_id != source.session_id
        || completion.final_event_hash != source.source_event_hash
    {
        return Err(video_error(
            "semantic video canonical source changed after presentation planning",
        ));
    }

    require_semantic_video_capability()?;

    let output = directory.join("semantic-source.mkv");
    drop(secure_file(&output)?);

    let stderr_path = directory.join("semantic-source-ffmpeg.log");
    let stderr_file = secure_file(&stderr_path)?;
    let spans_path = directory.join("semantic-source-map.jsonl");
    let mut spans_writer = secure_writer(&spans_path)?;
    let ffmpeg_arguments = semantic_ffmpeg_arguments(source, &output);
    let mut child = Command::new("ffmpeg")
        .args(&ffmpeg_arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .map_err(|source_error| return filesystem_error("ffmpeg", source_error))?;
    let Some(mut stdin) = child.stdin.take() else {
        return finish_with_cleanup(
            Err(video_error("FFmpeg semantic source stdin was not created")),
            stop_ffmpeg(&mut child),
        );
    };
    let summary = match render_semantic_frames(source, &mut stdin, &mut |span| {
        return write_frame_span(&spans_path, &mut spans_writer, span);
    }) {
        Ok(summary) => summary,
        Err(error) => {
            drop(stdin);

            return finish_with_cleanup(Err(error), stop_ffmpeg(&mut child));
        }
    };
    drop(stdin);

    let status = child
        .wait()
        .map_err(|source_error| return filesystem_error("ffmpeg", source_error))?;

    if !status.success() {
        return Err(Error::ExternalCommand {
            operation: "FFmpeg semantic source export".to_owned(),
            reason: status.to_string(),
        });
    }

    let completion_after = validate_completed_session(&source.session)?;
    if completion_after.session_id != completion.session_id
        || completion_after.final_event_hash != completion.final_event_hash
    {
        return Err(video_error(
            "semantic video canonical source changed during rendering",
        ));
    }

    finish_writer(spans_writer, &spans_path)?;

    let (_, source_video_hash) = hash_regular_file(&output)?;
    let ffmpeg_version = command_version("ffmpeg")?;
    let video_probe = probe_video(&output)?;
    let manifest = SemanticVideoSourceManifest {
        version: SEMANTIC_VIDEO_VERSION,
        plan: source.clone(),
        truth_layer: "semantic_rendering".to_owned(),
        presented_state_count: summary.presented_state_count,
        rendered_state_count: summary.rendered_state_count,
        suppressed_state_count: summary.suppressed_state_count,
        pixel_stream_hash: summary.pixel_stream_hash,
        source_video_hash,
        ffmpeg_version,
        ffmpeg_arguments,
        video_probe,
    };

    let manifest_path = directory.join("semantic-source-manifest.json");
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(|source_error| {
        return Error::InvalidJson {
            path: manifest_path.clone(),
            json_path: "$".to_owned(),
            source: source_error,
        };
    })?;

    write_secure(&manifest_path, &bytes)?;

    return Ok((output, manifest));
}

fn render_semantic_frames(
    source: &SemanticVideoSourcePlan,
    output: &mut impl Write,
    span_sink: &mut dyn FnMut(&SemanticFrameSpan) -> Result<()>,
) -> Result<FrameSummary> {
    let expected_bytes = usize::from(source.canvas.width)
        .saturating_mul(usize::from(source.canvas.height))
        .saturating_mul(4);

    let mut scheduler = SemanticFrameScheduler::new(source, output, expected_bytes, span_sink);
    let mut pending: Option<(StateRecord, SemanticState)> = None;

    visit_session_states_while(&source.session, &AtomicBool::new(false), |record, state| {
        if !record.presented || record.monotonic_ns < source.start_monotonic_ns {
            if record.presented {
                pending = Some((record.clone(), state.clone()));
            }

            return Ok(true);
        }

        if record.monotonic_ns > source.end_monotonic_ns {
            return Ok(false);
        }

        if let Some((previous_record, previous_state)) = pending.take() {
            let boundary = source_frame_at_or_after(source, record.monotonic_ns);
            scheduler
                .emit(&previous_record, &previous_state, boundary)
                .map_err(|error| {
                    return EngineError::InvalidReplay {
                        reason: error.to_string(),
                        source: None,
                    };
                })?;
        }

        pending = Some((record.clone(), state.clone()));

        return Ok(true);
    })?;

    let Some((final_record, final_state)) = pending else {
        return Err(video_error(
            "semantic replay produced no presentation-eligible state",
        ));
    };
    scheduler.emit(&final_record, &final_state, source.frame_count)?;

    return scheduler.finish();
}

struct SemanticFrameScheduler<'scheduler, W> {
    source: &'scheduler SemanticVideoSourcePlan,
    output: &'scheduler mut W,
    expected_bytes: usize,
    next_frame: u64,
    presented_state_count: u64,
    rendered_state_count: u64,
    suppressed_state_count: u64,
    pixel_stream_hasher: blake3::Hasher,
    span_sink: &'scheduler mut dyn FnMut(&SemanticFrameSpan) -> Result<()>,
    last_rendered_state_hash: Option<String>,
    last_rendered_pixels: Option<Vec<u8>>,
}

impl<'scheduler, W: Write> SemanticFrameScheduler<'scheduler, W> {
    fn new(
        source: &'scheduler SemanticVideoSourcePlan,
        output: &'scheduler mut W,
        expected_bytes: usize,
        span_sink: &'scheduler mut dyn FnMut(&SemanticFrameSpan) -> Result<()>,
    ) -> Self {
        return Self {
            source,
            output,
            expected_bytes,
            next_frame: 0,
            presented_state_count: 0,
            rendered_state_count: 0,
            suppressed_state_count: 0,
            pixel_stream_hasher: blake3::Hasher::new(),
            span_sink,
            last_rendered_state_hash: None,
            last_rendered_pixels: None,
        };
    }

    fn emit(&mut self, record: &StateRecord, state: &SemanticState, end_frame: u64) -> Result<()> {
        self.presented_state_count = self.presented_state_count.saturating_add(1);

        let end_frame = end_frame.min(self.source.frame_count);

        if end_frame <= self.next_frame {
            self.suppressed_state_count = self.suppressed_state_count.saturating_add(1);

            return Ok(());
        }

        let pixels = if self
            .last_rendered_state_hash
            .as_ref()
            .is_some_and(|hash| return hash == &record.state_hash)
            && let Some(cached) = &self.last_rendered_pixels
        {
            cached.clone()
        } else {
            let rendered = render_video_rgba(
                state,
                self.source.canvas.cols,
                self.source.canvas.rows,
                self.source.raster_profile,
            )
            .map_err(render_error)?;

            self.last_rendered_state_hash = Some(record.state_hash.clone());
            self.last_rendered_pixels = Some(rendered.clone());
            rendered
        };

        if pixels.len() != self.expected_bytes {
            return Err(video_error(
                "semantic renderer returned an invalid RGBA frame",
            ));
        }

        let span = SemanticFrameSpan {
            start_frame: self.next_frame,
            end_frame,
            state_id: record.state_id,
            state_hash: record.state_hash.clone(),
            state_monotonic_ns: record.monotonic_ns,
        };
        for _ in span.start_frame..span.end_frame {
            self.output
                .write_all(&pixels)
                .map_err(|source_error| return filesystem_error("<ffmpeg-stdin>", source_error))?;

            self.pixel_stream_hasher.update(&pixels);
        }

        self.next_frame = end_frame;
        self.rendered_state_count = self.rendered_state_count.saturating_add(1);
        (self.span_sink)(&span)?;

        return Ok(());
    }

    fn finish(self) -> Result<FrameSummary> {
        if self.next_frame != self.source.frame_count {
            return Err(video_error(
                "semantic frame schedule did not cover the source",
            ));
        }
        return Ok(FrameSummary {
            presented_state_count: self.presented_state_count,
            rendered_state_count: self.rendered_state_count,
            suppressed_state_count: self.suppressed_state_count,
            pixel_stream_hash: self.pixel_stream_hasher.finalize().to_hex().to_string(),
        });
    }
}

struct FrameSummary {
    presented_state_count: u64,
    rendered_state_count: u64,
    suppressed_state_count: u64,
    pixel_stream_hash: String,
}

fn maximum_applied_geometry(log: &EventLog) -> (u16, u16) {
    let mut maximum = (log.header.cols, log.header.rows);
    let mut intents = HashMap::new();

    for record in &log.events {
        match &record.event {
            Event::ResizeIntent {
                operation_id,
                cols,
                rows,
                ..
            } => {
                intents.insert(*operation_id, (*cols, *rows));
            }
            Event::ResizeResult {
                operation_id,
                status: OperationStatus::Applied,
                ..
            } => {
                if let Some((cols, rows)) = intents.remove(operation_id) {
                    maximum.0 = maximum.0.max(cols);
                    maximum.1 = maximum.1.max(rows);
                }
            }
            Event::ResizeResult { operation_id, .. } => {
                intents.remove(operation_id);
            }
            _ => {}
        }
    }

    return maximum;
}

fn validate_source_plan(source: &SemanticVideoSourcePlan) -> Result<()> {
    let font_set = semantic_font_set().map_err(render_error)?;
    let expected_width = source.canvas.cols.checked_mul(SEMANTIC_CELL_WIDTH);
    let expected_height = source.canvas.rows.checked_mul(SEMANTIC_CELL_HEIGHT);
    let expected_frames = frame_ceil(
        source
            .end_monotonic_ns
            .saturating_sub(source.start_monotonic_ns),
        source.fps,
    )
    .saturating_add(1);

    if source.version != SEMANTIC_VIDEO_VERSION
        || !(1..=120).contains(&source.fps)
        || source.frame_count == 0
        || source.frame_count != expected_frames
        || source.end_monotonic_ns < source.start_monotonic_ns
        || source.canvas.cols == 0
        || source.canvas.rows == 0
        || source.canvas.width == 0
        || source.canvas.height == 0
        || source.canvas.cell_width != SEMANTIC_CELL_WIDTH
        || source.canvas.cell_height != SEMANTIC_CELL_HEIGHT
        || Some(source.canvas.width) != expected_width
        || Some(source.canvas.height) != expected_height
        || source.canvas.policy != "max_grid_top_left_v1"
        || source.renderer != SEMANTIC_PNG_RENDERER
        || !font_set.resources().eq(source.fonts.iter())
        || source.font_set_blake3 != font_set.font_set_blake3()
        || source.scheduling_policy != "latest_presented_at_frame_start_v1"
    {
        return Err(video_error("semantic video source plan is invalid"));
    }
    return Ok(());
}

fn source_frame_at_or_after(source: &SemanticVideoSourcePlan, monotonic_ns: u64) -> u64 {
    return frame_ceil(
        monotonic_ns.saturating_sub(source.start_monotonic_ns),
        source.fps,
    )
    .min(source.frame_count);
}

fn frame_ceil(monotonic_ns: u64, fps: u16) -> u64 {
    let scaled = u128::from(monotonic_ns).saturating_mul(u128::from(fps));
    let frames =
        scaled.saturating_add(u128::from(NANOS_PER_SECOND - 1)) / u128::from(NANOS_PER_SECOND);

    return u64::try_from(frames).unwrap_or(u64::MAX);
}

fn semantic_ffmpeg_arguments(source: &SemanticVideoSourcePlan, output: &Path) -> Vec<String> {
    return vec![
        "-hide_banner".to_owned(),
        "-nostdin".to_owned(),
        "-y".to_owned(),
        "-f".to_owned(),
        "rawvideo".to_owned(),
        "-pix_fmt".to_owned(),
        "rgba".to_owned(),
        "-video_size".to_owned(),
        format!("{}x{}", source.canvas.width, source.canvas.height),
        "-framerate".to_owned(),
        source.fps.to_string(),
        "-i".to_owned(),
        "pipe:0".to_owned(),
        "-an".to_owned(),
        "-frames:v".to_owned(),
        source.frame_count.to_string(),
        "-vf".to_owned(),
        "format=rgb24".to_owned(),
        "-threads".to_owned(),
        "1".to_owned(),
        "-c:v".to_owned(),
        "libx264rgb".to_owned(),
        "-crf".to_owned(),
        "0".to_owned(),
        "-preset".to_owned(),
        "medium".to_owned(),
        output.display().to_string(),
    ];
}

fn write_frame_span(path: &Path, writer: &mut impl Write, span: &SemanticFrameSpan) -> Result<()> {
    serde_json::to_writer(&mut *writer, span).map_err(|source_error| {
        return Error::InvalidJson {
            path: path.to_path_buf(),
            json_path: "$".to_owned(),
            source: source_error,
        };
    })?;

    writer.write_all(b"\n").map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;

    return Ok(());
}

fn require_semantic_video_capability() -> Result<()> {
    command_version("ffprobe").map_err(|error| {
        return Error::CapabilityBlocked {
            id: SEMANTIC_VIDEO_CAPABILITY.to_owned(),
            code: "semantic_video_dependency_missing".to_owned(),
            detail: format!("FFprobe is unavailable: {error}"),
            remediation: "install FFmpeg with FFprobe and the libx264rgb encoder".to_owned(),
        };
    })?;

    return probe_semantic_video_encode().map_err(|detail| {
        return Error::CapabilityBlocked {
            id: SEMANTIC_VIDEO_CAPABILITY.to_owned(),
            code: "semantic_video_dependency_missing".to_owned(),
            detail,
            remediation: "install FFmpeg with FFprobe and the libx264rgb encoder".to_owned(),
        };
    });
}

fn command_version(command: &str) -> Result<String> {
    let output = Command::new(command)
        .arg("-version")
        .output()
        .map_err(|source_error| return filesystem_error(command, source_error))?;

    if !output.status.success() {
        return Err(Error::ExternalCommand {
            operation: "media tool version query".to_owned(),
            reason: format!(
                "{command} -version failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    return Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned());
}

fn probe_video(path: &Path) -> Result<serde_json::Value> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ])
        .arg(path)
        .output()
        .map_err(|source_error| return filesystem_error("ffprobe", source_error))?;

    if !output.status.success() {
        return Err(Error::ExternalCommand {
            operation: "FFprobe semantic source validation".to_owned(),
            reason: format!(
                "FFprobe rejected the semantic source video: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    return serde_json::from_slice(&output.stdout).map_err(|source_error| {
        return Error::InvalidJson {
            path: PathBuf::from("<ffprobe-semantic-source>"),
            json_path: "$".to_owned(),
            source: source_error,
        };
    });
}

fn render_error(error: RenderError) -> Error {
    return video_error(format!("semantic rendering failed: {error}"));
}

fn stop_ffmpeg(child: &mut Child) -> Result<()> {
    let killed = child
        .kill()
        .map_err(|source| return filesystem_error("ffmpeg", source));
    let reaped = child
        .wait()
        .map(|_status| {})
        .map_err(|source| return filesystem_error("ffmpeg", source));

    return finish_with_cleanup(killed, reaped);
}

fn filesystem_error(path: impl Into<PathBuf>, source: io::Error) -> Error {
    return Error::Filesystem {
        path: path.into(),
        source,
    };
}

fn video_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
        source: None,
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use termtrace_model::{terminal::replay::StateDelta, terminal::state::DirtyState};
    use termtrace_terminal::GhosttyVt;

    #[test]
    fn semantic_frame_boundaries_never_anticipate_a_state() {
        assert_eq!(frame_ceil(0, 24), 0);
        assert_eq!(frame_ceil(1, 24), 1);
        assert_eq!(frame_ceil(41_666_666, 24), 1);
        assert_eq!(frame_ceil(41_666_667, 24), 2);
    }

    #[test]
    fn ordered_frame_scheduler_rasterizes_once_and_maps_the_complete_span() {
        let mut engine = GhosttyVt::new(2, 2, 0).expect("semantic engine");
        engine.write(b"A");
        let state = engine.snapshot().expect("semantic state");
        let font_set = semantic_font_set().expect("semantic font set");
        let source = SemanticVideoSourcePlan {
            version: SEMANTIC_VIDEO_VERSION,
            session: PathBuf::from("session"),
            session_id: uuid::Uuid::nil(),
            source_event_hash: "a".repeat(64),
            start_monotonic_ns: 0,
            end_monotonic_ns: 1,
            fps: 1,
            frame_count: 2,
            canvas: SemanticVideoCanvas {
                cols: 2,
                rows: 2,
                cell_width: SEMANTIC_CELL_WIDTH,
                cell_height: SEMANTIC_CELL_HEIGHT,
                width: 24,
                height: 48,
                policy: "max_grid_top_left_v1".to_owned(),
            },
            renderer: SEMANTIC_PNG_RENDERER.to_owned(),
            raster_profile: SemanticRasterProfile::Direct,
            fonts: font_set.resources().cloned().collect(),
            font_set_blake3: font_set.font_set_blake3().to_owned(),
            scheduling_policy: "latest_presented_at_frame_start_v1".to_owned(),
        };

        let record = StateRecord {
            state_id: 7,
            event_seq: Some(3),
            event_byte_offset: Some(0),
            monotonic_ns: 0,
            state_hash: "b".repeat(64),
            presented: true,
            ghostty_dirty: DirtyState::Full,
            ghostty_dirty_rows: vec![0, 1],
            delta: StateDelta {
                geometry: None,
                cursor: None,
                modes: None,
                colors: None,
                active_style: None,
                metadata: None,
                rows: Vec::new(),
            },
        };

        let frame_bytes = usize::from(source.canvas.width)
            .saturating_mul(usize::from(source.canvas.height))
            .saturating_mul(4);

        let mut bytes = Vec::new();
        let mut spans = Vec::new();
        let mut collect_span = |span: &SemanticFrameSpan| {
            spans.push(span.clone());

            return Ok(());
        };
        let mut scheduler =
            SemanticFrameScheduler::new(&source, &mut bytes, frame_bytes, &mut collect_span);
        scheduler.emit(&record, &state, 2).expect("schedule span");
        let summary = scheduler.finish().expect("finish schedule");
        let mut supersampled_source = source.clone();
        supersampled_source.raster_profile = SemanticRasterProfile::Supersampled2x;
        let mut supersampled_bytes = Vec::new();
        let mut discard_span = |_: &SemanticFrameSpan| return Ok(());
        let mut supersampled_scheduler = SemanticFrameScheduler::new(
            &supersampled_source,
            &mut supersampled_bytes,
            frame_bytes,
            &mut discard_span,
        );
        supersampled_scheduler
            .emit(&record, &state, 2)
            .expect("schedule supersampled span");
        let supersampled_summary = supersampled_scheduler
            .finish()
            .expect("finish supersampled schedule");

        assert_eq!(bytes.len(), frame_bytes * 2);
        assert_eq!(summary.presented_state_count, 1);
        assert_eq!(summary.rendered_state_count, 1);
        assert_eq!(summary.suppressed_state_count, 0);
        assert_eq!(
            summary.pixel_stream_hash,
            blake3::hash(&bytes).to_hex().to_string()
        );
        assert_eq!(
            supersampled_summary.pixel_stream_hash,
            blake3::hash(&supersampled_bytes).to_hex().to_string()
        );
        assert_ne!(
            summary.pixel_stream_hash,
            supersampled_summary.pixel_stream_hash
        );
        assert_eq!(spans[0].state_id, 7);
        assert_eq!((spans[0].start_frame, spans[0].end_frame), (0, 2));
    }

    #[test]
    fn semantic_video_frame_rendering_uses_full_truecolor_and_ansi256_resolution() {
        let mut engine = GhosttyVt::new(4, 1, 0).expect("semantic engine");
        // Cell 0: TrueColor background (123, 45, 67)
        // Cell 1: ANSI-256 color index 196 (255, 0, 0)
        engine.write(b"\x1b[48;2;123;45;67m \x1b[48;5;196m ");
        let state = engine.snapshot().expect("semantic state");
        let pixels = render_video_rgba(
            &state,
            state.cols,
            state.rows,
            SemanticRasterProfile::Direct,
        )
        .expect("render video rgba");

        let width = usize::from(state.cols) * usize::from(SEMANTIC_CELL_WIDTH);
        // Cell 0: (x in 0..12, y in 0..24), each pixel is RGBA(123, 45, 67, 255)
        for y in 0..24 {
            for x in 0..12 {
                let offset = (y * width + x) * 4;

                assert_eq!(pixels[offset], 123);
                assert_eq!(pixels[offset + 1], 45);
                assert_eq!(pixels[offset + 2], 67);
                assert_eq!(pixels[offset + 3], 255);
            }
        }

        // Cell 1: (x in 12..24, y in 0..24), each pixel is RGBA(255, 0, 0, 255)
        for y in 0..24 {
            for x in 12..24 {
                let offset = (y * width + x) * 4;

                assert_eq!(pixels[offset], 255);
                assert_eq!(pixels[offset + 1], 0);
                assert_eq!(pixels[offset + 2], 0);
                assert_eq!(pixels[offset + 3], 255);
            }
        }
    }
}
