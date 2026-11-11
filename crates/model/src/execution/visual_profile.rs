//! Versioned visual-profile selection, recorded identity, and preflight report contracts.

use crate::{
    artifact::{save::SemanticImageResource, visual::GeometrySolveReport},
    error::ModelValidationError,
    execution::session::GhosttyVisualOverrides,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Only supported visual-profile contract version.
pub const VISUAL_PROFILE_VERSION: u32 = 1;
/// Only supported preflight report version.
pub const VISUAL_PROFILE_REPORT_VERSION: u32 = 1;
/// Display DPI a native profile resolves to when it names none.
pub const DEFAULT_PROFILE_DPI: u32 = 96;
const MAX_FONT_SIZE_POINTS: u32 = 255;
const MAX_PADDING_POINTS: u16 = 2000;
const MIN_DPI: u32 = 24;
const MAX_DPI: u32 = 960;
const MAX_TEXT_BYTES: usize = 4096;

/// Immutable visual configuration selected for one launch.
///
/// The native section configures Ghostty for native pixels. The semantic section selects the
/// bundled semantic renderer and never changes native configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VisualProfile {
    /// Contract version; exactly 1.
    pub version: u32,
    /// Native Ghostty configuration; selecting it requires Ghostty and the named fonts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<NativeVisualProfile>,
    /// Semantic renderer selection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic: Option<SemanticVisualProfile>,
}

/// Native Ghostty settings. Absent fields keep the base configuration's value.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeVisualProfile {
    /// Absolute Ghostty config file replacing the isolated Ghostty-default base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
    /// Exact installed font family; no substitute is accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
    /// Font size in points as an exact decimal string, such as `"15"` or `"13.5"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size_points: Option<String>,
    /// Window padding in points on each side; zero is an explicit zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padding_points: Option<PaddingPoints>,
    /// Default foreground as `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<String>,
    /// Default background as `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    /// Display DPI used by every native probe, launch, and replay; 96 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpi: Option<u32>,
}

/// Symmetric window padding in points per side.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PaddingPoints {
    /// Left and right padding.
    pub x: u16,
    /// Top and bottom padding.
    pub y: u16,
}

/// Semantic renderer selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SemanticVisualProfile {
    /// Bundled semantic resource preset.
    pub preset: SemanticPreset,
}

/// Bundled semantic fonts and fixed metrics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
pub enum SemanticPreset {
    /// Bundled JetBrains Mono and Symbols Nerd Font faces with fixed 12x24 cells.
    #[serde(rename = "bundled-v1")]
    BundledV1,
}

impl VisualProfile {
    /// Native standard profile: JetBrains Mono 15 pt, 8-point padding, fixed colors, 96 DPI,
    /// and the bundled semantic preset.
    #[must_use]
    pub fn native_standard() -> Self {
        return Self {
            version: VISUAL_PROFILE_VERSION,
            native: Some(NativeVisualProfile {
                config: None,
                font_family: Some("JetBrains Mono".to_owned()),
                font_size_points: Some("15".to_owned()),
                padding_points: Some(PaddingPoints { x: 8, y: 8 }),
                foreground: Some("#d8dee9".to_owned()),
                background: Some("#20242c".to_owned()),
                dpi: Some(DEFAULT_PROFILE_DPI),
            }),
            semantic: Some(SemanticVisualProfile {
                preset: SemanticPreset::BundledV1,
            }),
        };
    }

    /// Semantic-only profile that needs no native tooling or host fonts.
    #[must_use]
    pub const fn semantic_bundled() -> Self {
        return Self {
            version: VISUAL_PROFILE_VERSION,
            native: None,
            semantic: Some(SemanticVisualProfile {
                preset: SemanticPreset::BundledV1,
            }),
        };
    }

    /// Validate every field and return the canonical form: lowercase colors and an explicit
    /// native DPI.
    ///
    /// # Errors
    ///
    /// Returns [`ModelValidationError::InvalidVisualProfile`] for an unsupported version, an
    /// empty profile, control characters, a font size that is not an exact positive decimal
    /// of at most 255 points, a malformed color, padding above 2000 points, a DPI outside
    /// 24..=960, or a relative config path.
    pub fn normalized(&self) -> Result<Self, ModelValidationError> {
        if self.version != VISUAL_PROFILE_VERSION {
            return Err(invalid(format!(
                "unsupported visual profile version {}; expected {VISUAL_PROFILE_VERSION}",
                self.version
            )));
        }

        if self.native.is_none() && self.semantic.is_none() {
            return Err(invalid(
                "visual profile must select a native or semantic section",
            ));
        }
        return Ok(Self {
            version: self.version,
            native: self
                .native
                .as_ref()
                .map(NativeVisualProfile::normalized)
                .transpose()?,
            semantic: self.semantic,
        });
    }

    /// BLAKE3 of the compact JSON of the normalized profile.
    ///
    /// # Errors
    ///
    /// Returns the normalization error.
    pub fn identity(&self) -> Result<String, ModelValidationError> {
        let normalized = self.normalized()?;
        let bytes = serde_json::to_vec(&normalized)
            .map_err(|error| return invalid(format!("serialize visual profile: {error}")))?;

        return Ok(blake3::hash(&bytes).to_hex().to_string());
    }

    /// Normalized profile bound to its identity.
    ///
    /// # Errors
    ///
    /// Returns the normalization error.
    pub fn record(&self) -> Result<RecordedVisualProfile, ModelValidationError> {
        return Ok(RecordedVisualProfile {
            profile: self.normalized()?,
            profile_hash: self.identity()?,
        });
    }
}

impl NativeVisualProfile {
    fn normalized(&self) -> Result<Self, ModelValidationError> {
        if let Some(config) = &self.config {
            text("config", config)?;
            if !config.starts_with('/') {
                return Err(invalid("native config must be an absolute path"));
            }
        }

        if let Some(family) = &self.font_family {
            text("font_family", family)?;
            if family.trim() != family {
                return Err(invalid(
                    "font_family must not have leading or trailing whitespace",
                ));
            }
        }

        if let Some(size) = &self.font_size_points {
            font_size(size)?;
        }

        if let Some(padding) = self.padding_points
            && (padding.x > MAX_PADDING_POINTS || padding.y > MAX_PADDING_POINTS)
        {
            return Err(invalid(format!(
                "padding_points must be at most {MAX_PADDING_POINTS} points per side"
            )));
        }

        let dpi = self.dpi.unwrap_or(DEFAULT_PROFILE_DPI);
        if !(MIN_DPI..=MAX_DPI).contains(&dpi) {
            return Err(invalid(format!(
                "dpi must be between {MIN_DPI} and {MAX_DPI}, got {dpi}"
            )));
        }
        return Ok(Self {
            config: self.config.clone(),
            font_family: self.font_family.clone(),
            font_size_points: self.font_size_points.clone(),
            padding_points: self.padding_points,
            foreground: self
                .foreground
                .as_deref()
                .map(|value| return normalize_rgb_color("foreground", value))
                .transpose()?,
            background: self
                .background
                .as_deref()
                .map(|value| return normalize_rgb_color("background", value))
                .transpose()?,
            dpi: Some(dpi),
        });
    }

    /// Focused Ghostty overrides applied after the base configuration.
    #[must_use]
    pub fn overrides(&self) -> GhosttyVisualOverrides {
        return GhosttyVisualOverrides {
            font_family: self.font_family.clone(),
            font_size: self.font_size_points.clone(),
            padding_x: self.padding_points.map(|padding| return padding.x),
            padding_y: self.padding_points.map(|padding| return padding.y),
            foreground: self.foreground.clone(),
            background: self.background.clone(),
        };
    }

    /// Resolved display DPI.
    #[must_use]
    pub fn resolved_dpi(&self) -> u32 {
        return self.dpi.unwrap_or(DEFAULT_PROFILE_DPI);
    }
}

/// Normalize a `#RRGGBB` color to lowercase.
///
/// # Errors
///
/// Returns [`ModelValidationError::InvalidVisualProfile`] for any other syntax.
pub fn normalize_rgb_color(field: &str, value: &str) -> Result<String, ModelValidationError> {
    let hex = value
        .strip_prefix('#')
        .filter(|hex| {
            return hex.len() == 6 && hex.bytes().all(|byte| return byte.is_ascii_hexdigit());
        })
        .ok_or_else(|| return invalid(format!("{field} must use #RRGGBB syntax")))?;

    return Ok(format!("#{}", hex.to_ascii_lowercase()));
}

fn font_size(value: &str) -> Result<(), ModelValidationError> {
    let malformed = || {
        return invalid(format!(
            "font_size_points must be an exact positive decimal such as \"15\" or \"13.5\", got `{value}`"
        ));
    };

    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if whole.is_empty()
        || !whole.bytes().all(|byte| return byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| return byte.is_ascii_digit())
        || (whole.len() > 1 && whole.starts_with('0'))
        || (value.contains('.') && (fraction.is_empty() || fraction.ends_with('0')))
        || whole.len() > 3
    {
        return Err(malformed());
    }

    let whole = whole
        .parse::<u32>()
        .map_err(|error| return invalid(format!("font_size_points: {error}")))?;

    if whole == 0 && fraction.is_empty() {
        return Err(invalid("font_size_points must be greater than zero"));
    }

    if whole > MAX_FONT_SIZE_POINTS || (whole == MAX_FONT_SIZE_POINTS && !fraction.is_empty()) {
        return Err(invalid(format!(
            "font_size_points must be at most {MAX_FONT_SIZE_POINTS}"
        )));
    }
    return Ok(());
}

fn text(field: &str, value: &str) -> Result<(), ModelValidationError> {
    if value.is_empty() || value.len() > MAX_TEXT_BYTES {
        return Err(invalid(format!(
            "{field} must contain 1 to {MAX_TEXT_BYTES} bytes"
        )));
    }

    if value.chars().any(char::is_control) {
        return Err(invalid(format!(
            "{field} must not contain control characters or line breaks"
        )));
    }
    return Ok(());
}

fn invalid(reason: impl Into<String>) -> ModelValidationError {
    return ModelValidationError::InvalidVisualProfile {
        reason: reason.into(),
    };
}

/// Normalized profile committed into a canonical session manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecordedVisualProfile {
    /// Normalized selected profile.
    pub profile: VisualProfile,
    /// BLAKE3 of the compact JSON of `profile`.
    pub profile_hash: String,
}

impl RecordedVisualProfile {
    /// Verify that the recorded profile is normalized and matches its identity.
    ///
    /// # Errors
    ///
    /// Returns [`ModelValidationError::InvalidVisualProfile`] when the profile is invalid, not
    /// in normalized form, or its hash does not match.
    pub fn validate(&self) -> Result<(), ModelValidationError> {
        if self.profile.normalized()? != self.profile {
            return Err(invalid("recorded visual profile is not normalized"));
        }

        if self.profile.identity()? != self.profile_hash {
            return Err(invalid(
                "recorded visual profile hash does not match the profile",
            ));
        }
        return Ok(());
    }
}

/// Stable visual-profile failure categories shared by every SDK.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VisualProfileFailure {
    /// The profile request itself is malformed or unsupported.
    InvalidVisualProfile,
    /// Native tooling needed by a native profile is not installed or did not answer.
    NativeProfileUnavailable,
    /// Ghostty reported diagnostics for the source or resolved configuration.
    GhosttyConfigInvalid,
    /// A requested font family is not installed under its exact name.
    FontUnavailable,
    /// An explicit viewport does not produce the required grid.
    ViewportMismatch,
    /// Native resolution failed for another reason.
    NativeProfileFailed,
}

impl VisualProfileFailure {
    /// Stable code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidVisualProfile => return "invalid_visual_profile",
            Self::NativeProfileUnavailable => return "native_profile_unavailable",
            Self::GhosttyConfigInvalid => return "ghostty_config_invalid",
            Self::FontUnavailable => return "font_unavailable",
            Self::ViewportMismatch => return "viewport_mismatch",
            Self::NativeProfileFailed => return "native_profile_failed",
        }
    }
}

/// Result of validating a visual profile before launch.
///
/// The report holds settings and resource identities only, never local paths or timestamps.
/// It is advisory: a later launch resolves the profile again.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VisualProfileReport {
    /// Report version; exactly 1.
    pub version: u32,
    /// Normalized profile.
    pub profile: VisualProfile,
    /// BLAKE3 identity of `profile`.
    pub profile_hash: String,
    /// Bundled semantic resources, when the semantic section is selected.
    pub semantic: Option<SemanticImageResource>,
    /// Native resolution facts, when the native section is selected.
    pub native: Option<NativeProfileReport>,
    /// What a launch with this profile guarantees.
    pub guarantees: Vec<String>,
    /// What it does not guarantee.
    pub limitations: Vec<String>,
}

/// Native facts observed while resolving a native profile.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NativeProfileReport {
    /// Installed Ghostty version report.
    pub ghostty_version: String,
    /// BLAKE3 of the Ghostty executable that resolved the profile.
    pub ghostty_executable_blake3: String,
    /// BLAKE3 of the exact replay-profile input bytes.
    pub replay_profile_hash: String,
    /// BLAKE3 of the exact effective configuration reported by Ghostty.
    pub effective_config_hash: String,
    /// Font families named by the effective configuration.
    pub requested_font_families: Vec<String>,
    /// Requested families Ghostty discovered under their exact names.
    pub discovered_font_families: Vec<String>,
    /// Display DPI used for the geometry probe and every later native launch.
    pub dpi: u32,
    /// Exact-grid viewport search at that DPI.
    pub geometry: GeometrySolveReport,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn native(fields: serde_json::Value) -> Result<VisualProfile, String> {
        let profile: VisualProfile =
            serde_json::from_value(json!({"version": 1, "native": fields}))
                .map_err(|error| return error.to_string())?;
        return profile
            .normalized()
            .map_err(|error| return error.to_string());
    }

    #[test]
    fn decoding_rejects_unknown_fields_and_semantic_customization() {
        for value in [
            json!({"version": 1, "semantic": {"preset": "bundled-v1"}, "theme": "dark"}),
            json!({"version": 1, "native": {"font": "Example Mono"}}),
            json!({"version": 1, "semantic": {"preset": "bundled-v1", "font_family": "Example Mono"}}),
            json!({"version": 1, "semantic": {"preset": "bundled-v2"}}),
            json!({"version": 1, "native": {"padding_points": {"x": 1, "y": 1, "top": 1}}}),
            json!({"version": 1, "native": {"font_size_points": 15}}),
        ] {
            assert!(
                serde_json::from_value::<VisualProfile>(value.clone()).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn normalization_rejects_unsupported_versions_and_empty_profiles() {
        for value in [
            json!({"version": 2, "semantic": {"preset": "bundled-v1"}}),
            json!({"version": 0, "semantic": {"preset": "bundled-v1"}}),
            json!({"version": 1}),
        ] {
            let profile: VisualProfile = serde_json::from_value(value.clone()).expect("decodes");
            assert!(profile.normalized().is_err(), "{value}");
        }
    }

    #[test]
    fn font_size_must_be_an_exact_positive_bounded_decimal() {
        for accepted in ["15", "13.5", "0.5", "255", "7.25"] {
            assert!(
                native(json!({"font_size_points": accepted})).is_ok(),
                "{accepted}"
            );
        }

        for rejected in [
            "NaN", "nan", "inf", "0", "0.0", "-1", "+1", "1e3", "015", "15.", ".5", "15.50", "256",
            "255.5", "1000", "", " 15", "15 ", "1_5", "١٥",
        ] {
            assert!(
                native(json!({"font_size_points": rejected})).is_err(),
                "{rejected:?}"
            );
        }
    }

    #[test]
    fn text_fields_reject_config_line_injection() {
        for family in [
            "Example Mono\nbackground = #000000",
            "Example Mono\r",
            "Example\u{0}Mono",
            "Example\tMono",
            "",
            " Example Mono",
        ] {
            assert!(
                native(json!({"font_family": family})).is_err(),
                "{family:?}"
            );
        }

        assert!(native(json!({"config": "/configs/a\nfont-size = 99"})).is_err());
        assert!(native(json!({"config": "relative/config.ghostty"})).is_err());
        assert!(native(json!({"font_family": "Example \"Quoted\" Mono"})).is_ok());
    }

    #[test]
    fn colors_normalize_to_lowercase_and_reject_other_syntax() {
        let profile = native(json!({"foreground": "#D8DEE9", "background": "#20242C"}))
            .expect("valid colors");
        let section = profile.native.expect("native section");

        assert_eq!(section.foreground.as_deref(), Some("#d8dee9"));
        assert_eq!(section.background.as_deref(), Some("#20242c"));

        for color in ["#fff", "d8dee9", "#d8dee9ff", "#gggggg", "red", "#d8dee\n"] {
            assert!(native(json!({"foreground": color})).is_err(), "{color:?}");
        }
    }

    #[test]
    fn dpi_and_padding_are_bounded_and_dpi_defaults_explicitly() {
        let profile = native(json!({})).expect("empty native section");
        assert_eq!(
            profile.native.and_then(|native| return native.dpi),
            Some(96)
        );

        for dpi in [0, 23, 961] {
            assert!(native(json!({"dpi": dpi})).is_err(), "{dpi}");
        }

        assert!(native(json!({"dpi": 144})).is_ok());
        assert!(native(json!({"padding_points": {"x": 0, "y": 0}})).is_ok());
        assert!(native(json!({"padding_points": {"x": 2001, "y": 0}})).is_err());
        assert!(native(json!({"padding_points": {"x": -1, "y": 0}})).is_err());
    }

    #[test]
    fn identity_is_independent_of_input_spelling_but_not_of_settings() {
        let upper: VisualProfile = serde_json::from_value(json!({
            "native": {"background": "#20242C", "font_family": "Example Mono"},
            "version": 1
        }))
        .expect("decodes");

        let lower: VisualProfile = serde_json::from_value(json!({
            "version": 1,
            "native": {"font_family": "Example Mono", "background": "#20242c", "dpi": 96}
        }))
        .expect("decodes");

        let zero_padding: VisualProfile = serde_json::from_value(json!({
            "version": 1,
            "native": {"font_family": "Example Mono", "background": "#20242c", "padding_points": {"x": 0, "y": 0}}
        }))
        .expect("decodes");

        assert_eq!(upper.identity().unwrap(), lower.identity().unwrap());
        assert_ne!(upper.identity().unwrap(), zero_padding.identity().unwrap());
        assert_ne!(
            VisualProfile::semantic_bundled().identity().unwrap(),
            VisualProfile::native_standard().identity().unwrap()
        );
    }

    #[test]
    fn shared_sdk_vectors_normalize_hash_and_fail_identically() {
        let vectors: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../fixtures/conformance/sdk/visual-profiles.json"
        ))
        .expect("visual profile vectors");

        for vector in vectors["positive"].as_array().expect("positive vectors") {
            let profile: VisualProfile =
                serde_json::from_value(vector["profile"].clone()).expect("decodes");
            let recorded = profile.record().expect("valid profile");
            assert_eq!(
                serde_json::to_value(&recorded.profile).expect("normalized JSON"),
                vector["normalized"],
                "{}",
                vector["name"]
            );
            assert_eq!(
                recorded.profile_hash, vector["profile_hash"],
                "{}",
                vector["name"]
            );
        }

        for vector in vectors["invalid"].as_array().expect("invalid vectors") {
            let profile: VisualProfile =
                serde_json::from_value(vector["profile"].clone()).expect("decodes");
            assert!(profile.normalized().is_err(), "{}", vector["name"]);
        }

        for vector in vectors["malformed"].as_array().expect("malformed vectors") {
            let error = serde_json::from_value::<VisualProfile>(vector["profile"].clone())
                .expect_err("malformed profile must not decode");
            let expected = vector["message"].as_str().expect("message");
            assert!(
                error.to_string().contains(expected),
                "{}: {error}",
                vector["name"]
            );
        }
    }

    #[test]
    fn recorded_profile_rejects_tampering() {
        let recorded = VisualProfile::native_standard().record().expect("valid");
        recorded.validate().expect("recorded profile verifies");

        let mut changed_setting = recorded.clone();
        if let Some(native) = changed_setting.profile.native.as_mut() {
            native.font_size_points = Some("16".to_owned());
        }

        assert!(changed_setting.validate().is_err());

        let mut changed_hash = recorded.clone();
        changed_hash.profile_hash = "0".repeat(64);
        assert!(changed_hash.validate().is_err());

        let mut denormalized = recorded;
        if let Some(native) = denormalized.profile.native.as_mut() {
            native.foreground = Some("#D8DEE9".to_owned());
        }

        assert!(denormalized.validate().is_err());
    }
}
