//! Additive SDK transport contracts. Never used to serialize canonical evidence.

use crate::{
    artifact::metadata::{ArtifactMember, ArtifactSensitivity},
    execution::event_log::CanonicalBoundary,
    value::ByteString,
};
use schemars::JsonSchema;
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, collections::BTreeSet};

/// Inert JSON Schema keyword naming the native unsigned integer a decimal string carries
/// (`uint64` or `uint128`). Native projections and generated host types read it; validators
/// ignore it, and the portable string form is unchanged.
pub const NATIVE_INTEGER_ANNOTATION: &str = "x-termtrace-native-integer";

// Exact lexical bound as well as canonical decimal syntax; JSON Schema must reject overflow.
fn decimal_pattern(max: &str) -> String {
    let mut alternatives = vec!["0".to_owned(), format!("[1-9][0-9]{{0,{}}}", max.len() - 2)];
    let mut prefix = String::new();

    for (i, digit) in max.bytes().enumerate() {
        let low = if i == 0 { b'1' } else { b'0' };
        if digit > low {
            alternatives.push(format!(
                "{}[{}-{}][0-9]{{{}}}",
                prefix,
                char::from(low),
                char::from(digit - 1),
                max.len() - i - 1
            ));
        }

        prefix.push(char::from(digit));
    }

    alternatives.push(max.to_owned());

    return format!("^(?:{})$", alternatives.join("|"));
}

struct RelativePathSchema;

impl JsonSchema for RelativePathSchema {
    fn schema_name() -> Cow<'static, str> {
        return "ArtifactRelativePath".into();
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        return schemars::json_schema!({
            "type": "string", "pattern": "^[^/\\\\:\\u0000]+(?:/[^/\\\\:\\u0000]+)*$",
            "not": { "pattern": "(^|/)\\.{1,2}(/|$)" }
        });
    }
}

struct NullableStringSchema;

impl JsonSchema for NullableStringSchema {
    fn schema_name() -> Cow<'static, str> {
        return "NullableString".into();
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        return schemars::json_schema!({"type": ["string", "null"]});
    }
}

fn required_nullable_string<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<String>, D::Error> {
    return Option::<String>::deserialize(d);
}

macro_rules! decimal {
    ($name:ident, $int:ty, $max:literal, $native:literal) => {
        #[doc = "Exact unsigned SDK integer, serialized only as a canonical decimal string."]
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub struct $name(pub $int);
        impl Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                return s.serialize_newtype_struct(crate::value::DECIMAL_MARKER, &self.0.to_string());
            }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                if s.is_empty() || !s.bytes().all(|b| return b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0')) {
                    return Err(D::Error::custom("expected canonical unsigned decimal string"));
                }
                return s.parse::<$int>().map(Self).map_err(D::Error::custom);
            }
        }
        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> { return stringify!($name).into(); }
            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                return schemars::json_schema!({"type":"string", "pattern":decimal_pattern(&<$int>::MAX.to_string()), "maxLength":$max, (NATIVE_INTEGER_ANNOTATION):$native});
            }
        }
    }
}
decimal!(DecimalU64, u64, 20, "uint64");
decimal!(DecimalU128, u128, 39, "uint128");

/// Non-derived source truth; not a replacement for manifest truth-layer strings.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactSourceKind {
    /// Reconstructed terminal state rendered semantically.
    SemanticRendering,
    /// Pixels rendered while replaying canonical evidence.
    NativeReplay,
    /// Pixels captured during the original execution.
    OriginalExecution,
}

/// Product provenance, with presentation explicitly retaining its source truth.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArtifactProvenance {
    /// Semantic reconstruction, not original pixels.
    SemanticRendering,
    /// Native replay pixels, not original execution.
    NativeReplay,
    /// Original execution capture.
    OriginalExecution,
    /// Derived presentation; authoritative mapping remains in its manifest.
    PresentationOutput {
        /// Truth of the input to presentation.
        source_kind: ArtifactSourceKind,
    },
}

/// Explicit output kind, including previews rather than inferring from extensions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactProductKind {
    /// Saved state.
    Snapshot,
    /// Moving output.
    Video,
    /// Replay delivery.
    Playback,
    /// Presentation derivative.
    Presentation,
    /// Preview derivative.
    Preview,
}

/// Exact manifest reference, relative to the artifact directory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactManifestRef {
    /// Normalized relative filename.
    #[schemars(with = "RelativePathSchema")]
    pub path: String,
    /// BLAKE3 of the exact manifest bytes.
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub blake3: String,
}

/// Binding to canonical source; no frame-exact original-capture correlation implied.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSourceRef {
    /// Canonical session UUID.
    pub session_id: uuid::Uuid,
    /// Hash of the source session manifest.
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub session_manifest_hash: String,
    /// Exact canonical prefix (header hash at sequence zero).
    pub boundary: SdkCanonicalBoundary,
}

/// Exact canonical boundary with portable wide integers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SdkCanonicalBoundary {
    /// Sequence assigned to the next event.
    pub next_event_seq: DecimalU64,
    /// Prior event hash, or the event header hash at sequence zero.
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub event_hash: String,
    /// Canonical PTY bytes through this boundary.
    pub output_bytes: DecimalU64,
}
impl From<CanonicalBoundary> for SdkCanonicalBoundary {
    fn from(b: CanonicalBoundary) -> Self {
        return Self {
            next_event_seq: DecimalU64(b.next_event_seq),
            event_hash: b.event_hash,
            output_bytes: DecimalU64(b.output_bytes),
        };
    }
}

/// Portable member metadata. Canonical `ArtifactMember` serialization is unchanged.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SdkArtifactMember {
    /// Relative filename.
    #[schemars(with = "RelativePathSchema")]
    pub path: String,
    /// Exact bytes in the complete file.
    pub byte_length: DecimalU64,
    /// Whole-file BLAKE3.
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub blake3: String,
    /// MIME type.
    pub media_type: String,
    /// Schema identifier, or explicit null.
    #[serde(deserialize_with = "required_nullable_string")]
    #[schemars(with = "NullableStringSchema")]
    pub schema_type: Option<String>,
    /// Existing sensitivity contract.
    pub sensitivity: ArtifactSensitivity,
    /// Existing manifest relationship.
    pub relationship: String,
}
impl From<ArtifactMember> for SdkArtifactMember {
    fn from(m: ArtifactMember) -> Self {
        return Self {
            path: m.path,
            byte_length: DecimalU64(m.byte_length),
            blake3: m.blake3,
            media_type: m.media_type,
            schema_type: m.schema_type,
            sensitivity: m.sensitivity,
            relationship: m.relationship,
        };
    }
}

/// Versioned SDK envelope. Data only; does not promise runtime capture/export support.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaArtifactResult {
    /// Exactly 1. Decoders must reject other versions.
    #[schemars(range(min = 1, max = 1))]
    pub version: u32,
    /// Exact OS directory bytes, standard padded Base64 in JSON.
    pub directory_bytes: ByteString,
    /// Manifest is authoritative for mappings and publication facts.
    pub manifest: ArtifactManifestRef,
    /// Canonical source boundary.
    pub source: ArtifactSourceRef,
    /// Product truth.
    pub provenance: ArtifactProvenance,
    /// Product classification.
    pub product_kind: ArtifactProductKind,
    /// Complete member metadata.
    pub members: Vec<SdkArtifactMember>,
}

// Keep plain serde callers as strict as the service transport codec. This private
// wire type is deliberately not used for canonical evidence or schema generation.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaArtifactWire {
    version: u32,
    directory_bytes: ByteString,
    manifest: ArtifactManifestRef,
    source: ArtifactSourceRef,
    provenance: ArtifactProvenance,
    product_kind: ArtifactProductKind,
    members: Vec<SdkArtifactMember>,
}
impl<'de> Deserialize<'de> for MediaArtifactResult {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = MediaArtifactWire::deserialize(deserializer)?;
        let result = Self {
            version: wire.version,
            directory_bytes: wire.directory_bytes,
            manifest: wire.manifest,
            source: wire.source,
            provenance: wire.provenance,
            product_kind: wire.product_kind,
            members: wire.members,
        };

        result.validate().map_err(D::Error::custom)?;

        return Ok(result);
    }
}

impl MediaArtifactResult {
    /// Validate semantic invariants after decoding and before encoding.
    ///
    /// # Errors
    /// Rejects unsupported versions, escaping paths, invalid hashes, duplicate members,
    /// empty directories, and contradictory product/provenance combinations.
    pub fn validate(&self) -> Result<(), String> {
        fn path(s: &str) -> bool {
            return !s.is_empty()
                && !s.contains(['\\', '\0'])
                && s.split('/')
                    .all(|p| return !p.is_empty() && p != "." && p != ".." && !p.contains(':'));
        }

        fn hash(s: &str) -> bool {
            return s.len() == 64
                && s.bytes()
                    .all(|b| return b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        }

        if self.version != 1 {
            return Err("unsupported SDK artifact version".into());
        }

        if self.directory_bytes.as_bytes().is_empty()
            || self.directory_bytes.as_bytes().contains(&0)
        {
            return Err("invalid directory bytes".into());
        }

        if !path(&self.manifest.path)
            || !hash(&self.manifest.blake3)
            || !hash(&self.source.session_manifest_hash)
        {
            return Err("invalid manifest reference".into());
        }

        if !hash(&self.source.boundary.event_hash) {
            return Err("invalid source boundary".into());
        }

        let presentation = matches!(
            self.provenance,
            ArtifactProvenance::PresentationOutput { .. }
        );

        if presentation
            != matches!(
                self.product_kind,
                ArtifactProductKind::Presentation | ArtifactProductKind::Preview
            )
        {
            return Err("contradictory product provenance".into());
        }

        let mut paths = BTreeSet::new();
        for m in &self.members {
            if !path(&m.path)
                || m.path == self.manifest.path
                || !paths.insert(&m.path)
                || !hash(&m.blake3)
                || m.media_type.is_empty()
                || m.relationship.is_empty()
            {
                return Err("invalid artifact member".into());
            }
        }

        return Ok(());
    }
}
