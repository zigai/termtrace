//! Marker-resolved presentation edit plans over native or semantic sources.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fmt::Write as _,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str, thread,
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    artifact::{
        ArtifactMember, collect_artifact_members, commit_directory, create_private_directory,
        finish_writer, hash_regular_file, secure_file, secure_writer, verify_artifact_members,
        write_secure,
    },
    event::Event,
    event_log::EventLog,
    geometry::TerminalGeometryReport,
    ghostty_vt::{CursorState, CursorStyle},
    replay::{StateRecord, StateSelection, replay},
    semantic_image::SEMANTIC_FONT_BYTES,
    semantic_video::{
        SemanticFrameSpan, SemanticVideoSourcePlan, materialize as materialize_semantic_video,
        plan as plan_semantic_video,
    },
    session::validate_completed_log,
    timeline::{SourceRange, resolve_source_selection},
    visual::{VisualManifest, visual_directory_name_for},
};

/// Current edit-plan format version.
pub const EDIT_PLAN_VERSION: u32 = 1;
/// Current normalized presentation plan and manifest format version.
pub const PRESENTATION_FORMAT_VERSION: u32 = 2;
const NANOS_PER_SECOND: u64 = 1_000_000_000;

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
}

impl EditPlan {
    /// Parse and validate one strict edit-plan document.
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let bytes = fs_err::read(path).map_err(|source| {
            return Error::Filesystem {
                path: path.to_path_buf(),
                source,
            };
        })?;
        let mut plan: Self = serde_json::from_slice(&bytes).map_err(|source| {
            return Error::InvalidJson {
                path: path.to_path_buf(),
                json_path: "$".to_owned(),
                source,
            };
        })?;
        let base = path.parent().unwrap_or_else(|| return Path::new("."));
        for subtitle in &mut plan.subtitles {
            if subtitle.ass.is_relative() {
                subtitle.ass = base.join(&subtitle.ass);
            }
        }
        plan.validate()?;
        return Ok(plan);
    }

    fn validate(&self) -> Result<()> {
        if self.version != EDIT_PLAN_VERSION || self.clips.is_empty() {
            return Err(plan_error(
                "edit plan must use version 1 and contain at least one clip",
            ));
        }
        for clip in &self.clips {
            clip.from.validate()?;
            clip.to.validate()?;
            if clip.speed.numerator == 0 || clip.speed.denominator == 0 {
                return Err(plan_error(
                    "clip speed numerator and denominator must be nonzero",
                ));
            }
            if u64::from(clip.speed.numerator) > u64::from(clip.speed.denominator) * 64
                || u64::from(clip.speed.denominator) > u64::from(clip.speed.numerator) * 64
            {
                return Err(plan_error("clip speed must be between 1/64 and 64"));
            }
            if clip.caption.as_ref().is_some_and(|caption| {
                return caption.is_empty()
                    || caption.chars().count() > 4096
                    || caption.contains('\0');
            }) {
                return Err(plan_error(
                    "clip captions must contain 1-4096 non-NUL characters",
                ));
            }
        }
        for annotation in &self.annotations {
            annotation.validate(self.clips.len())?;
        }
        for subtitle in &self.subtitles {
            if subtitle.ass.as_os_str().is_empty() {
                return Err(plan_error("ASS subtitle path must not be empty"));
            }
            validate_ass_resource(&subtitle.ass)?;
        }
        for transform in &self.transforms {
            transform.validate()?;
        }
        if let Some(change) = &self.change_visualization {
            change.validate()?;
        }
        if self.delivery != PresentationDelivery::default() {
            return Err(plan_error(
                "delivery currently requires codec `h264` and pixel format `yuv420p`",
            ));
        }
        if self.layout.branding.as_ref().is_some_and(|branding| {
            return branding.is_empty()
                || branding.chars().count() > 256
                || branding.contains('\0');
        }) {
            return Err(plan_error("branding must contain 1-256 non-NUL characters"));
        }
        return Ok(());
    }
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

impl EditSelector {
    fn validate(&self) -> Result<()> {
        let count = [
            self.marker.is_some(),
            self.monotonic_ns.is_some(),
            self.event_seq.is_some(),
            self.state_id.is_some(),
            self.native_frame.is_some(),
        ]
        .into_iter()
        .filter(|selected| return *selected)
        .count();
        if count != 1 {
            return Err(plan_error(
                "edit selector requires exactly one marker, monotonicNs, eventSeq, stateId, or nativeFrame",
            ));
        }
        if self.marker.as_deref().is_some_and(str::is_empty) {
            return Err(plan_error("edit marker must not be empty"));
        }
        return Ok(());
    }
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

impl TextAnnotation {
    fn validate(&self, clip_count: usize) -> Result<()> {
        if self.text.is_some() == self.template.is_some() {
            return Err(plan_error(
                "annotation requires exactly one of text or template",
            ));
        }
        let value = self
            .text
            .as_deref()
            .or(self.template.as_deref())
            .unwrap_or("");
        if value.is_empty() || value.chars().count() > 4096 || value.contains('\0') {
            return Err(plan_error(
                "annotation text/template must contain 1-4096 non-NUL characters",
            ));
        }
        if let Some(template) = &self.template {
            validate_annotation_template(template)?;
        }
        if let AnnotationScope::Clip { clip_index } = self.scope
            && clip_index >= clip_count
        {
            return Err(plan_error("annotation clip index is out of bounds"));
        }
        if let AnnotationScope::Range { from, to } = &self.scope {
            from.validate()?;
            to.validate()?;
        }
        if self.position.x.unsigned_abs() > 32_768 || self.position.y.unsigned_abs() > 32_768 {
            return Err(plan_error(
                "annotation position offsets must be within 32768 pixels",
            ));
        }
        self.style.validate()?;
        return Ok(());
    }
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

impl AnnotationStyle {
    fn validate(&self) -> Result<()> {
        if !(8..=128).contains(&self.font_size)
            || !valid_rgb(&self.foreground)
            || !valid_rgb(&self.background)
            || self.border > 16
        {
            return Err(plan_error("annotation style is outside bounded values"));
        }
        return Ok(());
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

impl ChangeVisualization {
    fn validate(&self) -> Result<()> {
        if self.source == ChangeSource::SemanticCells && self.pixel_threshold.is_some() {
            return Err(plan_error(
                "pixel threshold is valid only for encoded-pixel change visualization",
            ));
        }
        if self.source == ChangeSource::EncodedPixels && self.mode == ChangeMode::DirtyRows {
            return Err(plan_error(
                "dirty-row change visualization requires semantic-cell evidence",
            ));
        }
        if self.source == ChangeSource::EncodedPixels && self.mode == ChangeMode::BoundingBox {
            return Err(plan_error(
                "encoded-pixel bounding boxes are unavailable; use outline, heatmap, trail, or side_diff",
            ));
        }
        if self.hold_frames > 240 {
            return Err(plan_error("change overlay hold must not exceed 240 frames"));
        }
        return Ok(());
    }
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
}

impl Default for PresentationDelivery {
    fn default() -> Self {
        return Self {
            codec: "h264".to_owned(),
            pixel_format: "yuv420p".to_owned(),
        };
    }
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

impl PixelTransform {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Crop { width, height, .. } | Self::Callout { width, height, .. }
                if *width == 0 || *height == 0 =>
            {
                return Err(plan_error("crop/callout dimensions must be nonzero"));
            }
            Self::Zoom {
                numerator,
                denominator,
            } if *numerator == 0
                || *denominator == 0
                || u32::from(*numerator) > u32::from(*denominator) * 16
                || u32::from(*denominator) > u32::from(*numerator) * 16 =>
            {
                return Err(plan_error(
                    "zoom ratio must be positive and between 1/16 and 16",
                ));
            }
            Self::Callout {
                label: Some(label), ..
            } if label.is_empty() || label.chars().count() > 256 || label.contains('\0') => {
                return Err(plan_error(
                    "callout labels must contain 1-256 non-NUL characters",
                ));
            }
            _ => {}
        }
        return Ok(());
    }
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

impl PresentationSource {
    fn identity_hash(&self) -> Result<String> {
        return match self {
            Self::Native { video_hash, .. } => Ok(video_hash.clone()),
            Self::Semantic { plan } => {
                let identity = serde_json::json!({
                    "version": plan.version,
                    "session_id": plan.session_id,
                    "source_event_hash": &plan.source_event_hash,
                    "start_monotonic_ns": plan.start_monotonic_ns,
                    "end_monotonic_ns": plan.end_monotonic_ns,
                    "fps": plan.fps,
                    "frame_count": plan.frame_count,
                    "canvas": &plan.canvas,
                    "renderer": &plan.renderer,
                    "font_family": &plan.font_family,
                    "font_hash": &plan.font_hash,
                    "scheduling_policy": &plan.scheduling_policy,
                });
                let bytes = serde_json::to_vec(&identity).map_err(|source| {
                    return Error::InvalidJson {
                        path: PathBuf::from("<semantic-source-plan>"),
                        json_path: "$".to_owned(),
                        source,
                    };
                })?;
                Ok(blake3::hash(&bytes).to_hex().to_string())
            }
        };
    }

    fn truth_layer(&self) -> &'static str {
        return match self {
            Self::Native { .. } => "native_encoded_evidence",
            Self::Semantic { .. } => "semantic_rendering",
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

impl PreviewOptions {
    fn validate(self) -> Result<()> {
        if !(1..=10).contains(&self.columns)
            || !(80..=1920).contains(&self.tile_width)
            || !(1..=100).contains(&self.max_frames)
            || !(160..=1920).contains(&self.preview_width)
            || !self.preview_width.is_multiple_of(2)
        {
            return Err(plan_error(
                "preview dimensions or frame count are out of bounds",
            ));
        }
        return Ok(());
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
    /// Latest presentation-eligible semantic state at or before the marker.
    pub semantic_state_id: Option<u64>,
    /// Complete hash of the associated semantic state.
    pub semantic_state_hash: Option<String>,
    /// Whether an associated presentation-eligible semantic state exists.
    pub presentation_eligible: bool,
    /// Honest native correlation level for the nominal frame.
    pub native_evidence_level: String,
}

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
            let count = occurrence_counts.entry(label.as_str()).or_insert(0_u64);
            *count = count.saturating_add(1);
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
        let occurrence_count = occurrence_counts.get(label.as_str()).copied().unwrap_or(0);
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
    edit.validate()?;
    let session = session.as_ref();
    let log = EventLog::read(session.join("events.jsonl"))?;
    validate_completed_log(session, &log)?;
    let states = replay(&log)?.states;
    let (
        source,
        fps,
        source_width,
        source_height,
        source_geometry,
        capture_start,
        capture_range,
        semantic_final_frame,
    ) = match source_request {
        PresentationSourceRequest::Native => {
            let visual = session.join(visual_directory_name_for(None, StateSelection::All));
            let manifest_path = visual.join("manifest.json");
            let manifest_bytes = fs_err::read(&manifest_path).map_err(|source| {
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
            let capture_start = manifest
                .source_range
                .map_or(0, |capture| return capture.start_monotonic_ns);
            (
                PresentationSource::Native {
                    video: source_video,
                    video_hash: source_video_hash,
                },
                manifest.fps,
                manifest.width,
                manifest.height,
                manifest.geometry_report,
                capture_start,
                manifest.source_range,
                None,
            )
        }
        PresentationSourceRequest::Semantic { fps } => {
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
                let range = resolve_clip(&log, &states, clip, 0, fps)?;
                semantic_start = semantic_start.min(range.start_monotonic_ns);
                semantic_end = semantic_end.max(range.end_monotonic_ns);
            }
            let semantic = plan_semantic_video(session, &log, fps, semantic_start, semantic_end)?;
            let geometry = TerminalGeometryReport {
                cols: semantic.canvas.cols,
                rows: semantic.canvas.rows,
                cell_width: semantic.canvas.cell_width,
                cell_height: semantic.canvas.cell_height,
                text_width: semantic.canvas.width,
                text_height: semantic.canvas.height,
            };
            let final_frame = (semantic.end_monotonic_ns, semantic.frame_count);
            (
                PresentationSource::Semantic {
                    plan: semantic.clone(),
                },
                semantic.fps,
                semantic.canvas.width,
                semantic.canvas.height,
                geometry,
                semantic.start_monotonic_ns,
                None,
                Some(final_frame),
            )
        }
    };
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
    let edit_plan_hash = blake3::hash(&serde_json::to_vec(&edit_identity).map_err(|source| {
        return Error::InvalidJson {
            path: PathBuf::from("<edit-plan>"),
            json_path: "$".to_owned(),
            source,
        };
    })?)
    .to_hex()
    .to_string();
    let mut output_start_frame = 0_u64;
    let mut clips = Vec::new();
    for clip in &edit.clips {
        let range = resolve_clip(&log, &states, clip, capture_start, fps)?;
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
        let source_start_frame = frame_floor(range.start_monotonic_ns - capture_start, fps);
        let source_end_frame = semantic_final_frame.map_or_else(
            || return frame_ceil(range.end_monotonic_ns - capture_start, fps),
            |(semantic_end, final_frame)| {
                return if range.end_monotonic_ns == semantic_end {
                    final_frame
                } else {
                    frame_ceil(range.end_monotonic_ns - capture_start, fps)
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
    let annotations = normalize_annotations(
        &log,
        &states,
        edit,
        &clips,
        output_start_frame,
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
        output_frame_count: output_start_frame,
    });
}

fn validate_planned_source(source: &PresentationSource) -> Result<()> {
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

fn materialize_planned_source(plan: &PresentationPlan, staging: &Path) -> Result<PathBuf> {
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

/// Export a normalized plan without modifying its immutable source evidence.
pub fn export(
    plan: &PresentationPlan,
    destination: impl AsRef<Path>,
) -> Result<PresentationOutcome> {
    if plan.clips.is_empty() || plan.output_frame_count == 0 {
        return Err(plan_error("presentation plan has no output frames"));
    }
    validate_planned_source(&plan.source)?;
    let destination = destination.as_ref();
    if destination.exists() {
        return Err(Error::ArtifactExists {
            path: destination.to_path_buf(),
        });
    }
    let parent = destination
        .parent()
        .ok_or_else(|| return plan_error("presentation destination must have a parent"))?;
    fs_err::create_dir_all(parent).map_err(|source| {
        return Error::Filesystem {
            path: parent.to_path_buf(),
            source,
        };
    })?;
    let staging = parent.join(format!(".presentation-{}.tmp", uuid::Uuid::now_v7()));
    create_private_directory(&staging)?;
    export_staged(plan, &staging)?;
    commit_directory(parent, &staging, destination)?;
    return Ok(PresentationOutcome {
        directory: destination.to_path_buf(),
        video: destination.join("presentation.mp4"),
    });
}

/// Render an atomic contact sheet and low-resolution preview of a normalized edit.
pub fn preview(
    plan: &PresentationPlan,
    destination: impl AsRef<Path>,
    options: PreviewOptions,
) -> Result<PresentationPreviewOutcome> {
    options.validate()?;
    if options.change_only && plan.change_visualization.is_none() {
        return Err(plan_error(
            "change-only preview requires change_visualization in the edit plan",
        ));
    }
    validate_planned_source(&plan.source)?;
    let destination = destination.as_ref();
    if destination.exists() {
        return Err(Error::ArtifactExists {
            path: destination.to_path_buf(),
        });
    }
    let parent = destination
        .parent()
        .ok_or_else(|| return plan_error("preview destination must have a parent"))?;
    fs_err::create_dir_all(parent).map_err(|source| {
        return Error::Filesystem {
            path: parent.to_path_buf(),
            source,
        };
    })?;
    let composed = parent.join(format!(".preview-source-{}.tmp", uuid::Uuid::now_v7()));
    let _composed_cleanup = DirectoryCleanup(composed.clone());
    let rendered = export(plan, &composed)?;
    let staging = parent.join(format!(".preview-{}.tmp", uuid::Uuid::now_v7()));
    create_private_directory(&staging)?;
    let selected_frames = select_preview_frames(plan, options)?;
    write_secure(
        &staging.join("selected-frames.json"),
        &serde_json::to_vec_pretty(&selected_frames).map_err(|source| {
            return Error::InvalidJson {
                path: staging.join("selected-frames.json"),
                json_path: "$".to_owned(),
                source,
            };
        })?,
    )?;
    let normalized = serde_json::to_vec_pretty(plan).map_err(|source| {
        return Error::InvalidJson {
            path: staging.join("normalized-plan.json"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    write_secure(&staging.join("normalized-plan.json"), &normalized)?;
    let source_manifest =
        fs_err::read(rendered.directory.join("manifest.json")).map_err(|source| {
            return Error::Filesystem {
                path: rendered.directory.join("manifest.json"),
                source,
            };
        })?;
    write_secure(
        &staging.join("source-presentation-manifest.json"),
        &source_manifest,
    )?;
    thread::scope(|scope| {
        let contact_sheet = scope.spawn(|| {
            return render_contact_sheet(&rendered.video, &staging, &selected_frames, options);
        });
        let preview_video = scope.spawn(|| {
            return render_low_resolution_preview(&rendered.video, &staging, options.preview_width);
        });
        contact_sheet
            .join()
            .map_err(|_panic| return plan_error("contact-sheet rendering worker panicked"))??;
        preview_video
            .join()
            .map_err(|_panic| return plan_error("preview-video rendering worker panicked"))??;
        return Ok(());
    })?;
    let members = collect_artifact_members(&staging)?;
    let manifest = PresentationPreviewManifest {
        version: PRESENTATION_FORMAT_VERSION,
        edit_plan_hash: plan.edit_plan_hash.clone(),
        source_identity_hash: plan.source.identity_hash()?,
        truth_layer: "presentation_preview".to_owned(),
        options,
        selected_frames,
        ffmpeg_version: command_version("ffmpeg")?,
        members,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|source| {
        return Error::InvalidJson {
            path: staging.join("manifest.json"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    write_secure(&staging.join("manifest.json"), &manifest_bytes)?;
    verify_artifact_members(&staging, &manifest.members)?;
    commit_directory(parent, &staging, destination)?;
    return Ok(PresentationPreviewOutcome {
        directory: destination.to_path_buf(),
        contact_sheet: destination.join("contact-sheet.png"),
        video: destination.join("preview.mp4"),
    });
}

struct DirectoryCleanup(PathBuf);

impl Drop for DirectoryCleanup {
    fn drop(&mut self) {
        let _result = fs_err::remove_dir_all(&self.0);
    }
}

fn select_preview_frames(plan: &PresentationPlan, options: PreviewOptions) -> Result<Vec<u64>> {
    let candidates = if options.change_only {
        let mut frames = BTreeSet::new();
        match plan
            .change_visualization
            .as_ref()
            .map(|change| return change.source)
        {
            Some(ChangeSource::SemanticCells) => {
                let hold = plan.change_visualization.as_ref().map_or(0, |change| {
                    return u64::from(change.hold_frames);
                });
                for change in &plan.change_frames {
                    for frame in &change.output_frames {
                        for held in *frame
                            ..=frame
                                .saturating_add(hold)
                                .min(plan.output_frame_count.saturating_sub(1))
                        {
                            frames.insert(held);
                        }
                    }
                }
            }
            Some(ChangeSource::EncodedPixels) => {
                frames.extend(1..plan.output_frame_count);
            }
            None => {}
        }
        frames.into_iter().collect::<Vec<_>>()
    } else {
        (0..plan.output_frame_count).collect::<Vec<_>>()
    };
    if candidates.is_empty() {
        return Err(plan_error("preview selection contains no frames"));
    }
    let count = usize::from(options.max_frames).min(candidates.len());
    if count == 1 {
        return candidates
            .first()
            .copied()
            .map(|frame| return vec![frame])
            .ok_or_else(|| return plan_error("preview selection contains no frames"));
    }
    let mut selected = Vec::with_capacity(count);
    for index in 0..count {
        let candidate_index =
            index.saturating_mul(candidates.len().saturating_sub(1)) / count.saturating_sub(1);
        if let Some(frame) = candidates.get(candidate_index) {
            selected.push(*frame);
        }
    }
    selected.dedup();
    return Ok(selected);
}

fn render_contact_sheet(
    source: &Path,
    staging: &Path,
    selected_frames: &[u64],
    options: PreviewOptions,
) -> Result<()> {
    let selection = selected_frames
        .iter()
        .map(|frame| return format!("eq(n\\,{frame})"))
        .collect::<Vec<_>>()
        .join("+");
    let rows = selected_frames.len().div_ceil(usize::from(options.columns));
    let filter = format!(
        "select={selection},scale={}:-2:flags=lanczos,tile={}x{}:nb_frames={}:padding=4:margin=4:color=black",
        options.tile_width,
        options.columns,
        rows,
        selected_frames.len(),
    );
    let process = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-y", "-i"])
        .arg(source)
        .args(["-vf", &filter, "-frames:v", "1", "contact-sheet.png"])
        .current_dir(staging)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|source| {
            return Error::Filesystem {
                path: PathBuf::from("ffmpeg"),
                source,
            };
        })?;
    write_secure(&staging.join("contact-sheet.log"), &process.stderr)?;
    if !process.status.success() {
        return Err(plan_error(format!(
            "FFmpeg contact-sheet rendering failed with {}",
            process.status
        )));
    }
    return Ok(());
}

fn render_low_resolution_preview(source: &Path, staging: &Path, width: u16) -> Result<()> {
    let filter = format!("scale=w='min(iw,{width})':h=-2:flags=lanczos");
    let process = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-y", "-i"])
        .arg(source)
        .args([
            "-vf",
            &filter,
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "28",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "preview.mp4",
        ])
        .current_dir(staging)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|source| {
            return Error::Filesystem {
                path: PathBuf::from("ffmpeg"),
                source,
            };
        })?;
    write_secure(&staging.join("preview.log"), &process.stderr)?;
    if !process.status.success() {
        return Err(plan_error(format!(
            "FFmpeg low-resolution preview failed with {}",
            process.status
        )));
    }
    return Ok(());
}

fn export_staged(plan: &PresentationPlan, staging: &Path) -> Result<PresentationManifest> {
    let source_video = materialize_planned_source(plan, staging)?;
    let filter_path = staging.join("filter.txt");
    let mut filters = Vec::new();
    let captioned = plan.layout.caption != CaptionLayout::None
        && plan.clips.iter().any(|clip| return clip.caption.is_some());
    for (index, clip) in plan.clips.iter().enumerate() {
        let mut filter = format!(
            "[0:v]trim=start_frame={}:end_frame={},setpts=(PTS-STARTPTS)*{}/{}",
            clip.source_start_frame,
            clip.source_end_frame,
            clip.speed.denominator,
            clip.speed.numerator,
        );
        if clip.hold_frame_count > 0 {
            write!(
                &mut filter,
                ",tpad=stop_mode=clone:stop_duration={}.{:09}",
                clip.hold_frame_count / u64::from(plan.fps),
                clip.hold_frame_count % u64::from(plan.fps) * NANOS_PER_SECOND
                    / u64::from(plan.fps),
            )
            .map_err(|_error| return plan_error("could not build FFmpeg hold filter"))?;
        }
        if captioned && plan.layout.caption == CaptionLayout::Footer {
            filter.push_str(",pad=iw:ih+48:0:0:color=black");
        }
        if captioned && let Some(caption) = &clip.caption {
            let caption_name = format!("caption-{index}.txt");
            let caption_path = staging.join(&caption_name);
            write_secure(&caption_path, caption.as_bytes())?;
            write!(
                &mut filter,
                ",drawtext=textfile='{}':fontcolor=white:fontsize=20:box=1:boxcolor=black@0.65:x=16:y=h-34",
                filter_path_value(Path::new(&caption_name))?,
            )
            .map_err(|_error| return plan_error("could not build FFmpeg caption filter"))?;
        }
        write!(&mut filter, "[v{index}]")
            .map_err(|_error| return plan_error("could not build FFmpeg clip filter"))?;
        filters.push(filter);
    }
    let inputs = (0..plan.clips.len())
        .map(|index| return format!("[v{index}]"))
        .collect::<String>();
    filters.push(format!(
        "{inputs}concat=n={}:v=1:a=0[joined]",
        plan.clips.len()
    ));
    let subtitle_resources = prepare_subtitle_resources(plan, staging)?;
    let changed = append_change_visualization(&mut filters, plan, "joined")?;
    let mut prior = append_pixel_transforms(&mut filters, plan, staging, &changed)?;
    for (index, resource) in subtitle_resources.iter().enumerate() {
        let next = format!("subtitle{index}");
        filters.push(format!(
            "[{}]ass=filename='{}':fontsdir='{}'[{}]",
            prior,
            filter_path_value(resource.file_name().map(Path::new).ok_or_else(|| {
                return plan_error("subtitle resource has no filename");
            })?)?,
            filter_path_value(Path::new("fonts"))?,
            next,
        ));
        prior = next;
    }
    filters.push(format!("[{prior}]null[outv]"));
    let filter = format!("{};\n", filters.join(";\n"));
    write_secure(&filter_path, filter.as_bytes())?;
    if plan.change_visualization.as_ref().is_some_and(|change| {
        return change.source == ChangeSource::EncodedPixels;
    }) {
        write_source_frame_hashes(plan, &source_video, staging)?;
    }
    let ffmpeg_arguments = vec![
        "-hide_banner".to_owned(),
        "-nostdin".to_owned(),
        "-y".to_owned(),
        "-i".to_owned(),
        source_video.display().to_string(),
        "-filter_complex_script".to_owned(),
        "filter.txt".to_owned(),
        "-map".to_owned(),
        "[outv]".to_owned(),
        "-an".to_owned(),
        "-r".to_owned(),
        plan.fps.to_string(),
        "-frames:v".to_owned(),
        plan.output_frame_count.to_string(),
        "-c:v".to_owned(),
        "libx264".to_owned(),
        "-pix_fmt".to_owned(),
        "yuv420p".to_owned(),
        "-movflags".to_owned(),
        "+faststart".to_owned(),
        "presentation.mp4".to_owned(),
    ];
    let process = Command::new("ffmpeg")
        .args(&ffmpeg_arguments)
        .current_dir(staging)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|source| {
            return Error::Filesystem {
                path: PathBuf::from("ffmpeg"),
                source,
            };
        })?;
    write_secure(&staging.join("ffmpeg.log"), &process.stderr)?;
    if !process.status.success() {
        return Err(plan_error(format!(
            "FFmpeg presentation export failed with {}",
            process.status
        )));
    }
    let (output_probe, ffmpeg_version) = thread::scope(|scope| {
        let probe = scope.spawn(|| -> Result<serde_json::Value> {
            let output_probe = probe_delivery_video(staging)?;
            write_secure(
                &staging.join("output-probe.json"),
                &serde_json::to_vec_pretty(&output_probe).map_err(|source| {
                    return Error::InvalidJson {
                        path: staging.join("output-probe.json"),
                        json_path: "$".to_owned(),
                        source,
                    };
                })?,
            )?;
            return Ok(output_probe);
        });
        let frame_map = scope.spawn(|| -> Result<()> {
            let semantic_spans = if matches!(plan.source, PresentationSource::Semantic { .. }) {
                read_semantic_frame_spans(&staging.join("semantic-source-map.jsonl"))?
            } else {
                Vec::new()
            };
            return write_presentation_map(
                &staging.join("presentation-map.jsonl"),
                plan,
                &semantic_spans,
            );
        });
        let normalized_plan = scope.spawn(|| -> Result<()> {
            let normalized = serde_json::to_vec_pretty(plan).map_err(|source| {
                return Error::InvalidJson {
                    path: staging.join("normalized-plan.json"),
                    json_path: "$".to_owned(),
                    source,
                };
            })?;
            return write_secure(&staging.join("normalized-plan.json"), &normalized);
        });
        let version = scope.spawn(|| return command_version("ffmpeg"));
        let output_probe = probe
            .join()
            .map_err(|_panic| return plan_error("output-probe worker panicked"))??;
        frame_map
            .join()
            .map_err(|_panic| return plan_error("presentation-map worker panicked"))??;
        normalized_plan
            .join()
            .map_err(|_panic| return plan_error("normalized-plan worker panicked"))??;
        let ffmpeg_version = version
            .join()
            .map_err(|_panic| return plan_error("FFmpeg-version worker panicked"))??;
        return Ok((output_probe, ffmpeg_version));
    })?;
    let members = collect_artifact_members(staging)?;
    let output_video_hash = members
        .iter()
        .find(|member| return member.path == "presentation.mp4")
        .map(|member| return member.blake3.clone())
        .ok_or_else(|| return plan_error("presentation member list has no output video"))?;
    let manifest = PresentationManifest {
        version: PRESENTATION_FORMAT_VERSION,
        plan: plan.clone(),
        ffmpeg_version,
        ffmpeg_arguments,
        output_probe,
        truth_layer: "presentation_output".to_owned(),
        source_truth_layer: plan.source.truth_layer().to_owned(),
        output_video_hash,
        members,
    };
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|source| {
        return Error::InvalidJson {
            path: staging.join("manifest.json"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    write_secure(&staging.join("manifest.json"), &manifest_bytes)?;
    verify_artifact_members(staging, &manifest.members)?;
    return Ok(manifest);
}

fn write_presentation_map(
    path: &Path,
    plan: &PresentationPlan,
    semantic_spans: &[SemanticFrameSpan],
) -> Result<()> {
    let mut writer = secure_writer(path)?;
    for (clip_index, clip) in plan.clips.iter().enumerate() {
        for output_frame in clip.output_start_frame..clip.output_end_frame {
            let (source_frame, operation) = mapped_source_frame(clip, output_frame);
            let semantic_state = semantic_spans.iter().find(|span| {
                return span.start_frame <= source_frame && source_frame < span.end_frame;
            });
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
                transforms: frame_transform_names(plan, output_frame),
                change_state_ids: semantic_change_states_at(plan, output_frame),
                encoded_pixel_change: plan.change_visualization.as_ref().is_some_and(|change| {
                    return change.source == ChangeSource::EncodedPixels && output_frame > 0;
                }),
                cursor_mask_state_id: plan
                    .cursor_masks
                    .iter()
                    .find(|mask| return mask.output_frame == output_frame)
                    .map(|mask| return mask.state_id),
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
    return finish_writer(writer, path);
}

fn read_semantic_frame_spans(path: &Path) -> Result<Vec<SemanticFrameSpan>> {
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

fn semantic_change_states_at(plan: &PresentationPlan, output_frame: u64) -> Vec<u64> {
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

#[derive(Clone, Copy)]
struct ScheduledBox {
    region: ChangeRegion,
    start_frame: u64,
    end_frame: u64,
}

fn append_change_visualization(
    filters: &mut Vec<String>,
    plan: &PresentationPlan,
    input: &str,
) -> Result<String> {
    let Some(change) = &plan.change_visualization else {
        return Ok(input.to_owned());
    };
    return match change.source {
        ChangeSource::SemanticCells => {
            append_semantic_change_visualization(filters, plan, change, input)
        }
        ChangeSource::EncodedPixels => append_encoded_change_visualization(filters, change, input),
    };
}

fn append_semantic_change_visualization(
    filters: &mut Vec<String>,
    plan: &PresentationPlan,
    change: &ChangeVisualization,
    input: &str,
) -> Result<String> {
    let boxes = scheduled_semantic_boxes(plan, change);
    if boxes.len() > 100_000 {
        return Err(plan_error(
            "semantic change visualization exceeds 100000 scheduled regions",
        ));
    }
    let (base, mut prior) = if change.mode == ChangeMode::SideDiff {
        filters.push(format!("[{input}]split=2[changebase][changeinput]"));
        (Some("changebase"), "changeinput".to_owned())
    } else {
        (None, input.to_owned())
    };
    for (index, box_) in boxes.iter().enumerate() {
        let next = format!("change{index}");
        let (color, thickness) = match change.mode {
            ChangeMode::DirtyRows => ("orange@0.28", "fill"),
            ChangeMode::Heatmap => ("red@0.42", "fill"),
            ChangeMode::Trail => ("magenta@0.30", "fill"),
            ChangeMode::BoundingBox => ("cyan@0.90", "3"),
            ChangeMode::Outline | ChangeMode::SideDiff => ("yellow@0.90", "2"),
        };
        filters.push(format!(
            "[{prior}]drawbox=x={}:y={}:w={}:h={}:color={color}:t={thickness}:enable='between(n,{},{})'[{next}]",
            box_.region.x,
            box_.region.y,
            box_.region.width,
            box_.region.height,
            box_.start_frame,
            box_.end_frame,
        ));
        prior = next;
    }
    if let Some(base) = base {
        filters.push(format!("[{base}][{prior}]hstack=inputs=2[changeoutput]"));
        return Ok("changeoutput".to_owned());
    }
    return Ok(prior);
}

fn scheduled_semantic_boxes(
    plan: &PresentationPlan,
    change: &ChangeVisualization,
) -> Vec<ScheduledBox> {
    let mut boxes = Vec::new();
    let geometry = plan.source_geometry;
    let x_origin = plan.source_width.saturating_sub(geometry.text_width) / 2;
    let y_origin = plan.source_height.saturating_sub(geometry.text_height) / 2;
    for frame in &plan.change_frames {
        let regions = match change.mode {
            ChangeMode::DirtyRows => frame
                .dirty_rows
                .iter()
                .map(|row| {
                    return ChangeRegion {
                        x: x_origin,
                        y: y_origin.saturating_add(row.saturating_mul(geometry.cell_height)),
                        width: geometry.text_width,
                        height: geometry.cell_height,
                    };
                })
                .collect(),
            ChangeMode::BoundingBox => bounding_region(&frame.changed_regions)
                .into_iter()
                .collect(),
            _ => frame.changed_regions.clone(),
        };
        for output_frame in &frame.output_frames {
            for region in &regions {
                boxes.push(ScheduledBox {
                    region: *region,
                    start_frame: *output_frame,
                    end_frame: output_frame
                        .saturating_add(u64::from(change.hold_frames))
                        .min(plan.output_frame_count.saturating_sub(1)),
                });
            }
        }
    }
    return boxes;
}

fn bounding_region(regions: &[ChangeRegion]) -> Option<ChangeRegion> {
    let first = regions.first()?;
    let mut left = first.x;
    let mut top = first.y;
    let mut right = first.x.saturating_add(first.width);
    let mut bottom = first.y.saturating_add(first.height);
    for region in regions.iter().skip(1) {
        left = left.min(region.x);
        top = top.min(region.y);
        right = right.max(region.x.saturating_add(region.width));
        bottom = bottom.max(region.y.saturating_add(region.height));
    }
    return Some(ChangeRegion {
        x: left,
        y: top,
        width: right.saturating_sub(left),
        height: bottom.saturating_sub(top),
    });
}

fn append_encoded_change_visualization(
    filters: &mut Vec<String>,
    change: &ChangeVisualization,
    input: &str,
) -> Result<String> {
    let threshold = change.pixel_threshold.unwrap_or(16);
    filters.push(format!("[{input}]split=2[encodedbase][encodedinput]"));
    let mut diff_filter = format!(
        "[encodedinput]tblend=all_mode=difference,lutrgb=r='if(gte(val,{threshold}),val,0)':g='if(gte(val,{threshold}),val,0)':b='if(gte(val,{threshold}),val,0)'"
    );
    match change.mode {
        ChangeMode::Outline => diff_filter.push_str(",edgedetect=mode=colormix:high=0.4"),
        ChangeMode::Trail if change.hold_frames > 0 => {
            write!(
                &mut diff_filter,
                ",tmix=frames={}",
                u64::from(change.hold_frames) + 1,
            )
            .map_err(|_error| return plan_error("could not build encoded trail filter"))?;
        }
        ChangeMode::Heatmap | ChangeMode::Trail | ChangeMode::SideDiff => {}
        ChangeMode::DirtyRows | ChangeMode::BoundingBox => {
            return Err(plan_error("unsupported encoded-pixel change mode"));
        }
    }
    diff_filter.push_str("[encodeddiff]");
    filters.push(diff_filter);
    if change.mode == ChangeMode::SideDiff {
        filters.push("[encodedbase][encodeddiff]hstack=inputs=2[changeoutput]".to_owned());
    } else {
        filters.push(
            "[encodedbase][encodeddiff]blend=all_mode=screen:all_opacity=0.65[changeoutput]"
                .to_owned(),
        );
    }
    return Ok("changeoutput".to_owned());
}

fn append_pixel_transforms(
    filters: &mut Vec<String>,
    plan: &PresentationPlan,
    staging: &Path,
    input: &str,
) -> Result<String> {
    let mut prior = input.to_owned();
    let mut width = presentation_canvas_width_before_transforms(plan);
    let mut height =
        u32::from(plan.source_height) + if presentation_has_footer(plan) { 48 } else { 0 };
    let mut sequence = 0_usize;
    for transform in &plan.transforms {
        match transform {
            PixelTransform::CursorMask => {
                for (region, start_frame, end_frame) in cursor_mask_ranges(&plan.cursor_masks) {
                    let next = format!("transform{sequence}");
                    filters.push(format!(
                        "[{prior}]drawbox=x={}:y={}:w={}:h={}:color=black@1:t=fill:enable='between(n,{start_frame},{end_frame})'[{next}]",
                        region.x,
                        region.y,
                        region.width,
                        region.height,
                    ));
                    prior = next;
                    sequence += 1;
                    if plan.change_visualization.as_ref().is_some_and(|change| {
                        return change.mode == ChangeMode::SideDiff;
                    }) {
                        let next = format!("transform{sequence}");
                        filters.push(format!(
                            "[{prior}]drawbox=x={}:y={}:w={}:h={}:color=black@1:t=fill:enable='between(n,{start_frame},{end_frame})'[{next}]",
                            u32::from(region.x) + u32::from(plan.source_width),
                            region.y,
                            region.width,
                            region.height,
                        ));
                        prior = next;
                        sequence += 1;
                    }
                }
            }
            PixelTransform::Crop {
                x,
                y,
                width: crop_width,
                height: crop_height,
            } => {
                let next = format!("transform{sequence}");
                filters.push(format!(
                    "[{prior}]crop={crop_width}:{crop_height}:{x}:{y}[{next}]"
                ));
                prior = next;
                width = u32::from(*crop_width);
                height = u32::from(*crop_height);
                sequence += 1;
            }
            PixelTransform::Zoom {
                numerator,
                denominator,
            } => {
                width = scaled_even_u32(width, *numerator, *denominator);
                height = scaled_even_u32(height, *numerator, *denominator);
                let next = format!("transform{sequence}");
                filters.push(format!(
                    "[{prior}]scale={width}:{height}:flags=lanczos[{next}]"
                ));
                prior = next;
                sequence += 1;
            }
            PixelTransform::Callout {
                x,
                y,
                width: callout_width,
                height: callout_height,
                label,
            } => {
                let next = format!("transform{sequence}");
                filters.push(format!(
                    "[{prior}]drawbox=x={x}:y={y}:w={callout_width}:h={callout_height}:color=cyan@0.95:t=3[{next}]"
                ));
                prior = next;
                sequence += 1;
                if let Some(label) = label {
                    ensure_presentation_font(staging)?;
                    let name = format!("callout-{sequence}.txt");
                    write_secure(&staging.join(&name), label.as_bytes())?;
                    let next = format!("transform{sequence}");
                    filters.push(format!(
                        "[{prior}]drawtext=fontfile='fonts/FiraCode-Regular.ttf':textfile='{}':fontcolor=cyan:fontsize=18:box=1:boxcolor=black@0.7:x={}:y={}[{next}]",
                        filter_path_value(Path::new(&name))?,
                        x.saturating_add(4),
                        y.saturating_sub(24),
                    ));
                    prior = next;
                    sequence += 1;
                }
            }
        }
    }
    if let Some(branding) = &plan.layout.branding {
        ensure_presentation_font(staging)?;
        write_secure(&staging.join("branding.txt"), branding.as_bytes())?;
        filters.push(format!(
            "[{prior}]drawtext=fontfile='fonts/FiraCode-Regular.ttf':textfile='branding.txt':fontcolor=white@0.8:fontsize=16:box=1:boxcolor=black@0.55:x=w-tw-12:y=12[branded]"
        ));
        prior = "branded".to_owned();
    }
    return Ok(prior);
}

fn presentation_canvas_width_before_transforms(plan: &PresentationPlan) -> u32 {
    if plan.change_visualization.as_ref().is_some_and(|change| {
        return change.mode == ChangeMode::SideDiff;
    }) {
        return u32::from(plan.source_width) * 2;
    }
    return u32::from(plan.source_width);
}

fn scaled_even_u32(value: u32, numerator: u16, denominator: u16) -> u32 {
    return value.saturating_mul(u32::from(numerator)) / u32::from(denominator) / 2 * 2;
}

fn cursor_mask_ranges(masks: &[CursorMaskFrame]) -> Vec<(ChangeRegion, u64, u64)> {
    let mut ranges: Vec<(ChangeRegion, u64, u64)> = Vec::new();
    for mask in masks {
        match ranges.last_mut() {
            Some((region, _, end))
                if *region == mask.region && end.saturating_add(1) == mask.output_frame =>
            {
                *end = mask.output_frame;
            }
            _ => ranges.push((mask.region, mask.output_frame, mask.output_frame)),
        }
    }
    return ranges;
}

fn ensure_presentation_font(staging: &Path) -> Result<()> {
    let fonts = staging.join("fonts");
    fs_err::create_dir_all(&fonts).map_err(|source| {
        return Error::Filesystem {
            path: fonts.clone(),
            source,
        };
    })?;
    let font = fonts.join("FiraCode-Regular.ttf");
    if font.exists() {
        let bytes = fs_err::read(&font).map_err(|source| {
            return Error::Filesystem {
                path: font.clone(),
                source,
            };
        })?;
        if bytes != SEMANTIC_FONT_BYTES {
            return Err(plan_error("staged presentation font changed unexpectedly"));
        }
    } else {
        write_secure(&font, SEMANTIC_FONT_BYTES)?;
    }
    return Ok(());
}

fn prepare_subtitle_resources(plan: &PresentationPlan, staging: &Path) -> Result<Vec<PathBuf>> {
    if plan.annotations.is_empty() && plan.subtitles.is_empty() {
        return Ok(Vec::new());
    }
    require_ass_filter()?;
    ensure_presentation_font(staging)?;
    let mut resources = Vec::new();
    if !plan.annotations.is_empty() {
        let generated = staging.join("annotations.ass");
        write_secure(&generated, generated_ass(plan)?.as_bytes())?;
        resources.push(generated);
    }
    for (index, subtitle) in plan.subtitles.iter().enumerate() {
        validate_ass_resource(&subtitle.source_path)?;
        let destination = staging.join(format!("subtitle-{index}.ass"));
        let bytes = fs_err::read(&subtitle.source_path).map_err(|source| {
            return Error::Filesystem {
                path: subtitle.source_path.clone(),
                source,
            };
        })?;
        if blake3::hash(&bytes).to_hex().as_str() != subtitle.resource_hash {
            return Err(plan_error(format!(
                "ASS subtitle resource {} changed after planning",
                subtitle.source_path.display()
            )));
        }
        write_secure(&destination, &bytes)?;
        resources.push(destination);
    }
    return Ok(resources);
}

fn require_ass_filter() -> Result<()> {
    let output = Command::new("ffmpeg")
        .args(["-hide_banner", "-filters"])
        .output()
        .map_err(|source| {
            return Error::Filesystem {
                path: PathBuf::from("ffmpeg"),
                source,
            };
        })?;
    if !output.status.success()
        || !String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| return line.split_whitespace().any(|field| return field == "ass"))
    {
        return Err(plan_error(
            "FFmpeg ASS/libass filter is required for annotations or subtitle tracks",
        ));
    }
    return Ok(());
}

fn generated_ass(plan: &PresentationPlan) -> Result<String> {
    let canvas_height = if plan.output_height > 0 {
        u64::from(plan.output_height)
    } else {
        u64::from(plan.source_height) + if presentation_has_footer(plan) { 48 } else { 0 }
    };
    let mut ass = format!(
        "[Script Info]\nScriptType: v4.00+\nWrapStyle: 2\nScaledBorderAndShadow: yes\nPlayResX: {}\nPlayResY: {}\nYCbCr Matrix: None\n\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n",
        presentation_canvas_width(plan),
        canvas_height
    );
    for (index, annotation) in plan.annotations.iter().enumerate() {
        if plan
            .annotations
            .iter()
            .take(index)
            .any(|prior| return prior.annotation_index == annotation.annotation_index)
        {
            continue;
        }
        let foreground = ass_color(&annotation.style.foreground, 255)?;
        let background = ass_color(
            &annotation.style.background,
            annotation.style.background_opacity,
        )?;
        writeln!(
            &mut ass,
            "Style: A{},Fira Code,{},{foreground},&H00000000,{background},{background},0,0,0,0,100,100,0,0,3,{},0,7,0,0,0,1",
            annotation.annotation_index, annotation.style.font_size, annotation.style.border,
        )
        .map_err(|_error| return plan_error("could not generate ASS style"))?;
    }
    ass.push_str("\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    let dynamic_event_count: u64 = plan
        .annotations
        .iter()
        .filter(|annotation| return annotation.template.is_some())
        .map(|annotation| return annotation.output_end_frame - annotation.output_start_frame)
        .sum();
    if dynamic_event_count > 1_000_000 {
        return Err(plan_error(
            "dynamic annotation expansion exceeds one million ASS events",
        ));
    }
    for annotation in &plan.annotations {
        if let Some(text) = &annotation.text {
            write_ass_dialogue(
                &mut ass,
                plan,
                annotation,
                annotation.output_start_frame,
                annotation.output_end_frame,
                text,
            )?;
            continue;
        }
        for output_frame in annotation.output_start_frame..annotation.output_end_frame {
            let template = annotation.template.as_deref().unwrap_or_default();
            let text = expand_annotation_template(plan, output_frame, template)?;
            write_ass_dialogue(
                &mut ass,
                plan,
                annotation,
                output_frame,
                output_frame + 1,
                &text,
            )?;
        }
    }
    return Ok(ass);
}

fn write_ass_dialogue(
    ass: &mut String,
    plan: &PresentationPlan,
    annotation: &NormalizedAnnotation,
    start_frame: u64,
    end_frame: u64,
    text: &str,
) -> Result<()> {
    let (alignment, x, y) = annotation_position(
        &annotation.position,
        i64::from(presentation_canvas_width(plan)),
        i64::from(plan.source_height) + if presentation_has_footer(plan) { 48 } else { 0 },
    );
    writeln!(
        ass,
        "Dialogue: {},{},{},A{},,0,0,0,,{{\\an{}\\pos({x},{y})}}{}",
        i32::from(annotation.z_order) + 32_768,
        ass_time(start_frame, plan.fps, false),
        ass_time(end_frame, plan.fps, true),
        annotation.annotation_index,
        alignment,
        escape_ass_text(text),
    )
    .map_err(|_error| return plan_error("could not generate ASS dialogue"))?;
    return Ok(());
}

fn presentation_has_footer(plan: &PresentationPlan) -> bool {
    return plan.layout.caption == CaptionLayout::Footer
        && plan.clips.iter().any(|clip| return clip.caption.is_some());
}

fn presentation_canvas_width(plan: &PresentationPlan) -> u16 {
    if plan.output_width > 0 {
        return plan.output_width;
    }
    if plan.change_visualization.as_ref().is_some_and(|change| {
        return change.mode == ChangeMode::SideDiff;
    }) {
        return plan.source_width.saturating_mul(2);
    }
    return plan.source_width;
}

fn annotation_position(position: &AnnotationPosition, width: i64, height: i64) -> (u8, i64, i64) {
    let x = i64::from(position.x);
    let y = i64::from(position.y);
    return match position.anchor {
        AnnotationAnchor::TopLeft => (7, x, y),
        AnnotationAnchor::TopRight => (9, width + x, y),
        AnnotationAnchor::BottomLeft => (1, x, height + y),
        AnnotationAnchor::BottomRight => (3, width + x, height + y),
        AnnotationAnchor::Center => (5, width / 2 + x, height / 2 + y),
    };
}

fn ass_color(rgb: &str, opacity: u8) -> Result<String> {
    let value = u32::from_str_radix(rgb.trim_start_matches('#'), 16)
        .map_err(|_error| return plan_error("annotation color is invalid"))?;
    let red = value >> 16;
    let green = value >> 8 & 0xff;
    let blue = value & 0xff;
    return Ok(format!(
        "&H{:02X}{blue:02X}{green:02X}{red:02X}",
        255 - opacity
    ));
}

fn ass_time(frame: u64, fps: u16, end: bool) -> String {
    let numerator = frame.saturating_mul(100);
    let centiseconds = if end {
        ceil_ratio(numerator, u64::from(fps))
    } else {
        numerator / u64::from(fps)
    };
    return format!(
        "{}:{:02}:{:02}.{:02}",
        centiseconds / 360_000,
        centiseconds / 6_000 % 60,
        centiseconds / 100 % 60,
        centiseconds % 100,
    );
}

fn escape_ass_text(text: &str) -> String {
    return text
        .replace('\\', "\\\\")
        .replace('{', "\\{")
        .replace('}', "\\}")
        .replace("\r\n", "\\N")
        .replace(['\r', '\n'], "\\N");
}

fn expand_annotation_template(
    plan: &PresentationPlan,
    output_frame: u64,
    template: &str,
) -> Result<String> {
    let clip = plan
        .clips
        .iter()
        .find(|clip| {
            return clip.output_start_frame <= output_frame && output_frame < clip.output_end_frame;
        })
        .ok_or_else(|| return plan_error("annotation frame has no source mapping"))?;
    let (source_frame, _) = mapped_source_frame(clip, output_frame);
    let marker = plan
        .markers
        .iter()
        .filter(|marker| return marker.nominal_frame == source_frame)
        .map(|marker| return marker.label.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let source_millis = source_frame.saturating_mul(1000) / u64::from(plan.fps);
    let source_time = format!("{}.{:03}s", source_millis / 1000, source_millis % 1000);
    let output_millis = output_frame.saturating_mul(1000) / u64::from(plan.fps);
    let timecode = format!(
        "{:02}:{:02}:{:02}.{:03}",
        output_millis / 3_600_000,
        output_millis / 60_000 % 60,
        output_millis / 1000 % 60,
        output_millis % 1000,
    );
    return Ok(template
        .replace("{output_frame}", &output_frame.to_string())
        .replace("{source_frame}", &source_frame.to_string())
        .replace("{source_time}", &source_time)
        .replace("{marker}", &marker)
        .replace("{timecode}", &timecode));
}

fn mapped_source_frame(clip: &NormalizedClip, output_frame: u64) -> (u64, &'static str) {
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
        PixelTransform::Callout { .. } => "callout",
    }
    .to_owned();
}

fn frame_transform_names(plan: &PresentationPlan, output_frame: u64) -> Vec<String> {
    let mut names = plan
        .transforms
        .iter()
        .filter(|transform| {
            return !matches!(transform, PixelTransform::CursorMask)
                || plan
                    .cursor_masks
                    .iter()
                    .any(|mask| return mask.output_frame == output_frame);
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
        return Err(plan_error(format!("{command} -version failed")));
    }
    return String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(str::to_owned)
        .ok_or_else(|| return plan_error(format!("{command} returned no version")));
}

fn probe_delivery_video(staging: &Path) -> Result<serde_json::Value> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=codec_name,pix_fmt,width,height,r_frame_rate,nb_frames,color_range,color_space,color_transfer,color_primaries:format=format_name,duration",
            "-of",
            "json",
            "presentation.mp4",
        ])
        .current_dir(staging)
        .output()
        .map_err(|source| {
            return Error::Filesystem {
                path: PathBuf::from("ffprobe"),
                source,
            };
        })?;
    if !output.status.success() {
        write_secure(&staging.join("ffprobe.log"), &output.stderr)?;
        return Err(plan_error("FFprobe rejected the presentation output"));
    }
    let value = serde_json::from_slice(&output.stdout).map_err(|source| {
        return Error::InvalidJson {
            path: staging.join("output-probe.json"),
            json_path: "$".to_owned(),
            source,
        };
    })?;
    return Ok(value);
}

fn write_source_frame_hashes(
    _plan: &PresentationPlan,
    source_video: &Path,
    staging: &Path,
) -> Result<()> {
    let hashes_path = staging.join("source-frame-hashes.framemd5");
    let log_path = staging.join("source-frame-hashes.log");
    let hashes = secure_file(&hashes_path)?;
    let log = secure_file(&log_path)?;
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-nostdin", "-i"])
        .arg(source_video)
        .args(["-map", "0:v:0", "-f", "framemd5", "-"])
        .stdout(Stdio::from(hashes))
        .stderr(Stdio::from(log))
        .status()
        .map_err(|source| {
            return Error::Filesystem {
                path: PathBuf::from("ffmpeg"),
                source,
            };
        })?;
    if !status.success() {
        return Err(plan_error(format!(
            "FFmpeg source-frame hashing failed with {status}"
        )));
    }
    let metadata = fs_err::metadata(&hashes_path).map_err(|source| {
        return Error::Filesystem {
            path: hashes_path,
            source,
        };
    })?;
    if metadata.len() == 0 {
        return Err(plan_error("FFmpeg source-frame hash output was empty"));
    }
    return Ok(());
}

fn filter_path_value(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| return plan_error("FFmpeg filter resource path is not UTF-8"))?;
    return Ok(value
        .replace('\\', "\\\\")
        .replace(':', "\\:")
        .replace('\'', "\\'"));
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

fn validate_transform_geometry(
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
                    "cursor_mask must precede crop and zoom transforms",
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

fn validate_annotation_template(template: &str) -> Result<()> {
    let mut token = None::<String>;
    for character in template.chars() {
        match (character, &mut token) {
            ('{', None) => token = Some(String::new()),
            ('{', Some(_)) => return Err(plan_error("annotation template has a nested token")),
            ('}', None) => {
                return Err(plan_error(
                    "annotation template has an unmatched closing brace",
                ));
            }
            ('}', Some(value)) => {
                if !matches!(
                    value.as_str(),
                    "output_frame" | "source_frame" | "source_time" | "marker" | "timecode"
                ) {
                    return Err(plan_error(format!(
                        "annotation template token `{{{value}}}` is unsupported"
                    )));
                }
                token = None;
            }
            (character, Some(value)) => value.push(character),
            (_, None) => {}
        }
    }
    if token.is_some() {
        return Err(plan_error("annotation template has an unclosed token"));
    }
    return Ok(());
}

fn validate_ass_resource(path: &Path) -> Result<()> {
    let metadata = fs_err::metadata(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    if !metadata.is_file() || metadata.len() > 16 * 1024 * 1024 {
        return Err(plan_error(format!(
            "ASS resource `{}` must be a regular file no larger than 16 MiB",
            path.display()
        )));
    }
    let bytes = fs_err::read(path).map_err(|source| {
        return Error::Filesystem {
            path: path.to_path_buf(),
            source,
        };
    })?;
    let content = str::from_utf8(&bytes)
        .map_err(|_error| return plan_error("ASS subtitle resource must be UTF-8"))?;
    if !content.contains("[Script Info]")
        || !content.contains("[Events]")
        || !content
            .lines()
            .any(|line| return line.starts_with("Dialogue:"))
    {
        return Err(plan_error(
            "ASS subtitle resource requires Script Info, Events, and Dialogue sections",
        ));
    }
    return Ok(());
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

fn ranges_overlap(left: SourceRange, right: SourceRange) -> bool {
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
    selector.validate()?;
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

fn frame_floor(monotonic_ns: u64, fps: u16) -> u64 {
    return monotonic_ns.saturating_mul(u64::from(fps)) / NANOS_PER_SECOND;
}

fn frame_ceil(monotonic_ns: u64, fps: u16) -> u64 {
    return ceil_ratio(
        monotonic_ns.saturating_mul(u64::from(fps)),
        NANOS_PER_SECOND,
    );
}

fn ceil_ratio(numerator: u64, denominator: u64) -> u64 {
    return numerator.saturating_add(denominator - 1) / denominator;
}

fn hash_file(path: &Path) -> Result<String> {
    return hash_regular_file(path).map(|(_, hash)| return hash);
}

fn plan_error(reason: impl Into<String>) -> Error {
    return Error::InvalidReplay {
        reason: reason.into(),
    };
}

fn valid_rgb(value: &str) -> bool {
    return value.len() == 7
        && value.starts_with('#')
        && value
            .as_bytes()
            .get(1..)
            .is_some_and(|hex| return hex.iter().all(u8::is_ascii_hexdigit));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_retiming_and_hold_round_up_to_frames() {
        assert_eq!(frame_floor(1_500_000_000, 24), 36);
        assert_eq!(frame_ceil(1_500_000_001, 24), 37);
        assert_eq!(ceil_ratio(25, 4), 7);
        assert_eq!(ceil_ratio(801 * 24, 1000), 20);
    }

    #[test]
    fn edit_selectors_require_one_source_coordinate() {
        let selector = EditSelector {
            marker: Some("before".to_owned()),
            monotonic_ns: Some(10),
            ..EditSelector::default()
        };
        assert!(selector.validate().is_err());
    }

    #[test]
    fn edit_plan_bounds_speed_caption_and_source_overlap() {
        let clip = EditClip {
            from: EditSelector {
                monotonic_ns: Some(0),
                ..EditSelector::default()
            },
            to: EditSelector {
                monotonic_ns: Some(1_000_000_000),
                ..EditSelector::default()
            },
            speed: RationalSpeed {
                numerator: 65,
                denominator: 1,
            },
            hold_ms: 0,
            caption: Some("bounded".to_owned()),
        };
        let edit = EditPlan {
            version: EDIT_PLAN_VERSION,
            clips: vec![clip],
            ..EditPlan::default()
        };
        assert!(edit.validate().is_err());
        assert!(ranges_overlap(
            SourceRange {
                start_monotonic_ns: 0,
                end_monotonic_ns: 10,
            },
            SourceRange {
                start_monotonic_ns: 9,
                end_monotonic_ns: 20,
            }
        ));
        assert!(!ranges_overlap(
            SourceRange {
                start_monotonic_ns: 0,
                end_monotonic_ns: 10,
            },
            SourceRange {
                start_monotonic_ns: 10,
                end_monotonic_ns: 20,
            }
        ));
    }

    #[test]
    fn presentation_map_distinguishes_retimed_and_hold_frames() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let path = temporary.path().join("presentation-map.jsonl");
        let plan = PresentationPlan {
            version: 1,
            edit_plan_hash: "a".repeat(64),
            source: PresentationSource::Native {
                video: PathBuf::from("source.mkv"),
                video_hash: "b".repeat(64),
            },
            fps: 24,
            clips: vec![NormalizedClip {
                source_range: SourceRange {
                    start_monotonic_ns: 0,
                    end_monotonic_ns: 1_000_000_000,
                },
                source_start_frame: 0,
                source_end_frame: 24,
                output_start_frame: 0,
                output_end_frame: 8,
                retimed_frame_count: 6,
                hold_frame_count: 2,
                speed: RationalSpeed {
                    numerator: 4,
                    denominator: 1,
                },
                caption: Some("fast clip".to_owned()),
            }],
            output_frame_count: 8,
            ..PresentationPlan::default()
        };
        write_presentation_map(&path, &plan, &[]).expect("write frame map");
        let entries = fs_err::read_to_string(path)
            .expect("read map")
            .lines()
            .map(|line| {
                return serde_json::from_str::<PresentationMapEntry>(line).expect("map entry");
            })
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 8);
        assert_eq!(entries[5].source_frame, 20);
        assert_eq!(entries[6].operation, "hold");
        assert!(entries.iter().all(|entry| return entry.captioned));
    }

    #[test]
    #[ignore = "requires FFmpeg with libx264"]
    fn exports_multi_clip_h264_presentation_atomically() {
        let temporary = tempfile::tempdir().expect("temporary directory");
        let source = temporary.path().join("source.mkv");
        let status = Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=size=64x64:rate=24:duration=1",
                "-c:v",
                "ffv1",
            ])
            .arg(&source)
            .status()
            .expect("run FFmpeg fixture");
        assert!(status.success());
        let plan = PresentationPlan {
            version: 1,
            edit_plan_hash: "a".repeat(64),
            source: PresentationSource::Native {
                video_hash: hash_file(&source).expect("source hash"),
                video: source,
            },
            fps: 24,
            clips: vec![NormalizedClip {
                source_range: SourceRange {
                    start_monotonic_ns: 0,
                    end_monotonic_ns: 500_000_000,
                },
                source_start_frame: 0,
                source_end_frame: 12,
                output_start_frame: 0,
                output_end_frame: 8,
                retimed_frame_count: 6,
                hold_frame_count: 2,
                speed: RationalSpeed {
                    numerator: 2,
                    denominator: 1,
                },
                caption: None,
            }],
            output_frame_count: 8,
            source_width: 64,
            source_height: 64,
            output_width: 64,
            output_height: 64,
            annotations: vec![NormalizedAnnotation {
                annotation_index: 0,
                output_start_frame: 0,
                output_end_frame: 8,
                text: None,
                template: Some("frame {output_frame} source {source_frame}".to_owned()),
                position: AnnotationPosition::default(),
                style: AnnotationStyle::default(),
                z_order: 0,
            }],
            change_visualization: Some(ChangeVisualization {
                source: ChangeSource::EncodedPixels,
                mode: ChangeMode::Heatmap,
                hold_frames: 0,
                pixel_threshold: Some(8),
            }),
            change_algorithm: Some("ffmpeg-adjacent-decoded-rgb-difference-v1".to_owned()),
            layout: PresentationLayout {
                caption: CaptionLayout::None,
                branding: Some("example".to_owned()),
            },
            transforms: vec![PixelTransform::Callout {
                x: 2,
                y: 26,
                width: 60,
                height: 30,
                label: Some("region".to_owned()),
            }],
            ..PresentationPlan::default()
        };
        let destination = temporary.path().join("presentation");
        let outcome = export(&plan, &destination).expect("export presentation");
        assert!(outcome.video.is_file());
        assert!(outcome.directory.join("manifest.json").is_file());
        assert!(outcome.directory.join("presentation-map.jsonl").is_file());
        assert!(outcome.directory.join("annotations.ass").is_file());
        assert!(
            fs_err::read_to_string(outcome.directory.join("annotations.ass"))
                .expect("read generated annotations")
                .contains("frame 7 source 11")
        );
        assert!(
            fs_err::read_to_string(outcome.directory.join("filter.txt"))
                .expect("read filter graph")
                .contains("tblend=all_mode=difference")
        );
        assert!(
            fs_err::read_to_string(outcome.directory.join("filter.txt"))
                .expect("read transforms")
                .contains("drawbox=x=2:y=26:w=60:h=30")
        );
        assert!(outcome.directory.join("branding.txt").is_file());
        assert!(
            fs_err::read_to_string(outcome.directory.join("source-frame-hashes.framemd5"))
                .expect("read decoded source hashes")
                .contains("#format: frame checksums")
        );
        let preview_outcome = preview(
            &plan,
            temporary.path().join("preview"),
            PreviewOptions {
                columns: 2,
                tile_width: 80,
                max_frames: 4,
                change_only: true,
                preview_width: 160,
            },
        )
        .expect("render preview");
        assert!(preview_outcome.contact_sheet.is_file());
        assert!(preview_outcome.video.is_file());
        assert!(preview_outcome.directory.join("manifest.json").is_file());
    }

    #[test]
    fn semantic_change_filters_retain_state_provenance_and_hold() {
        let plan = PresentationPlan {
            fps: 24,
            source_width: 80,
            source_height: 40,
            source_geometry: TerminalGeometryReport {
                cols: 10,
                rows: 2,
                cell_width: 8,
                cell_height: 20,
                text_width: 80,
                text_height: 40,
            },
            output_frame_count: 5,
            change_visualization: Some(ChangeVisualization {
                source: ChangeSource::SemanticCells,
                mode: ChangeMode::Outline,
                hold_frames: 2,
                pixel_threshold: None,
            }),
            change_frames: vec![NormalizedChangeFrame {
                state_id: 7,
                state_hash: "a".repeat(64),
                source_frame: 2,
                output_frames: vec![2],
                changed_regions: vec![ChangeRegion {
                    x: 8,
                    y: 0,
                    width: 8,
                    height: 20,
                }],
                dirty_rows: vec![0],
                evidence_level: "semantic_scheduled_for_frame".to_owned(),
            }],
            ..PresentationPlan::default()
        };
        let mut filters = Vec::new();
        let output = append_change_visualization(&mut filters, &plan, "input")
            .expect("build semantic overlay");
        assert_eq!(output, "change0");
        assert!(filters[0].contains("between(n,2,4)"));
        assert_eq!(semantic_change_states_at(&plan, 4), vec![7]);
    }

    #[test]
    fn transform_geometry_is_ordered_bounded_and_even() {
        let edit = EditPlan {
            transforms: vec![
                PixelTransform::Crop {
                    x: 8,
                    y: 4,
                    width: 32,
                    height: 24,
                },
                PixelTransform::Zoom {
                    numerator: 2,
                    denominator: 1,
                },
                PixelTransform::Callout {
                    x: 2,
                    y: 2,
                    width: 20,
                    height: 10,
                    label: None,
                },
            ],
            ..EditPlan::default()
        };
        assert_eq!(
            validate_transform_geometry(&edit, 80, 40, false).expect("valid transforms"),
            (64, 48)
        );
        let invalid = EditPlan {
            transforms: vec![
                PixelTransform::Crop {
                    x: 0,
                    y: 0,
                    width: 32,
                    height: 24,
                },
                PixelTransform::CursorMask,
            ],
            ..EditPlan::default()
        };
        assert!(validate_transform_geometry(&invalid, 80, 40, false).is_err());
    }

    #[test]
    fn cursor_masks_coalesce_only_consecutive_identical_regions() {
        let make = |output_frame, x| {
            return CursorMaskFrame {
                output_frame,
                source_frame: output_frame,
                state_id: output_frame,
                state_hash: "a".repeat(64),
                region: ChangeRegion {
                    x,
                    y: 0,
                    width: 2,
                    height: 16,
                },
                evidence_level: "semantic_scheduled_for_frame".to_owned(),
            };
        };
        assert_eq!(
            cursor_mask_ranges(&[make(0, 4), make(1, 4), make(2, 8)]),
            vec![
                (
                    ChangeRegion {
                        x: 4,
                        y: 0,
                        width: 2,
                        height: 16
                    },
                    0,
                    1
                ),
                (
                    ChangeRegion {
                        x: 8,
                        y: 0,
                        width: 2,
                        height: 16
                    },
                    2,
                    2
                )
            ]
        );
    }

    #[test]
    fn preview_selection_is_bounded_deterministic_and_change_aware() {
        let plan = PresentationPlan {
            output_frame_count: 10,
            change_visualization: Some(ChangeVisualization {
                source: ChangeSource::SemanticCells,
                mode: ChangeMode::Outline,
                hold_frames: 1,
                pixel_threshold: None,
            }),
            change_frames: vec![NormalizedChangeFrame {
                state_id: 1,
                state_hash: "a".repeat(64),
                source_frame: 3,
                output_frames: vec![3, 8],
                changed_regions: Vec::new(),
                dirty_rows: Vec::new(),
                evidence_level: "semantic_scheduled_for_frame".to_owned(),
            }],
            ..PresentationPlan::default()
        };
        let selected = select_preview_frames(
            &plan,
            PreviewOptions {
                max_frames: 3,
                change_only: true,
                ..PreviewOptions::default()
            },
        )
        .expect("select changes");
        assert_eq!(selected, vec![3, 4, 9]);
    }
}
