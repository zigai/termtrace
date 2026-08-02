//! Deterministic semantic-state video scheduling and source materialization.

use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{BufReader, Read as _, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    thread,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::write_secure,
    capability::{SEMANTIC_VIDEO_CAPABILITY, require_capability},
    event::{Event, OperationStatus},
    event_log::EventLog,
    ghostty_vt::SemanticState,
    replay::{StateRecord, visit_states},
    semantic_image::{
        SEMANTIC_CELL_HEIGHT, SEMANTIC_CELL_WIDTH, SEMANTIC_FONT_FAMILY, SEMANTIC_PNG_RENDERER,
        render_video_rgba, semantic_font_hash,
    },
    session::validate_completed_log,
};

const NANOS_PER_SECOND: u64 = 1_000_000_000;
/// Current semantic-video source format.
pub const SEMANTIC_VIDEO_VERSION: u32 = 2;

/// Fixed canvas used for every frame of one semantic video source.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticVideoCanvas {
    /// Largest terminal column count in the source execution.
    pub cols: u16,
    /// Largest terminal row count in the source execution.
    pub rows: u16,
    /// Deterministic semantic cell width.
    pub cell_width: u16,
    /// Deterministic semantic cell height.
    pub cell_height: u16,
    /// Encoded source width.
    pub width: u16,
    /// Encoded source height.
    pub height: u16,
    /// Stable resize/canvas policy identifier.
    pub policy: String,
}

/// Immutable semantic source description resolved during presentation planning.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticVideoSourcePlan {
    /// Source-plan schema version.
    pub version: u32,
    /// Canonical session path.
    pub session: PathBuf,
    /// Canonical session identity.
    pub session_id: Uuid,
    /// Terminal canonical event hash.
    pub source_event_hash: String,
    /// Inclusive source-time origin.
    pub start_monotonic_ns: u64,
    /// Inclusive completed-session endpoint used for the final frame hold.
    pub end_monotonic_ns: u64,
    /// Constant semantic source frame rate.
    pub fps: u16,
    /// Exact source frame count, including one final semantic-state frame.
    pub frame_count: u64,
    /// Fixed source canvas.
    pub canvas: SemanticVideoCanvas,
    /// Semantic renderer identity.
    pub renderer: String,
    /// Bundled deterministic font family.
    pub font_family: String,
    /// BLAKE3 identity of the bundled font bytes.
    pub font_hash: String,
    /// Stable state-to-frame scheduling policy.
    pub scheduling_policy: String,
}

/// One contiguous source-frame span rendered from one semantic state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticFrameSpan {
    /// Inclusive source frame.
    pub start_frame: u64,
    /// Exclusive source frame.
    pub end_frame: u64,
    /// Exact semantic state identifier.
    pub state_id: u64,
    /// Exact complete semantic state hash.
    pub state_hash: String,
    /// Canonical state timestamp.
    pub state_monotonic_ns: u64,
}

/// Provenance and scheduling result for one materialized semantic source video.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticVideoSourceManifest {
    /// Manifest schema version.
    pub version: u32,
    /// Exact immutable source plan.
    pub plan: SemanticVideoSourcePlan,
    /// Product truth layer.
    pub truth_layer: String,
    /// Number of presentation-eligible states in the selected interval.
    pub presented_state_count: u64,
    /// Number of presentation-eligible states owning at least one frame.
    pub rendered_state_count: u64,
    /// Number of presentation-eligible states suppressed by frame sampling.
    pub suppressed_state_count: u64,
    /// BLAKE3 of exact RGBA frames sent to the encoder.
    pub pixel_stream_hash: String,
    /// BLAKE3 of the lossless semantic source video.
    pub source_video_hash: String,
    /// First line of the exact FFmpeg build used.
    pub ffmpeg_version: String,
    /// Exact FFmpeg argument vector.
    pub ffmpeg_arguments: Vec<String>,
    /// Parsed FFprobe source-video evidence.
    pub video_probe: serde_json::Value,
}

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
    require_capability(SEMANTIC_VIDEO_CAPABILITY)?;

    let output = directory.join("semantic-source.mkv");
    let stderr_path = directory.join("semantic-source-ffmpeg.log");
    let stderr_file = File::create(&stderr_path).map_err(|source_error| {
        return Error::Filesystem {
            path: stderr_path.clone(),
            source: source_error,
        };
    })?;
    let ffmpeg_arguments = semantic_ffmpeg_arguments(source, &output);
    let mut child = Command::new("ffmpeg")
        .args(&ffmpeg_arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .map_err(|source_error| {
            return Error::Filesystem {
                path: PathBuf::from("ffmpeg"),
                source: source_error,
            };
        })?;
    let Some(mut stdin) = child.stdin.take() else {
        let _result = child.kill();
        let _result = child.wait();
        return Err(video_error("FFmpeg semantic source stdin was not created"));
    };
    let summary = match render_semantic_frames(source, &log, &mut stdin) {
        Ok(summary) => summary,
        Err(error) => {
            drop(stdin);
            let _result = child.kill();
            let _result = child.wait();
            return Err(error);
        }
    };
    drop(stdin);
    let status = child.wait().map_err(|source_error| {
        return Error::Filesystem {
            path: PathBuf::from("ffmpeg"),
            source: source_error,
        };
    })?;
    if !status.success() {
        return Err(video_error(format!(
            "FFmpeg semantic source export failed with {status}"
        )));
    }
    if summary.written_frames != source.frame_count {
        return Err(video_error(format!(
            "semantic source wrote {} frames instead of {}",
            summary.written_frames, source.frame_count
        )));
    }

    let (source_video_hash, ffmpeg_version, video_probe) = thread::scope(|scope| {
        let frame_map = scope.spawn(|| {
            return write_frame_spans(&directory.join("semantic-source-map.jsonl"), &summary.spans);
        });
        let source_hash = scope.spawn(|| return hash_file(&output));
        let version = scope.spawn(|| return command_version("ffmpeg"));
        let probe = scope.spawn(|| return probe_video(&output));
        frame_map
            .join()
            .map_err(|_panic| return video_error("semantic frame-map worker panicked"))??;
        let source_video_hash = source_hash
            .join()
            .map_err(|_panic| return video_error("semantic source-hash worker panicked"))??;
        let ffmpeg_version = version
            .join()
            .map_err(|_panic| return video_error("FFmpeg-version worker panicked"))??;
        let video_probe = probe
            .join()
            .map_err(|_panic| return video_error("semantic video-probe worker panicked"))??;
        return Ok((source_video_hash, ffmpeg_version, video_probe));
    })?;
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
    write_secure(
        &directory.join("semantic-source-manifest.json"),
        &serde_json::to_vec_pretty(&manifest).map_err(|source_error| {
            return Error::InvalidJson {
                path: directory.join("semantic-source-manifest.json"),
                json_path: "$".to_owned(),
                source: source_error,
            };
        })?,
    )?;
    return Ok((output, manifest));
}

fn render_semantic_frames(
    source: &SemanticVideoSourcePlan,
    log: &EventLog,
    output: &mut (impl Write + Send),
) -> Result<FrameSummary> {
    let worker_count = render_worker_count();
    let channel_capacity = worker_count.saturating_mul(2).max(1);
    let expected_bytes = usize::from(source.canvas.width)
        .saturating_mul(usize::from(source.canvas.height))
        .saturating_mul(4);
    return thread::scope(|scope| {
        let (task_sender, task_receiver) = sync_channel(channel_capacity);
        let task_receiver = Arc::new(Mutex::new(task_receiver));
        let (rendered_sender, rendered_receiver) = sync_channel(channel_capacity);
        let (hash_sender, hash_receiver) = sync_channel(channel_capacity);

        let hash_worker = scope.spawn(move || return hash_pixel_stream(hash_receiver));
        let collector = scope.spawn(move || {
            return collect_rendered_frames(rendered_receiver, hash_sender, output, expected_bytes);
        });
        let mut render_workers = Vec::with_capacity(worker_count);
        for _worker in 0..worker_count {
            let tasks = Arc::clone(&task_receiver);
            let rendered = rendered_sender.clone();
            let canvas_cols = source.canvas.cols;
            let canvas_rows = source.canvas.rows;
            render_workers.push(scope.spawn(move || {
                render_frames(tasks, rendered, canvas_cols, canvas_rows, expected_bytes);
            }));
        }
        drop(rendered_sender);

        let production = (|| -> Result<ScheduleSummary> {
            let mut scheduler = SemanticFrameScheduler::new(source, task_sender);
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
                    scheduler.emit(&previous_record, previous_state, boundary)?;
                }
                pending = Some((record.clone(), state.clone()));
                return Ok(());
            })?;
            let Some((final_record, final_state)) = pending else {
                return Err(video_error(
                    "semantic replay produced no presentation-eligible state",
                ));
            };
            scheduler.emit(&final_record, final_state, source.frame_count)?;
            return Ok(scheduler.finish());
        })();

        for worker in render_workers {
            worker
                .join()
                .map_err(|_panic| return video_error("semantic render worker panicked"))?;
        }
        let collected = collector
            .join()
            .map_err(|_panic| return video_error("semantic frame collector panicked"))??;
        let pixel_stream_hash = hash_worker
            .join()
            .map_err(|_panic| return video_error("semantic pixel-hash worker panicked"))??;
        let scheduled = production?;
        if collected.rendered_state_count != scheduled.rendered_state_count {
            return Err(video_error(
                "semantic render workers returned an incomplete state set",
            ));
        }
        return Ok(FrameSummary {
            written_frames: collected.written_frames,
            presented_state_count: scheduled.presented_state_count,
            rendered_state_count: collected.rendered_state_count,
            suppressed_state_count: scheduled.suppressed_state_count,
            pixel_stream_hash,
            spans: collected.spans,
        });
    });
}

struct SemanticFrameScheduler<'source> {
    source: &'source SemanticVideoSourcePlan,
    tasks: SyncSender<RenderTask>,
    next_frame: u64,
    next_sequence: u64,
    presented_state_count: u64,
    rendered_state_count: u64,
    suppressed_state_count: u64,
}

impl<'source> SemanticFrameScheduler<'source> {
    fn new(source: &'source SemanticVideoSourcePlan, tasks: SyncSender<RenderTask>) -> Self {
        return Self {
            source,
            tasks,
            next_frame: 0,
            next_sequence: 0,
            presented_state_count: 0,
            rendered_state_count: 0,
            suppressed_state_count: 0,
        };
    }

    fn emit(&mut self, record: &StateRecord, state: SemanticState, end_frame: u64) -> Result<()> {
        self.presented_state_count = self.presented_state_count.saturating_add(1);
        let end_frame = end_frame.min(self.source.frame_count);
        if end_frame <= self.next_frame {
            self.suppressed_state_count = self.suppressed_state_count.saturating_add(1);
            return Ok(());
        }
        let start_frame = self.next_frame;
        self.tasks
            .send(RenderTask {
                sequence: self.next_sequence,
                span: SemanticFrameSpan {
                    start_frame,
                    end_frame,
                    state_id: record.state_id,
                    state_hash: record.state_hash.clone(),
                    state_monotonic_ns: record.monotonic_ns,
                },
                state,
            })
            .map_err(|_error| return video_error("semantic render workers stopped early"))?;
        self.next_frame = end_frame;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.rendered_state_count = self.rendered_state_count.saturating_add(1);
        return Ok(());
    }

    fn finish(self) -> ScheduleSummary {
        return ScheduleSummary {
            presented_state_count: self.presented_state_count,
            rendered_state_count: self.rendered_state_count,
            suppressed_state_count: self.suppressed_state_count,
        };
    }
}

struct RenderTask {
    sequence: u64,
    span: SemanticFrameSpan,
    state: SemanticState,
}

struct RenderedFrame {
    sequence: u64,
    span: SemanticFrameSpan,
    pixels: Arc<Vec<u8>>,
}

struct PixelHashTask {
    pixels: Arc<Vec<u8>>,
    repetitions: u64,
}

struct ScheduleSummary {
    presented_state_count: u64,
    rendered_state_count: u64,
    suppressed_state_count: u64,
}

struct CollectedSummary {
    written_frames: u64,
    rendered_state_count: u64,
    spans: Vec<SemanticFrameSpan>,
}

fn render_worker_count() -> usize {
    let available = thread::available_parallelism().map_or(1, usize::from);
    return available.saturating_sub(1).clamp(1, 4);
}

fn render_frames(
    tasks: Arc<Mutex<Receiver<RenderTask>>>,
    rendered: SyncSender<Result<RenderedFrame>>,
    canvas_cols: u16,
    canvas_rows: u16,
    expected_bytes: usize,
) {
    loop {
        let task = match tasks.lock() {
            Ok(receiver) => match receiver.recv() {
                Ok(task) => task,
                Err(_closed) => return,
            },
            Err(_poisoned) => {
                let _result = rendered.send(Err(video_error(
                    "semantic render task receiver was poisoned",
                )));
                return;
            }
        };
        let result = render_video_rgba(&task.state, canvas_cols, canvas_rows).and_then(|pixels| {
            if pixels.len() != expected_bytes {
                return Err(video_error(
                    "semantic renderer returned an invalid RGBA frame",
                ));
            }
            return Ok(RenderedFrame {
                sequence: task.sequence,
                span: task.span,
                pixels: Arc::new(pixels),
            });
        });
        let failed = result.is_err();
        if rendered.send(result).is_err() || failed {
            return;
        }
    }
}

fn collect_rendered_frames(
    rendered: Receiver<Result<RenderedFrame>>,
    hashes: SyncSender<PixelHashTask>,
    output: &mut impl Write,
    expected_bytes: usize,
) -> Result<CollectedSummary> {
    let mut next_sequence = 0_u64;
    let mut pending = BTreeMap::new();
    let mut written_frames = 0_u64;
    let mut spans = Vec::new();
    while let Ok(result) = rendered.recv() {
        let frame = result?;
        if frame.pixels.len() != expected_bytes || pending.insert(frame.sequence, frame).is_some() {
            return Err(video_error(
                "semantic render collector received an invalid frame",
            ));
        }
        while let Some(frame) = pending.remove(&next_sequence) {
            let repetitions = frame.span.end_frame.saturating_sub(frame.span.start_frame);
            hashes
                .send(PixelHashTask {
                    pixels: Arc::clone(&frame.pixels),
                    repetitions,
                })
                .map_err(|_error| return video_error("semantic pixel-hash worker stopped early"))?;
            for _frame in frame.span.start_frame..frame.span.end_frame {
                output
                    .write_all(frame.pixels.as_slice())
                    .map_err(|source_error| {
                        return Error::Filesystem {
                            path: PathBuf::from("<ffmpeg-stdin>"),
                            source: source_error,
                        };
                    })?;
                written_frames = written_frames.saturating_add(1);
            }
            spans.push(frame.span);
            next_sequence = next_sequence.saturating_add(1);
        }
    }
    if !pending.is_empty() {
        return Err(video_error(
            "semantic render collector ended with an ordering gap",
        ));
    }
    return Ok(CollectedSummary {
        written_frames,
        rendered_state_count: u64::try_from(spans.len()).unwrap_or(u64::MAX),
        spans,
    });
}

fn hash_pixel_stream(tasks: Receiver<PixelHashTask>) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    while let Ok(task) = tasks.recv() {
        for _frame in 0..task.repetitions {
            hasher.update(task.pixels.as_slice());
        }
    }
    return Ok(hasher.finalize().to_hex().to_string());
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
        serde_json::to_writer(&mut bytes, span).map_err(|source| {
            return Error::InvalidJson {
                path: path.to_path_buf(),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        bytes.push(b'\n');
    }
    return write_secure(path, &bytes);
}

fn command_version(command: &str) -> Result<String> {
    let output = Command::new(command)
        .arg("-version")
        .output()
        .map_err(|source| {
            return Error::Filesystem {
                path: PathBuf::from(command),
                source,
            };
        })?;
    if !output.status.success() {
        return Err(video_error(format!("{command} -version failed")));
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
        .map_err(|source| {
            return Error::Filesystem {
                path: PathBuf::from("ffprobe"),
                source,
            };
        })?;
    if !output.status.success() {
        return Err(video_error("FFprobe rejected the semantic source video"));
    }
    return serde_json::from_slice(&output.stdout).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<ffprobe-semantic-source>"),
            json_path: "$".to_owned(),
            source,
        };
    });
}

fn hash_file(path: &Path) -> Result<String> {
    let file = File::open(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    let mut reader = BufReader::new(file);
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut hasher = blake3::Hasher::new();
    loop {
        let count = reader.read(&mut buffer).map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
        if count == 0 {
            break;
        }
        let chunk = buffer
            .get(..count)
            .ok_or_else(|| return video_error("source hash reader returned an invalid length"))?;
        hasher.update(chunk);
    }
    return Ok(hasher.finalize().to_hex().to_string());
}

fn video_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ghostty_vt::{DirtyState, GhosttyVt},
        replay::StateDelta,
    };

    #[test]
    fn semantic_frame_boundaries_never_anticipate_a_state() {
        assert_eq!(frame_ceil(0, 24), 0);
        assert_eq!(frame_ceil(1, 24), 1);
        assert_eq!(frame_ceil(41_666_666, 24), 1);
        assert_eq!(frame_ceil(41_666_667, 24), 2);
    }

    #[test]
    fn ordered_frame_collector_rasterizes_once_and_maps_the_complete_span() {
        let mut engine = GhosttyVt::new(2, 2, 0).expect("semantic engine");
        let state = engine.snapshot().expect("semantic state");
        let source = SemanticVideoSourcePlan {
            version: SEMANTIC_VIDEO_VERSION,
            session: PathBuf::from("session"),
            session_id: Uuid::nil(),
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
        let (task_sender, task_receiver) = sync_channel(1);
        let mut scheduler = SemanticFrameScheduler::new(&source, task_sender);
        scheduler.emit(&record, state, 2).expect("schedule span");
        let scheduled = scheduler.finish();
        let task = task_receiver.recv().expect("render task");
        let pixels = Arc::new(
            render_video_rgba(&task.state, source.canvas.cols, source.canvas.rows)
                .expect("render frame"),
        );
        let (rendered_sender, rendered_receiver) = sync_channel(1);
        rendered_sender
            .send(Ok(RenderedFrame {
                sequence: task.sequence,
                span: task.span,
                pixels,
            }))
            .expect("send rendered frame");
        drop(rendered_sender);
        let (hash_sender, hash_receiver) = sync_channel(1);
        let mut bytes = Vec::new();
        let frame_bytes = usize::from(source.canvas.width)
            .saturating_mul(usize::from(source.canvas.height))
            .saturating_mul(4);
        let collected =
            collect_rendered_frames(rendered_receiver, hash_sender, &mut bytes, frame_bytes)
                .expect("collect rendered frame");
        let pixel_stream_hash = hash_pixel_stream(hash_receiver).expect("hash pixel stream");

        assert_eq!(bytes.len(), frame_bytes * 2);
        assert_eq!(collected.written_frames, 2);
        assert_eq!(collected.rendered_state_count, 1);
        assert_eq!(scheduled.suppressed_state_count, 0);
        assert_eq!(pixel_stream_hash, blake3::hash(&bytes).to_hex().to_string());
        assert_eq!(collected.spans[0].state_id, 7);
        assert_eq!(
            (collected.spans[0].start_frame, collected.spans[0].end_frame),
            (0, 2)
        );
    }

    #[test]
    fn frame_collector_restores_worker_completion_order_before_writing_and_hashing() {
        let span = |start_frame, end_frame, state_id| {
            return SemanticFrameSpan {
                start_frame,
                end_frame,
                state_id,
                state_hash: state_id.to_string().repeat(64),
                state_monotonic_ns: state_id,
            };
        };
        let (rendered_sender, rendered_receiver) = sync_channel(2);
        rendered_sender
            .send(Ok(RenderedFrame {
                sequence: 1,
                span: span(1, 3, 2),
                pixels: Arc::new(vec![2_u8; 4]),
            }))
            .expect("send later frame first");
        rendered_sender
            .send(Ok(RenderedFrame {
                sequence: 0,
                span: span(0, 1, 1),
                pixels: Arc::new(vec![1_u8; 4]),
            }))
            .expect("send earlier frame second");
        drop(rendered_sender);
        let (hash_sender, hash_receiver) = sync_channel(2);
        let mut bytes = Vec::new();
        let collected = collect_rendered_frames(rendered_receiver, hash_sender, &mut bytes, 4)
            .expect("collect frames in sequence order");
        let pixel_stream_hash = hash_pixel_stream(hash_receiver).expect("hash ordered pixels");

        assert_eq!(
            bytes,
            [vec![1_u8; 4], vec![2_u8; 4], vec![2_u8; 4]].concat()
        );
        assert_eq!(
            collected
                .spans
                .iter()
                .map(|span| return span.state_id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(pixel_stream_hash, blake3::hash(&bytes).to_hex().to_string());
    }
}
