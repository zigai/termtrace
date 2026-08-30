//! Edited presentation plan, manifest, and report contracts.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    artifact::{
        metadata::ArtifactMember, save::SemanticRasterProfile,
        semantic_video::SemanticVideoSourcePlan,
    },
    terminal::replay::SourceRange,
    value::geometry::TerminalGeometryReport,
};

/// Current edit-plan format version.
pub const EDIT_PLAN_VERSION: u32 = 1;
/// Current normalized presentation plan and manifest format version.
pub const PRESENTATION_FORMAT_VERSION: u32 = 2;

/// One immutable presentation edit plan.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditPlan {
    /// Edit-plan format version.
    pub version: u32,
    /// Caller-ordered source clips.
    pub clips: Vec<EditClip>,
    /// Arbitrary normalized text annotation tracks.
    #[serde(default)]
    pub annotations: Vec<TextAnnotation>,
    /// Caller-authored ASS subtitle resources.
    #[serde(default)]
    pub subtitles: Vec<SubtitleTrack>,
    /// Optional per-frame change overlay policy.
    pub change_visualization: Option<ChangeVisualization>,
    /// Presentation canvas and caption policy.
    #[serde(default)]
    pub layout: PresentationLayout,
    /// Delivery encoding contract.
    #[serde(default)]
    pub delivery: PresentationDelivery,
    /// Explicit visibly labeled pixel transforms.
    #[serde(default)]
    pub transforms: Vec<PixelTransform>,
    /// User-facing video facade settings retained in the final manifest.
    #[serde(default)]
    pub render_config: Option<VideoRenderConfig>,
}

/// One source interval with rational speed and a generated final hold.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditClip {
    /// Inclusive source selector.
    pub from: EditSelector,
    /// Exclusive source selector.
    pub to: EditSelector,
    /// Playback speed as an exact rational.
    #[serde(default)]
    pub speed: RationalSpeed,
    /// Generated hold duration after the final retimed source frame.
    #[serde(default)]
    pub hold_ms: u64,
    /// Optional human presentation caption.
    pub caption: Option<String>,
}

/// Effective user-facing video facade settings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VideoRenderConfig {
    /// Named delivery preset.
    pub preset: String,
    /// Requested renderer without fallback.
    pub renderer: String,
    /// Effective output frame rate.
    pub fps: u16,
    /// Exact requested output resolution.
    pub resolution: Option<[u16; 2]>,
    /// Requested integer scale factor.
    pub scale: Option<u16>,
    /// Optional inclusive marker.
    pub start_marker: Option<String>,
    /// Optional exclusive marker.
    pub end_marker: Option<String>,
    /// Whether long gaps without child output were removed.
    pub trim_idle: bool,
    /// Idle gap threshold used by trimming.
    pub idle_threshold_ms: Option<u64>,
    /// Generated opening hold.
    pub hold_start_ms: u64,
    /// Generated closing hold.
    pub hold_end_ms: u64,
    /// Effective cursor presentation style.
    pub cursor_style: String,
}

/// Unique marker or exact canonical monotonic timestamp.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditSelector {
    /// Unique canonical marker label.
    pub marker: Option<String>,
    /// Exact canonical session offset in nanoseconds.
    pub monotonic_ns: Option<u64>,
    /// Latest semantic state at or before this canonical event.
    pub event_seq: Option<u64>,
    /// Exact semantic state identifier.
    pub state_id: Option<u64>,
    /// Exact native source frame.
    pub native_frame: Option<u64>,
}

/// One text annotation with an exact scope, position, and visible style.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextAnnotation {
    /// Literal text; mutually exclusive with `template`.
    pub text: Option<String>,
    /// Dynamic template expanded per output frame.
    pub template: Option<String>,
    /// Output/source selection scope.
    pub scope: AnnotationScope,
    /// Exact anchored placement.
    #[serde(default)]
    pub position: AnnotationPosition,
    /// Visible text/background style.
    #[serde(default)]
    pub style: AnnotationStyle,
    /// Stable overlay ordering.
    #[serde(default)]
    pub z_order: i16,
}

/// Annotation lifetime on the normalized presentation timeline.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub enum AnnotationScope {
    /// Every output frame.
    WholeVideo,
    /// One normalized clip.
    Clip {
        /// Zero-based caller clip index.
        clip_index: usize,
    },
    /// One exact output frame.
    OutputFrame {
        /// Zero-based presentation frame.
        output_frame: u64,
    },
    /// A source-selector-bounded half-open interval.
    Range {
        /// Inclusive source endpoint.
        from: EditSelector,
        /// Exclusive source endpoint.
        to: EditSelector,
    },
}

/// Named anchor plus signed pixel offset.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnnotationPosition {
    /// Named canvas anchor.
    pub anchor: AnnotationAnchor,
    /// Signed horizontal offset from the anchor.
    pub x: i32,
    /// Signed vertical offset from the anchor.
    pub y: i32,
}

impl Default for AnnotationPosition {
    fn default() -> Self {
        return Self {
            anchor: AnnotationAnchor::TopLeft,
            x: 16,
            y: 16,
        };
    }
}

/// Stable annotation anchor vocabulary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AnnotationAnchor {
    /// Top-left canvas corner.
    #[default]
    TopLeft,
    /// Top-right canvas corner.
    TopRight,
    /// Bottom-left canvas corner.
    BottomLeft,
    /// Bottom-right canvas corner.
    BottomRight,
    /// Canvas center.
    Center,
}

/// Deterministic visible text style compiled to ASS.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AnnotationStyle {
    /// Font size in pixels.
    pub font_size: u16,
    /// `#RRGGBB` foreground.
    pub foreground: String,
    /// `#RRGGBB` background.
    pub background: String,
    /// Background opacity from zero through 255.
    pub background_opacity: u8,
    /// Border width in pixels.
    pub border: u8,
}

impl Default for AnnotationStyle {
    fn default() -> Self {
        return Self {
            font_size: 20,
            foreground: "#ffffff".to_owned(),
            background: "#000000".to_owned(),
            background_opacity: 160,
            border: 1,
        };
    }
}

/// Caller-authored ASS resource copied and hash-manifested before rendering.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubtitleTrack {
    /// ASS subtitle document.
    pub ass: PathBuf,
}

/// Validated subtitle input with stable content identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NormalizedSubtitle {
    /// Resolved caller resource path used for this export.
    pub source_path: PathBuf,
    /// BLAKE3 of the exact imported ASS bytes.
    pub resource_hash: String,
}

/// Explicit source and shape for per-frame change overlays.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangeVisualization {
    /// Semantic scheduling or actual decoded adjacent pixels.
    pub source: ChangeSource,
    /// Visible overlay form.
    pub mode: ChangeMode,
    /// Number of output frames to retain a fading/held overlay.
    #[serde(default)]
    pub hold_frames: u16,
    /// Pixel-difference threshold for encoded input, ignored for semantic input.
    pub pixel_threshold: Option<u8>,
}

/// Truth source used by a change overlay.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangeSource {
    /// Nominally scheduled complete semantic-cell changes.
    SemanticCells,
    /// Actual decoded adjacent encoded-frame pixels.
    EncodedPixels,
}

/// Visible change-overlay mode.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangeMode {
    /// Outline changed cells or pixel regions.
    Outline,
    /// Shade rows reported dirty by semantic evidence.
    DirtyRows,
    /// Draw one minimal enclosing rectangle.
    BoundingBox,
    /// Color changes by frequency or magnitude.
    Heatmap,
    /// Fade prior change regions across held frames.
    Trail,
    /// Add a side-by-side difference panel.
    SideDiff,
}

/// One nominal semantic-change region in source pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChangeRegion {
    /// Left source pixel.
    pub x: u16,
    /// Top source pixel.
    pub y: u16,
    /// Region width.
    pub width: u16,
    /// Region height.
    pub height: u16,
}

/// Semantic delta nominally scheduled onto presentation frames.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NormalizedChangeFrame {
    /// Semantic state that introduced the delta.
    pub state_id: u64,
    /// Complete semantic state hash.
    pub state_hash: String,
    /// Nominal native frame selected by clock arithmetic.
    pub source_frame: u64,
    /// Output frames mapped to this source frame.
    pub output_frames: Vec<u64>,
    /// Nominal changed-cell pixel regions.
    pub changed_regions: Vec<ChangeRegion>,
    /// Rows reported dirty by the semantic engine.
    pub dirty_rows: Vec<u16>,
    /// Honest correlation level for this overlay input.
    pub evidence_level: String,
}

/// One semantic cursor region masked on a mapped presentation frame.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CursorMaskFrame {
    /// Presentation frame receiving the mask.
    pub output_frame: u64,
    /// Native source frame selected by presentation mapping.
    pub source_frame: u64,
    /// Latest presentation-eligible semantic state used for cursor geometry.
    pub state_id: u64,
    /// Complete source semantic state hash.
    pub state_hash: String,
    /// Nominal cursor pixel region.
    pub region: ChangeRegion,
    /// Honest semantic-to-native correlation level.
    pub evidence_level: String,
}

/// Canvas layout for caption presets.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentationLayout {
    /// Caption placement, with footer preserving terminal pixels.
    pub caption: CaptionLayout,
    /// Optional caller-owned branding label.
    pub branding: Option<String>,
}

impl Default for PresentationLayout {
    fn default() -> Self {
        return Self {
            caption: CaptionLayout::Footer,
            branding: None,
        };
    }
}

/// Placement of the per-clip caption preset.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CaptionLayout {
    /// Extend the canvas below the native viewport.
    #[default]
    Footer,
    /// Intentionally draw captions over source pixels.
    Overlay,
    /// Retain caption metadata without rendering it.
    None,
}

/// Delivery codec and pixel-format contract.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentationDelivery {
    /// Stable delivery codec name.
    pub codec: String,
    /// Stable output pixel format.
    pub pixel_format: String,
    /// libx264 constant-rate-factor quality.
    #[serde(default = "default_delivery_crf")]
    pub crf: u8,
    /// libx264 speed/compression preset.
    #[serde(default = "default_delivery_preset")]
    pub encoder_preset: String,
}

impl Default for PresentationDelivery {
    fn default() -> Self {
        return Self {
            codec: "h264".to_owned(),
            pixel_format: "yuv420p".to_owned(),
            crf: default_delivery_crf(),
            encoder_preset: default_delivery_preset(),
        };
    }
}

fn default_delivery_crf() -> u8 {
    return 23;
}

fn default_delivery_preset() -> String {
    return "medium".to_owned();
}

/// Explicit presentation-only pixel transform.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "transform", rename_all = "snake_case", deny_unknown_fields)]
pub enum PixelTransform {
    /// Replace cursor pixels during presentation composition.
    CursorMask,
    /// Crop to an exact source rectangle.
    Crop {
        /// Left source pixel.
        x: u16,
        /// Top source pixel.
        y: u16,
        /// Rectangle width.
        width: u16,
        /// Rectangle height.
        height: u16,
    },
    /// Scale by one exact rational factor.
    Zoom {
        /// Scale numerator.
        numerator: u16,
        /// Scale denominator.
        denominator: u16,
    },
    /// Resize to an exact presentation canvas.
    Resize {
        /// Output width in pixels.
        width: u16,
        /// Output height in pixels.
        height: u16,
    },
    /// Draw a visibly labeled rectangular callout.
    Callout {
        /// Left callout pixel.
        x: u16,
        /// Top callout pixel.
        y: u16,
        /// Callout width.
        width: u16,
        /// Callout height.
        height: u16,
        /// Optional visible label.
        label: Option<String>,
    },
}

/// Exact positive playback speed numerator/denominator.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RationalSpeed {
    /// Source-time multiplier numerator.
    pub numerator: u32,
    /// Source-time multiplier denominator.
    pub denominator: u32,
}

impl Default for RationalSpeed {
    fn default() -> Self {
        return Self {
            numerator: 1,
            denominator: 1,
        };
    }
}

/// Resolved clip and exact source/output frame decisions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NormalizedClip {
    /// Canonical half-open source interval.
    pub source_range: SourceRange,
    /// Inclusive source video frame.
    pub source_start_frame: u64,
    /// Exclusive source video frame.
    pub source_end_frame: u64,
    /// First output frame for this clip.
    pub output_start_frame: u64,
    /// Exclusive output frame after retiming and hold.
    pub output_end_frame: u64,
    /// Retimed source-derived frame count.
    pub retimed_frame_count: u64,
    /// Generated hold frame count.
    pub hold_frame_count: u64,
    /// Exact playback speed.
    pub speed: RationalSpeed,
    /// Optional caption retained as presentation content.
    pub caption: Option<String>,
}

/// One annotation resolved to a half-open output-frame interval.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NormalizedAnnotation {
    /// Stable caller annotation index.
    pub annotation_index: usize,
    /// Inclusive presentation frame.
    pub output_start_frame: u64,
    /// Exclusive presentation frame.
    pub output_end_frame: u64,
    /// Literal text, when the track is static.
    pub text: Option<String>,
    /// Per-frame dynamic text template.
    pub template: Option<String>,
    /// Exact anchored placement.
    pub position: AnnotationPosition,
    /// Visible text/background style.
    pub style: AnnotationStyle,
    /// Stable overlay ordering.
    pub z_order: i16,
}

/// Caller-selected visual source used while resolving a presentation plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PresentationSourceRequest {
    /// Use an existing verified native Ghostty capture.
    Native,
    /// Deterministically render canonical semantic states at the requested FPS.
    Semantic {
        /// Constant semantic source frame rate.
        fps: u16,
        /// Deterministic rasterization quality profile.
        raster_profile: SemanticRasterProfile,
    },
}

impl Default for PresentationSourceRequest {
    fn default() -> Self {
        return Self::Native;
    }
}

/// Immutable visual source selected for one presentation plan.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PresentationSource {
    /// Actual pixels from a verified native Ghostty capture.
    Native {
        /// Lossless native source video.
        video: PathBuf,
        /// BLAKE3 of the native source video.
        video_hash: String,
    },
    /// Deterministic generated pixels from canonical semantic replay.
    Semantic {
        /// Complete semantic source plan.
        plan: SemanticVideoSourcePlan,
    },
}

impl Default for PresentationSource {
    fn default() -> Self {
        return Self::Native {
            video: PathBuf::new(),
            video_hash: String::new(),
        };
    }
}

/// Dry-run output binding a normalized edit to immutable visual evidence.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentationPlan {
    /// Presentation format version.
    pub version: u32,
    /// BLAKE3 of canonical input edit-plan JSON.
    pub edit_plan_hash: String,
    /// Explicit immutable visual source; native remains the default request.
    pub source: PresentationSource,
    /// Native capture or generated semantic source frame rate.
    pub fps: u16,
    /// Visual source width in pixels.
    pub source_width: u16,
    /// Visual source height in pixels.
    pub source_height: u16,
    /// Measured native or deterministic semantic grid and cell geometry.
    pub source_geometry: TerminalGeometryReport,
    /// Final presentation canvas width after side panels and transforms.
    pub output_width: u16,
    /// Final presentation canvas height after footer and transforms.
    pub output_height: u16,
    /// Effective user-facing render settings retained from the facade.
    pub render_config: Option<VideoRenderConfig>,
    /// Caller-ordered resolved clips.
    pub clips: Vec<NormalizedClip>,
    /// Canonical markers mapped onto native source frames.
    pub markers: Vec<MarkerRecord>,
    /// Annotation tracks normalized onto the output frame timeline.
    pub annotations: Vec<NormalizedAnnotation>,
    /// Validated caller-authored subtitle resources.
    pub subtitles: Vec<NormalizedSubtitle>,
    /// Explicit per-frame change overlay policy.
    pub change_visualization: Option<ChangeVisualization>,
    /// Semantic deltas scheduled onto mapped presentation frames.
    pub change_frames: Vec<NormalizedChangeFrame>,
    /// Stable change-overlay implementation and truth description.
    pub change_algorithm: Option<String>,
    /// Cursor regions nominally masked from presentation pixels.
    pub cursor_masks: Vec<CursorMaskFrame>,
    /// Presentation canvas and caption policy.
    pub layout: PresentationLayout,
    /// Delivery encoding contract.
    pub delivery: PresentationDelivery,
    /// Ordered visibly labeled pixel transforms.
    pub transforms: Vec<PixelTransform>,
    /// Total output frames after retiming and holds.
    pub output_frame_count: u64,
}

/// One output-frame provenance decision.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentationMapEntry {
    /// Zero-based presentation output frame.
    pub output_frame: u64,
    /// Caller-ordered clip index.
    pub clip_index: usize,
    /// Base source frame used for this output.
    pub source_frame: u64,
    /// Exact semantic source state, present only for semantic-rendered input.
    pub source_state_id: Option<u64>,
    /// Exact semantic source state hash, present only for semantic-rendered input.
    pub source_state_hash: Option<String>,
    /// Exact semantic source timestamp, present only for semantic-rendered input.
    pub source_state_monotonic_ns: Option<u64>,
    /// `source` for retimed frames or `hold` for generated duplicates.
    pub operation: String,
    /// Whether a caller caption alters this presentation frame.
    pub captioned: bool,
    /// Stable annotation indices rendered on this frame.
    pub annotation_indices: Vec<usize>,
    /// Presentation-only transforms applied to this frame.
    pub transforms: Vec<String>,
    /// Semantic states nominally scheduled into a change overlay on this frame.
    pub change_state_ids: Vec<u64>,
    /// Whether this frame uses actual adjacent decoded encoded-pixel comparison.
    pub encoded_pixel_change: bool,
    /// Semantic state used when a cursor mask affected this frame.
    pub cursor_mask_state_id: Option<u64>,
}

/// Integrity and tool provenance for an edited presentation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentationManifest {
    /// Presentation format version.
    pub version: u32,
    /// Exact resolved plan.
    pub plan: PresentationPlan,
    /// First line of the exact FFmpeg build used.
    pub ffmpeg_version: String,
    /// Exact direct-process FFmpeg argument vector.
    pub ffmpeg_arguments: Vec<String>,
    /// Parsed FFprobe delivery-video evidence.
    pub output_probe: serde_json::Value,
    /// Product truth layer, distinct from immutable native evidence.
    pub truth_layer: String,
    /// Truth layer used as presentation input.
    pub source_truth_layer: String,
    /// BLAKE3 of the encoded delivery video.
    pub output_video_hash: String,
    /// Whole-file integrity metadata for every non-manifest member.
    pub members: Vec<ArtifactMember>,
}

/// Atomically published edited presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationOutcome {
    /// No-replace presentation artifact directory.
    pub directory: PathBuf,
    /// Edited H.264 MP4 delivery video.
    pub video: PathBuf,
}

/// Bounded storyboard and low-resolution preview policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewOptions {
    /// Storyboard tile columns.
    pub columns: u8,
    /// Width of each storyboard tile.
    pub tile_width: u16,
    /// Maximum selected storyboard frames.
    pub max_frames: u16,
    /// Select only frames carrying the configured change visualization.
    pub change_only: bool,
    /// Low-resolution preview width.
    pub preview_width: u16,
}

impl Default for PreviewOptions {
    fn default() -> Self {
        return Self {
            columns: 4,
            tile_width: 320,
            max_frames: 12,
            change_only: false,
            preview_width: 640,
        };
    }
}

/// Integrity and selection provenance for one preview artifact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PresentationPreviewManifest {
    /// Preview format version.
    pub version: u32,
    /// Source edit identity.
    pub edit_plan_hash: String,
    /// Immutable native-video or canonical semantic-source identity.
    pub source_identity_hash: String,
    /// Presentation product truth layer.
    pub truth_layer: String,
    /// Applied preview policy.
    pub options: PreviewOptions,
    /// Exact selected presentation frames.
    pub selected_frames: Vec<u64>,
    /// First line of the exact FFmpeg build used.
    pub ffmpeg_version: String,
    /// Whole-file integrity metadata for every non-manifest member.
    pub members: Vec<ArtifactMember>,
}

/// Atomically published storyboard and low-resolution video.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PresentationPreviewOutcome {
    /// No-replace preview artifact directory.
    pub directory: PathBuf,
    /// Contact-sheet PNG.
    pub contact_sheet: PathBuf,
    /// Low-resolution H.264 MP4.
    pub video: PathBuf,
}

/// Canonical marker with duplicate and nominal-frame context.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MarkerRecord {
    /// Marker label.
    pub label: String,
    /// Canonical event sequence.
    pub event_seq: u64,
    /// Canonical session offset.
    pub monotonic_ns: u64,
    /// Nominal source frame at the requested rate.
    pub nominal_frame: u64,
    /// Total occurrences of this label.
    pub occurrence_count: u64,
    /// One-based position of this marker among occurrences of the same label.
    pub occurrence_index: u64,
    /// Latest presentation-eligible semantic state at or before the marker.
    pub semantic_state_id: Option<u64>,
    /// Complete hash of the associated semantic state.
    pub semantic_state_hash: Option<String>,
    /// Whether an associated presentation-eligible semantic state exists.
    pub presentation_eligible: bool,
    /// Honest native correlation level for the nominal frame.
    pub native_evidence_level: String,
}
