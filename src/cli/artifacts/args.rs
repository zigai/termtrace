use crate::cli::{
    analysis::args::{ShowRenderer, TimeRangeArgs},
    parsers::{
        parse_capture_target, parse_media_hold_duration, parse_state_hash, parse_video_fps,
        parse_video_output_path,
    },
};
use clap::{ArgGroup, Args, Subcommand, ValueEnum};
use std::{path::PathBuf, time::Duration};
use termtrace_engine::timeline::parse_time_offset_ns;
use termtrace_model::{
    artifact::{
        bundle::BundleProfile,
        save::{SaveFormat, SemanticRasterProfile},
    },
    execution::session::CaptureTarget,
};

#[derive(Debug, Args)]
pub(in crate::cli) struct ArtifactArgs {
    #[command(subcommand)]
    pub(in crate::cli) command: ArtifactSubcommand,
}

#[derive(Debug, Subcommand)]
pub(in crate::cli) enum ArtifactSubcommand {
    /// Import an ANSI/VT byte stream as canonical evidence.
    Import(ImportArgs),
    /// Capture actual Ghostty pixels on a private headless display.
    Capture(CaptureArgs),
    /// Atomically publish representations of one semantic state.
    Save(SaveArgs),
    /// Export a lossy UTF-8 asciinema v2 interoperability cast.
    Asciicast(ExportArgs),
}

#[derive(Debug, Args)]
pub(in crate::cli) struct BugReportFacadeArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// New owner-only bug-report directory.
    #[arg(short = 'o', long)]
    pub(in crate::cli) out: PathBuf,
    /// Include a semantic PNG screenshot of the last presented screen.
    #[arg(long)]
    pub(in crate::cli) screenshot: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(in crate::cli) enum BundleProfileArg {
    /// Bundle only viewport screen presentation evidence.
    Screen,
    /// Bundle state history and diagnostic summaries for debugging.
    Debug,
    /// Bundle complete byte-exact PTY event streams and all artifacts.
    Forensic,
}

impl From<BundleProfileArg> for BundleProfile {
    fn from(profile: BundleProfileArg) -> Self {
        return match profile {
            BundleProfileArg::Screen => Self::Screen,
            BundleProfileArg::Debug => Self::Debug,
            BundleProfileArg::Forensic => Self::Forensic,
        };
    }
}

#[derive(Debug, Args)]
#[expect(clippy::struct_excessive_bools, reason = "Independent CLI flags")]
pub(in crate::cli) struct BundleArgs {
    /// Completed canonical session.
    pub(in crate::cli) session: PathBuf,
    /// Fixed content and sensitivity policy.
    #[arg(long, value_enum, default_value_t = BundleProfileArg::Screen)]
    pub(in crate::cli) profile: BundleProfileArg,
    /// New owner-only destination directory.
    #[arg(short = 'o', long)]
    pub(in crate::cli) out: PathBuf,
    /// Print the selected paths and sensitivities without writing.
    #[arg(long)]
    pub(in crate::cli) dry_run: bool,
    /// Publish a deterministic compressed archive instead of a directory.
    #[arg(long)]
    pub(in crate::cli) archive: bool,
    /// Include a semantic PNG screenshot of the last presented screen.
    #[arg(long)]
    pub(in crate::cli) screenshot: bool,
    /// Emit stable structured output as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct BundleVerifyArgs {
    /// Published bundle directory.
    pub(in crate::cli) bundle: PathBuf,
    /// Trusted canonical source session whose identity and final hash are expected.
    #[arg(long)]
    pub(in crate::cli) source: PathBuf,
    /// Exact content and sensitivity policy expected by the verifier.
    #[arg(long, value_enum)]
    pub(in crate::cli) profile: BundleProfileArg,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct ImportArgs {
    /// ANSI/VT byte stream path, or `-` for standard input.
    #[arg(long)]
    pub(in crate::cli) input: PathBuf,
    /// New canonical session directory.
    #[arg(long)]
    pub(in crate::cli) out: PathBuf,
    /// Imported terminal columns (1-65535).
    #[arg(long, default_value_t = 160, value_parser = clap::value_parser!(u16).range(1..))]
    pub(in crate::cli) cols: u16,
    /// Imported terminal rows (1-65535).
    #[arg(long, default_value_t = 45, value_parser = clap::value_parser!(u16).range(1..))]
    pub(in crate::cli) rows: u16,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct ExportArgs {
    /// Source session directory.
    #[arg(long)]
    pub(in crate::cli) session: PathBuf,
    /// New asciicast v2 output path.
    #[arg(long)]
    pub(in crate::cli) out: PathBuf,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(in crate::cli) enum SaveFormatArg {
    /// Plain text screen representation.
    Txt,
    /// JSON representation of the semantic grid.
    Json,
    /// Deep forensic JSON representation including escape sequences and styles.
    DeepJson,
    /// Raw VT escape sequences reconstructing the screen state.
    Vt,
    /// Self-contained HTML document rendering the screen state.
    Html,
    /// Scalable Vector Graphics image representation.
    Svg,
    /// Portable Network Graphics raster image.
    Png,
}

impl From<SaveFormatArg> for SaveFormat {
    fn from(value: SaveFormatArg) -> Self {
        return match value {
            SaveFormatArg::Txt => Self::Txt,
            SaveFormatArg::Json => Self::Json,
            SaveFormatArg::DeepJson => Self::DeepJson,
            SaveFormatArg::Vt => Self::Vt,
            SaveFormatArg::Html => Self::Html,
            SaveFormatArg::Svg => Self::Svg,
            SaveFormatArg::Png => Self::Png,
        };
    }
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("save_selector")
        .multiple(false)
        .args(["state", "state_hash", "event_seq", "at", "at_marker"])
))]
pub(in crate::cli) struct SaveArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// New destination directory.
    #[arg(long)]
    pub(in crate::cli) out: PathBuf,
    /// One or more comma-separated representations.
    #[arg(long, value_enum, value_delimiter = ',', required = true)]
    pub(in crate::cli) format: Vec<SaveFormatArg>,
    /// Exact semantic state identifier.
    #[arg(long)]
    pub(in crate::cli) state: Option<u64>,
    /// Exact content-addressed semantic state hash.
    #[arg(long, value_parser = parse_state_hash)]
    pub(in crate::cli) state_hash: Option<String>,
    /// Latest state at or before this event sequence.
    #[arg(long)]
    pub(in crate::cli) event_seq: Option<u64>,
    /// Latest state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns)]
    pub(in crate::cli) at: Option<u64>,
    /// State at a unique marker.
    #[arg(long)]
    pub(in crate::cli) at_marker: Option<String>,
    /// Permit an explicitly selected synchronized-output hidden state.
    #[arg(long)]
    pub(in crate::cli) include_hidden: bool,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("screenshot_selector")
        .multiple(false)
        .args(["state", "state_hash", "event_seq", "at", "at_marker"])
))]
pub(in crate::cli) struct ScreenshotArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// New manifested screenshot artifact directory.
    #[arg(short = 'o', long)]
    pub(in crate::cli) out: PathBuf,
    /// Exact semantic state identifier.
    #[arg(long)]
    pub(in crate::cli) state: Vec<u64>,
    /// Exact content-addressed semantic state hash.
    #[arg(long, value_parser = parse_state_hash)]
    pub(in crate::cli) state_hash: Vec<String>,
    /// Latest state at or before this event sequence.
    #[arg(long)]
    pub(in crate::cli) event_seq: Vec<u64>,
    /// Latest state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns)]
    pub(in crate::cli) at: Vec<u64>,
    /// State at a unique marker.
    #[arg(long)]
    pub(in crate::cli) at_marker: Vec<String>,
    /// Permit an explicitly selected synchronized-output hidden state.
    #[arg(long)]
    pub(in crate::cli) include_hidden: bool,
    /// Semantic image representation.
    #[arg(long, value_enum, default_value_t = ScreenshotFormat::Png)]
    pub(in crate::cli) format: ScreenshotFormat,
    /// Semantic state renderer or actual Ghostty native frame.
    #[arg(long, value_enum, default_value_t = ShowRenderer::Semantic)]
    pub(in crate::cli) renderer: ShowRenderer,
    /// Strict whole native viewport as `WIDTHxHEIGHT`; omit to solve the exact recorded grid.
    #[arg(long, value_parser = parse_capture_target)]
    pub(in crate::cli) viewport: Option<CaptureTarget>,
    /// Emphasize that a supplied viewport is an exact evidentiary assertion.
    #[arg(long, requires = "viewport")]
    pub(in crate::cli) strict: bool,
    /// Emit stable structured output as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(in crate::cli) enum ScreenshotFormat {
    /// Scalable Vector Graphics vector screenshot.
    Svg,
    /// Portable Network Graphics raster screenshot.
    Png,
}

#[derive(Debug, Args)]
pub(in crate::cli) struct CaptureArgs {
    /// Indexed source session directory.
    #[arg(long)]
    pub(in crate::cli) session: PathBuf,
    /// Whole capture viewport width; omit both dimensions to use the recorded target or solve the exact grid.
    #[arg(long, requires = "height")]
    pub(in crate::cli) width: Option<u16>,
    /// Whole capture viewport height; omit both dimensions to use the recorded target or solve the exact grid.
    #[arg(long, requires = "width")]
    pub(in crate::cli) height: Option<u16>,
    /// Video frame rate: 24, 30, or 60 FPS.
    #[arg(long, value_parser = parse_video_fps, default_value_t = 24)]
    pub(in crate::cli) fps: u16,
    /// Use a matching index containing only terminal-presentable states.
    #[arg(long)]
    pub(in crate::cli) presented_only: bool,
    #[command(flatten)]
    pub(in crate::cli) range: TimeRangeArgs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(in crate::cli) enum VideoSourceArg {
    /// Verified pixels captured from the real Ghostty renderer.
    Native,
    /// Deterministic pixels generated from canonical semantic replay.
    Semantic,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(in crate::cli) enum SemanticRasterProfileArg {
    /// Rasterize directly at the output dimensions.
    Direct,
    /// Rasterize at twice the output dimensions and downsample once.
    #[value(name = "supersampled-2x")]
    Supersampled2x,
}

impl From<SemanticRasterProfileArg> for SemanticRasterProfile {
    fn from(value: SemanticRasterProfileArg) -> Self {
        return match value {
            SemanticRasterProfileArg::Direct => Self::Direct,
            SemanticRasterProfileArg::Supersampled2x => Self::Supersampled2x,
        };
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(in crate::cli) enum VideoPresetArg {
    /// Crisp, compact H.264 suitable for documentation.
    Docs,
    /// High-motion H.264 suitable for social publishing.
    Social,
    /// Lossless H.264 encoding with full chroma.
    Lossless,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(in crate::cli) enum VideoCursorStyleArg {
    /// Preserve the cursor captured in the source.
    Recorded,
    /// Mask the cursor using canonical cursor geometry.
    Hidden,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("video_mode")
        .required(true)
        .multiple(false)
        .args(["out", "dry_run", "preview_out"])
))]
#[expect(clippy::struct_excessive_bools, reason = "Independent CLI flags")]
pub(in crate::cli) struct VideoFacadeArgs {
    /// Completed session name or canonical path.
    pub(in crate::cli) session: PathBuf,
    /// New MP4 output file; a manifested `.termtrace` sidecar is retained.
    #[arg(short, long, value_parser = parse_video_output_path)]
    pub(in crate::cli) out: Option<PathBuf>,
    /// Resolve and print the presentation plan without rendering.
    #[arg(long)]
    pub(in crate::cli) dry_run: bool,
    /// Render a contact sheet and low-resolution review video into this new directory.
    #[arg(long)]
    pub(in crate::cli) preview_out: Option<PathBuf>,
    /// Strict versioned edit-plan JSON; omit to derive a plan from the facade options.
    #[arg(
        long,
        conflicts_with_all = [
            "preset",
            "resolution",
            "scale",
            "start_marker",
            "end_marker",
            "trim_idle",
            "hold_start",
            "hold_end",
            "cursor_style"
        ]
    )]
    pub(in crate::cli) edit: Option<PathBuf>,
    /// Delivery quality and default timing policy.
    #[arg(long, value_enum, default_value_t = VideoPresetArg::Docs)]
    pub(in crate::cli) preset: VideoPresetArg,
    /// Exact output frame rate; native capture must match.
    #[arg(long, value_parser = parse_video_fps)]
    pub(in crate::cli) fps: Option<u16>,
    /// Semantic replay or verified native pixels; native never falls back.
    #[arg(long, value_enum, default_value_t = VideoSourceArg::Native)]
    pub(in crate::cli) renderer: VideoSourceArg,
    /// Semantic rasterization quality; invalid for native rendering.
    #[arg(long, value_enum)]
    pub(in crate::cli) semantic_raster_profile: Option<SemanticRasterProfileArg>,
    /// Exact even output dimensions as `WIDTHxHEIGHT`.
    #[arg(long, value_parser = parse_capture_target, conflicts_with = "scale")]
    pub(in crate::cli) resolution: Option<CaptureTarget>,
    /// Integer output scale from 1 through 8.
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=8), conflicts_with = "resolution")]
    pub(in crate::cli) scale: Option<u16>,
    /// Begin at this unique marker.
    #[arg(long)]
    pub(in crate::cli) start_marker: Option<String>,
    /// End at this unique marker.
    #[arg(long)]
    pub(in crate::cli) end_marker: Option<String>,
    /// Remove child-output gaps longer than two seconds.
    #[arg(long)]
    pub(in crate::cli) trim_idle: bool,
    /// Hold the first frame; defaults are preset-specific.
    #[arg(long, value_parser = parse_media_hold_duration)]
    pub(in crate::cli) hold_start: Option<Duration>,
    /// Hold the final frame; defaults are preset-specific.
    #[arg(long, value_parser = parse_media_hold_duration)]
    pub(in crate::cli) hold_end: Option<Duration>,
    /// Preserve or hide the recorded cursor.
    #[arg(long, value_enum, default_value_t = VideoCursorStyleArg::Recorded)]
    pub(in crate::cli) cursor_style: VideoCursorStyleArg,
    /// Storyboard tile columns.
    #[arg(long, default_value_t = 4, requires = "preview_out")]
    pub(in crate::cli) columns: u8,
    /// Width of each storyboard tile.
    #[arg(long, default_value_t = 320, requires = "preview_out")]
    pub(in crate::cli) tile_width: u16,
    /// Maximum storyboard frames.
    #[arg(long, default_value_t = 12, requires = "preview_out")]
    pub(in crate::cli) max_frames: u16,
    /// Retain only frames carrying configured change visualization.
    #[arg(long, requires = "preview_out")]
    pub(in crate::cli) change_only: bool,
    /// Low-resolution preview width.
    #[arg(long, default_value_t = 640, requires = "preview_out")]
    pub(in crate::cli) preview_width: u16,
    /// Emit stable structured output as JSON.
    #[arg(long)]
    pub(in crate::cli) json: bool,
}
