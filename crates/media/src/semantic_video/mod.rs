//! Deterministic semantic-state video scheduling and source materialization.

use std::{
    collections::HashMap,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use crate::{Error, Result};
use termtrace_engine::{Error as EngineError, replay::visit_states};
use termtrace_evidence::{
    artifact::{hash_regular_file, secure_file, write_secure},
    event_log::EventLog,
    session::validate_completed_log,
};
use termtrace_model::{
    artifact::semantic_video::{
        SEMANTIC_VIDEO_VERSION, SemanticFrameSpan, SemanticVideoCanvas,
        SemanticVideoSourceManifest, SemanticVideoSourcePlan,
    },
    execution::event::{Event, OperationStatus},
    protocol::capability::SEMANTIC_VIDEO_CAPABILITY,
    terminal::replay::StateRecord,
    terminal::state::SemanticState,
};
use termtrace_render::{
    Error as RenderError, SEMANTIC_CELL_HEIGHT, SEMANTIC_CELL_WIDTH, SEMANTIC_FONT_FAMILY,
    SEMANTIC_PNG_RENDERER, render_video_rgba, semantic_font_hash,
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
) -> Result<SemanticVideoSourcePlan> {
    if !(1..=120).contains(&fps) {
        return Err(video_error("semantic video FPS must be between 1 and 120"));
    }
    let final_event = log.events.last().ok_or_else(|| {
        return video_error("semantic video requires at least one canonical event");
    })?;
    if start_monotonic_ns >= end_monotonic_ns || end_monotonic_ns > final_event.monotonic_ns {
        return Err(video_error(
            "semantic video source range must be nonempty and inside the completed session",
        ));
    }
    let (cols, rows) = maximum_applied_geometry(log);
    let width = cols.checked_mul(SEMANTIC_CELL_WIDTH).ok_or_else(|| {
        return video_error("semantic video width exceeds the supported pixel range");
    })?;
    let height = rows.checked_mul(SEMANTIC_CELL_HEIGHT).ok_or_else(|| {
        return video_error("semantic video height exceeds the supported pixel range");
    })?;
    let frame_count =
        frame_ceil(end_monotonic_ns.saturating_sub(start_monotonic_ns), fps).saturating_add(1);
    return Ok(SemanticVideoSourcePlan {
        version: SEMANTIC_VIDEO_VERSION,
        session: session.as_ref().to_path_buf(),
        session_id: log.header.session_id,
        source_event_hash: final_event.hash.clone(),
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
        font_family: SEMANTIC_FONT_FAMILY.to_owned(),
        font_hash: semantic_font_hash(),
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
    let log = EventLog::read(source.session.join("events.jsonl"))?;
    validate_completed_log(&source.session, &log)?;
    let final_event = log
        .events
        .last()
        .ok_or_else(|| return video_error("semantic video source event log is empty"))?;
    if log.header.session_id != source.session_id || final_event.hash != source.source_event_hash {
        return Err(video_error(
            "semantic video canonical source changed after presentation planning",
        ));
    }
    require_semantic_video_capability()?;

    let output = directory.join("semantic-source.mkv");
    drop(secure_file(&output)?);
    let stderr_path = directory.join("semantic-source-ffmpeg.log");
    let stderr_file = secure_file(&stderr_path)?;
    let ffmpeg_arguments = semantic_ffmpeg_arguments(source, &output);
    let mut child = Command::new("ffmpeg")
        .args(&ffmpeg_arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .map_err(|source_error| return filesystem_error("ffmpeg", source_error))?;
    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(video_error("FFmpeg semantic source stdin was not created"));
    };
    let summary = match render_semantic_frames(source, &log, &mut stdin) {
        Ok(summary) => summary,
        Err(error) => {
            drop(stdin);
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
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
    if summary.written_frames != source.frame_count {
        return Err(video_error(format!(
            "semantic source wrote {} frames instead of {}",
            summary.written_frames, source.frame_count
        )));
    }

    write_frame_spans(&directory.join("semantic-source-map.jsonl"), &summary.spans)?;
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
    log: &EventLog,
    output: &mut impl Write,
) -> Result<FrameSummary> {
    let expected_bytes = usize::from(source.canvas.width)
        .saturating_mul(usize::from(source.canvas.height))
        .saturating_mul(4);
    let mut scheduler = SemanticFrameScheduler::new(source, output, expected_bytes);
    let mut pending: Option<(StateRecord, SemanticState)> = None;
    visit_states(log, |record, state| {
        if !record.presented || record.monotonic_ns < source.start_monotonic_ns {
            if record.presented {
                pending = Some((record.clone(), state.clone()));
            }
            return Ok(());
        }
        if record.monotonic_ns > source.end_monotonic_ns {
            return Ok(());
        }
        if let Some((previous_record, previous_state)) = pending.take() {
            let boundary = source_frame_at_or_after(source, record.monotonic_ns);
            scheduler
                .emit(&previous_record, &previous_state, boundary)
                .map_err(|error| {
                    return EngineError::InvalidReplay {
                        reason: error.to_string(),
                    };
                })?;
        }
        pending = Some((record.clone(), state.clone()));
        return Ok(());
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
    spans: Vec<SemanticFrameSpan>,
}

impl<'scheduler, W: Write> SemanticFrameScheduler<'scheduler, W> {
    fn new(
        source: &'scheduler SemanticVideoSourcePlan,
        output: &'scheduler mut W,
        expected_bytes: usize,
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
            spans: Vec::new(),
        };
    }

    fn emit(&mut self, record: &StateRecord, state: &SemanticState, end_frame: u64) -> Result<()> {
        self.presented_state_count = self.presented_state_count.saturating_add(1);
        let end_frame = end_frame.min(self.source.frame_count);
        if end_frame <= self.next_frame {
            self.suppressed_state_count = self.suppressed_state_count.saturating_add(1);
            return Ok(());
        }
        let pixels = render_video_rgba(state, self.source.canvas.cols, self.source.canvas.rows)
            .map_err(render_error)?;
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
        self.spans.push(span);
        return Ok(());
    }

    fn finish(self) -> Result<FrameSummary> {
        if self.next_frame != self.source.frame_count {
            return Err(video_error(
                "semantic frame schedule did not cover the source",
            ));
        }
        return Ok(FrameSummary {
            written_frames: self.next_frame,
            presented_state_count: self.presented_state_count,
            rendered_state_count: self.rendered_state_count,
            suppressed_state_count: self.suppressed_state_count,
            pixel_stream_hash: self.pixel_stream_hasher.finalize().to_hex().to_string(),
            spans: self.spans,
        });
    }
}

struct FrameSummary {
    written_frames: u64,
    presented_state_count: u64,
    rendered_state_count: u64,
    suppressed_state_count: u64,
    pixel_stream_hash: String,
    spans: Vec<SemanticFrameSpan>,
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
        || source.font_family != SEMANTIC_FONT_FAMILY
        || source.font_hash != semantic_font_hash()
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
        "-c:v".to_owned(),
        "libx264rgb".to_owned(),
        "-crf".to_owned(),
        "0".to_owned(),
        "-preset".to_owned(),
        "medium".to_owned(),
        output.display().to_string(),
    ];
}

fn write_frame_spans(path: &Path, spans: &[SemanticFrameSpan]) -> Result<()> {
    let mut bytes = Vec::new();
    for span in spans {
        serde_json::to_writer(&mut bytes, span).map_err(|source_error| {
            return Error::InvalidJson {
                path: path.to_path_buf(),
                json_path: "$".to_owned(),
                source: source_error,
            };
        })?;
        bytes.push(b'\n');
    }
    write_secure(path, &bytes)?;
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
    let output = Command::new("ffmpeg")
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "color=c=black:s=16x16:d=0.04",
            "-frames:v",
            "1",
            "-vf",
            "format=rgb24",
            "-c:v",
            "libx264rgb",
            "-crf",
            "0",
            "-preset",
            "medium",
            "-f",
            "null",
            "-",
        ])
        .output()
        .map_err(|source| {
            return Error::CapabilityBlocked {
                id: SEMANTIC_VIDEO_CAPABILITY.to_owned(),
                code: "semantic_video_dependency_missing".to_owned(),
                detail: format!("could not execute FFmpeg: {source}"),
                remediation: "install FFmpeg with FFprobe and the libx264rgb encoder".to_owned(),
            };
        })?;
    if output.status.success() {
        return Ok(());
    }
    let detail: String = String::from_utf8_lossy(&output.stderr)
        .chars()
        .take(1_024)
        .collect();
    return Err(Error::CapabilityBlocked {
        id: SEMANTIC_VIDEO_CAPABILITY.to_owned(),
        code: "semantic_video_dependency_missing".to_owned(),
        detail: format!("FFmpeg semantic-video encode failed: {detail}"),
        remediation: "install FFmpeg with FFprobe and the libx264rgb encoder".to_owned(),
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

fn filesystem_error(path: impl Into<PathBuf>, source: io::Error) -> Error {
    return Error::Filesystem {
        path: path.into(),
        source,
    };
}

fn video_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
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
        let state = engine.snapshot().expect("semantic state");
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
            font_family: SEMANTIC_FONT_FAMILY.to_owned(),
            font_hash: semantic_font_hash(),
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
        let mut scheduler = SemanticFrameScheduler::new(&source, &mut bytes, frame_bytes);
        scheduler.emit(&record, &state, 2).expect("schedule span");
        let summary = scheduler.finish().expect("finish schedule");

        assert_eq!(bytes.len(), frame_bytes * 2);
        assert_eq!(summary.written_frames, 2);
        assert_eq!(summary.presented_state_count, 1);
        assert_eq!(summary.rendered_state_count, 1);
        assert_eq!(summary.suppressed_state_count, 0);
        assert_eq!(
            summary.pixel_stream_hash,
            blake3::hash(&bytes).to_hex().to_string()
        );
        assert_eq!(summary.spans[0].state_id, 7);
        assert_eq!(
            (summary.spans[0].start_frame, summary.spans[0].end_frame),
            (0, 2)
        );
    }
}
