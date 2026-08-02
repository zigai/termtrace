use super::*;

#[derive(Debug, Args)]
pub(super) struct ArtifactArgs {
    #[command(subcommand)]
    pub(super) command: ArtifactSubcommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum ArtifactSubcommand {
    /// Capture actual Ghostty pixels on a private headless display.
    Capture(CaptureArgs),
    /// Atomically publish representations of one semantic state.
    Save(SaveArgs),
    /// Render one deterministic semantic screenshot artifact directory.
    Screenshot(ScreenshotArgs),
    /// Plan or export presentation edits; native Ghostty pixels are the default source.
    Video(VideoArgs),
    /// Publish or inspect a deterministic sensitivity-labeled handoff.
    Bundle(ArtifactBundleArgs),
    /// Export a lossy UTF-8 asciinema v2 interoperability cast.
    Asciicast(ExportArgs),
}

#[derive(Debug, Args)]
pub(super) struct ArtifactBundleArgs {
    #[command(subcommand)]
    pub(super) command: ArtifactBundleSubcommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum ArtifactBundleSubcommand {
    /// Publish a deterministic sensitivity-labeled handoff directory.
    Create(BundleArgs),
    /// Verify every manifested bundle member and included evidence.
    Verify(BundleVerifyArgs),
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum BundleProfileArg {
    Screen,
    Debug,
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
pub(super) struct BundleArgs {
    /// Completed canonical session.
    pub(super) session: PathBuf,
    /// Fixed content and sensitivity policy.
    #[arg(long, value_enum, default_value_t = BundleProfileArg::Screen)]
    pub(super) profile: BundleProfileArg,
    /// New owner-only destination directory.
    #[arg(long)]
    pub(super) out: PathBuf,
    /// Print the selected paths and sensitivities without writing.
    #[arg(long)]
    pub(super) dry_run: bool,
    /// Publish a deterministic compressed archive instead of a directory.
    #[arg(long)]
    pub(super) archive: bool,
}

#[derive(Debug, Args)]
pub(super) struct BundleVerifyArgs {
    /// Published bundle directory.
    pub(super) bundle: PathBuf,
    /// Trusted canonical source session whose identity and final hash are expected.
    #[arg(long)]
    pub(super) source: PathBuf,
    /// Exact content and sensitivity policy expected by the verifier.
    #[arg(long, value_enum)]
    pub(super) profile: BundleProfileArg,
}

#[derive(Debug, Args)]
pub(super) struct ExportArgs {
    /// Source session directory.
    #[arg(long)]
    pub(super) session: PathBuf,
    /// New asciicast v2 output path.
    #[arg(long)]
    pub(super) out: PathBuf,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum SaveFormatArg {
    Txt,
    Json,
    DeepJson,
    Vt,
    Html,
    Svg,
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
pub(super) struct SaveArgs {
    /// Completed session name or canonical path.
    pub(super) session: PathBuf,
    /// New destination directory.
    #[arg(long)]
    pub(super) out: PathBuf,
    /// One or more comma-separated representations.
    #[arg(long, value_enum, value_delimiter = ',', required = true)]
    pub(super) format: Vec<SaveFormatArg>,
    /// Exact semantic state identifier.
    #[arg(long)]
    pub(super) state: Option<u64>,
    /// Exact content-addressed semantic state hash.
    #[arg(long, value_parser = parse_state_hash)]
    pub(super) state_hash: Option<String>,
    /// Latest state at or before this event sequence.
    #[arg(long)]
    pub(super) event_seq: Option<u64>,
    /// Latest state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns)]
    pub(super) at: Option<u64>,
    /// State at a unique marker.
    #[arg(long)]
    pub(super) at_marker: Option<String>,
    /// Permit an explicitly selected synchronized-output hidden state.
    #[arg(long)]
    pub(super) include_hidden: bool,
}

#[derive(Debug, Args)]
#[command(group(
    ArgGroup::new("screenshot_selector")
        .multiple(false)
        .args(["state", "state_hash", "event_seq", "at", "at_marker"])
))]
pub(super) struct ScreenshotArgs {
    /// Completed session name or canonical path.
    pub(super) session: PathBuf,
    /// New artifact directory containing `screen.svg` and its manifest.
    #[arg(long)]
    pub(super) out: PathBuf,
    /// Exact semantic state identifier.
    #[arg(long)]
    pub(super) state: Vec<u64>,
    /// Exact content-addressed semantic state hash.
    #[arg(long, value_parser = parse_state_hash)]
    pub(super) state_hash: Vec<String>,
    /// Latest state at or before this event sequence.
    #[arg(long)]
    pub(super) event_seq: Vec<u64>,
    /// Latest state at or before this session offset.
    #[arg(long, value_parser = parse_time_offset_ns)]
    pub(super) at: Vec<u64>,
    /// State at a unique marker.
    #[arg(long)]
    pub(super) at_marker: Vec<String>,
    /// Permit an explicitly selected synchronized-output hidden state.
    #[arg(long)]
    pub(super) include_hidden: bool,
    /// Semantic image representation.
    #[arg(long, value_enum, default_value_t = ScreenshotFormat::Png)]
    pub(super) format: ScreenshotFormat,
    /// Semantic state renderer or actual Ghostty native frame.
    #[arg(long, value_enum, default_value_t = ShowRenderer::Semantic)]
    pub(super) renderer: ShowRenderer,
    /// Strict whole native viewport as `WIDTHxHEIGHT`; omit to solve the exact recorded grid.
    #[arg(long, value_parser = parse_capture_target)]
    pub(super) viewport: Option<CaptureTarget>,
    /// Emphasize that a supplied viewport is an exact evidentiary assertion.
    #[arg(long, requires = "viewport")]
    pub(super) strict: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(super) enum ScreenshotFormat {
    Svg,
    Png,
}

#[derive(Debug, Args)]
pub(super) struct CaptureArgs {
    /// Indexed source session directory.
    #[arg(long)]
    pub(super) session: PathBuf,
    /// Whole capture viewport width; omit both dimensions to use the recorded target or solve the exact grid.
    #[arg(long, requires = "height")]
    pub(super) width: Option<u16>,
    /// Whole capture viewport height; omit both dimensions to use the recorded target or solve the exact grid.
    #[arg(long, requires = "width")]
    pub(super) height: Option<u16>,
    /// Video frame rate; native capture currently requires 24.
    #[arg(long, default_value_t = 24)]
    pub(super) fps: u16,
    /// Use a matching index containing only terminal-presentable states.
    #[arg(long)]
    pub(super) presented_only: bool,
    #[command(flatten)]
    pub(super) range: TimeRangeArgs,
}

#[derive(Debug, Args)]
pub(super) struct VideoArgs {
    #[command(subcommand)]
    pub(super) command: VideoCommand,
}

#[derive(Debug, Subcommand)]
pub(super) enum VideoCommand {
    /// Resolve clips, speeds, and holds without rendering.
    Plan(VideoPlanArgs),
    /// Render an atomic contact sheet and low-resolution review video.
    Preview(VideoPreviewArgs),
    /// Export an H.264 MP4 and frame provenance into a new directory.
    Export(VideoExportArgs),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(super) enum VideoSourceArg {
    /// Verified pixels captured from the real Ghostty renderer.
    Native,
    /// Deterministic pixels generated from canonical semantic replay.
    Semantic,
}

#[derive(Debug, Args)]
pub(super) struct VideoSourceArgs {
    /// Presentation source; native Ghostty pixels remain the default.
    #[arg(long, value_enum, default_value_t = VideoSourceArg::Native)]
    pub(super) source: VideoSourceArg,
    /// Semantic source frame rate; invalid for native capture.
    #[arg(long)]
    pub(super) fps: Option<u16>,
}

#[derive(Debug, Args)]
pub(super) struct VideoPreviewArgs {
    /// Completed session; native source additionally requires a verified capture.
    pub(super) session: PathBuf,
    #[command(flatten)]
    pub(super) source: VideoSourceArgs,
    /// Strict versioned edit-plan JSON.
    #[arg(long)]
    pub(super) edit: PathBuf,
    /// New no-replace preview artifact directory.
    #[arg(long)]
    pub(super) out: PathBuf,
    /// Storyboard tile columns.
    #[arg(long, default_value_t = 4)]
    pub(super) columns: u8,
    /// Width of each storyboard tile.
    #[arg(long, default_value_t = 320)]
    pub(super) tile_width: u16,
    /// Maximum storyboard frames.
    #[arg(long, default_value_t = 12)]
    pub(super) max_frames: u16,
    /// Retain only frames carrying configured change visualization.
    #[arg(long)]
    pub(super) change_only: bool,
    /// Low-resolution preview width.
    #[arg(long, default_value_t = 640)]
    pub(super) preview_width: u16,
}

#[derive(Debug, Args)]
pub(super) struct VideoPlanArgs {
    /// Completed session; native source additionally requires a verified capture.
    pub(super) session: PathBuf,
    #[command(flatten)]
    pub(super) source: VideoSourceArgs,
    /// Strict versioned edit-plan JSON.
    #[arg(long)]
    pub(super) edit: PathBuf,
}

#[derive(Debug, Args)]
pub(super) struct VideoExportArgs {
    /// Completed session; native source additionally requires a verified capture.
    pub(super) session: PathBuf,
    #[command(flatten)]
    pub(super) source: VideoSourceArgs,
    /// Strict versioned edit-plan JSON.
    #[arg(long)]
    pub(super) edit: PathBuf,
    /// New no-replace presentation artifact directory.
    #[arg(long)]
    pub(super) out: PathBuf,
}
