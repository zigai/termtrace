//! Additive delivery contracts. Legacy presentation plans and MP4 settings are unchanged.

use super::sdk::{
    ArtifactProvenance, ArtifactSourceKind, ArtifactSourceRef, DecimalU64, SdkArtifactMember,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A GIF loop is either infinite or absent (play once).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GifLoop {
    /// Netscape loop extension with count zero.
    #[default]
    Infinite,
    /// No loop extension.
    Once,
}

/// Fixed opaque delivery policies; no transparency or misleading lossless options.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "format", rename_all = "snake_case", deny_unknown_fields)]
pub enum DerivativeFormat {
    /// Global 256-color palette, ordered Bayer scale 5, full opaque frames.
    Gif {
        /// Explicit loop policy.
        #[serde(default, rename = "loop")]
        loop_policy: GifLoop,
    },
    /// VP9 CRF 32, zero bitrate, single thread, limited-range YUV420 BT.709 matrix.
    Webm {},
}
impl DerivativeFormat {
    /// Stable format name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Gif { .. } => return "gif",
            Self::Webm {} => return "webm",
        }
    }

    /// Fixed member basename.
    pub fn filename(self) -> String {
        return format!("presentation.{}", self.name());
    }
}

/// Nonempty duplicate-free output selection, with explicit source truth.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DerivativeExportOptions {
    /// One or both fixed delivery formats.
    #[schemars(length(min = 1, max = 2))]
    pub formats: Vec<DerivativeFormat>,
    /// Must match verified source pixels; never a fallback preference.
    pub source_kind: ArtifactSourceKind,
}
impl DerivativeExportOptions {
    /// Validate without launching tools.
    ///
    /// # Errors
    /// Rejects empty or repeated formats.
    pub fn validate(&self) -> Result<(), String> {
        if self.formats.is_empty()
            || self.formats.len() > 2
            || matches!(self.formats.as_slice(),[a,b] if a.name()==b.name())
        {
            return Err("formats must be nonempty and duplicate-free".into());
        }
        return Ok(());
    }
}

/// Nearest-centisecond boundary, ties rounded upward. Includes boundary N.
///
/// # Errors
/// Rejects FPS outside 1..=100 or a boundary beyond u64 centiseconds.
pub fn gif_boundary_centiseconds(frame: u64, fps: u16) -> Result<u64, String> {
    if !(1..=100).contains(&fps) {
        return Err("GIF FPS must be in 1..=100".into());
    }

    let boundary = (u128::from(frame) * 200 + u128::from(fps)) / (2 * u128::from(fps));

    return u64::try_from(boundary).map_err(|_error| return "GIF timing schedule overflow".into());
}

/// One timing decision, including the last frame's explicit delay.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GifFrameTiming {
    /// Zero-based normalized output frame.
    pub frame: DecimalU64,
    /// Rounded frame boundary in centiseconds.
    pub start_centiseconds: DecimalU64,
    /// Positive centisecond delay.
    pub delay_centiseconds: u16,
}

/// Actual encoder facts, not the legacy plan's H.264 delivery settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DerivativeOutput {
    /// Effective output and loop policy.
    pub delivery: DerivativeFormat,
    /// Relative member path.
    pub path: String,
    /// Exact bytes hash.
    pub blake3: String,
    /// Decode-verified number of frames.
    pub frame_count: DecimalU64,
    /// Explicit-path ffprobe evidence (including time base/color facts).
    #[serde(with = "crate::value::json")]
    #[schemars(with = "serde_json::Value")]
    pub probe: serde_json::Value,
}

/// Separately versioned multi-delivery presentation manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DerivativeManifest {
    /// Exactly 1.
    #[schemars(range(min = 1, max = 1))]
    pub version: u32,
    /// Verified canonical source boundary.
    pub source: ArtifactSourceRef,
    /// Always `presentation_output` with the verified input truth.
    pub provenance: ArtifactProvenance,
    /// Common normalized edit identity.
    pub edit_plan_hash: String,
    /// Pixel/profile source identity.
    pub source_identity_hash: String,
    /// Relative snapshot of the input capture/semantic provenance manifest.
    pub source_provenance_path: String,
    /// Hash of that exact snapshot (profile policy and tool provenance retained).
    pub source_provenance_blake3: String,
    /// Hash of the private lossless RGB composition used by all deliveries.
    pub composition_blake3: String,
    /// Common normalized plan member hash.
    pub normalized_plan_blake3: String,
    /// Common mapping member hash.
    pub presentation_map_blake3: String,
    /// Effective frame rate, in frames per second.
    pub fps: u16,
    /// Even padded canvas width in pixels.
    pub width: u16,
    /// Even padded canvas height in pixels.
    pub height: u16,
    /// Output count, including all identical and hold frames.
    pub frame_count: DecimalU64,
    /// Explicit, honest source RGB/color conversion assumptions.
    pub color_policy: String,
    /// Palette and timing policy, including viewer clamping caveat.
    pub gif_policy: String,
    /// Pinned tool build report.
    pub ffmpeg_version: String,
    /// Probe build report.
    pub ffprobe_version: String,
    /// All direct ffmpeg command vectors, in execution order.
    pub commands: Vec<Vec<String>>,
    /// Per-format actual delivery facts.
    pub outputs: Vec<DerivativeOutput>,
    /// Complete non-manifest payload (no self hash).
    pub members: Vec<SdkArtifactMember>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_gif_schedules_bound_every_boundary_and_final_delay() {
        for fps in [1, 3, 24, 30, 60, 99, 100] {
            for i in 0..10_001 {
                let q = gif_boundary_centiseconds(i, fps).unwrap();
                let next = gif_boundary_centiseconds(i + 1, fps).unwrap();
                assert!(next > q);
                assert!(
                    (i128::from(q) * i128::from(fps) - i128::from(i) * 100).abs() * 2
                        <= i128::from(fps)
                );
            }
        }

        assert_eq!(gif_boundary_centiseconds(1, 60).unwrap(), 2);
        assert_eq!(gif_boundary_centiseconds(2, 60).unwrap(), 3);
        assert!(gif_boundary_centiseconds(1, 0).is_err());
        assert!(gif_boundary_centiseconds(1, 101).is_err());
        assert!(gif_boundary_centiseconds(u64::MAX, 1).is_err());
    }

    #[test]
    fn strict_options_reject_unsupported_delivery_settings() {
        for input in [
            r#"{"format":"webm","loop":"infinite"}"#,
            r#"{"format":"gif","loop":"forever"}"#,
            r#"{"format":"webm","lossless":true}"#,
            r#"{"format":"gif","loop":"once","transparent":true}"#,
        ] {
            assert!(serde_json::from_str::<DerivativeFormat>(input).is_err());
        }

        assert!(DerivativeExportOptions {
            formats: vec![],
            source_kind: ArtifactSourceKind::NativeReplay
        }
        .validate()
        .is_err());
        assert!(DerivativeExportOptions {
            formats: vec![DerivativeFormat::Webm {}; 2],
            source_kind: ArtifactSourceKind::NativeReplay
        }
        .validate()
        .is_err());
    }
}
