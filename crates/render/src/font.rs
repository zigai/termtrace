//! Identity, validation, and loading for bundled deterministic semantic fonts.

use crate::error::{render_error, Error, Result};
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use resvg::usvg::fontdb::{Database, Source};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Cursor;
use std::ops::Range;
use std::sync::{Arc, LazyLock};
use termtrace_model::artifact::save::{SemanticFontResource, SemanticFontRole};
use ttf_parser::{name_id, Face};
use zstd::stream::decode_all;

/// Termtrace-scoped family for all JetBrains Mono text faces.
pub const SEMANTIC_TEXT_FONT_FAMILY: &str = "Termtrace JetBrains Mono";
/// Termtrace-scoped family for the Symbols Nerd Font fallback.
pub const SEMANTIC_SYMBOLS_FONT_FAMILY: &str = "Termtrace Symbols Nerd Font";
/// Embedded font-family stack prioritizing JetBrains Mono with Symbols Nerd Font fallback.
pub const SEMANTIC_TEXT_FONT_STACK: &str =
    "'Termtrace JetBrains Mono', 'Termtrace Symbols Nerd Font', monospace";
/// Embedded font-family stack prioritizing Symbols Nerd Font with JetBrains Mono fallback.
pub const SEMANTIC_SYMBOLS_FONT_STACK: &str =
    "'Termtrace Symbols Nerd Font', 'Termtrace JetBrains Mono', monospace";
pub(crate) const SEMANTIC_RASTER_TEXT_FONT_FAMILY: &str = "JetBrains Mono";
pub(crate) const SEMANTIC_RASTER_TEXT_FONT_STACK: &str =
    "'JetBrains Mono', 'Symbols Nerd Font', monospace";
pub(crate) const SEMANTIC_RASTER_SYMBOLS_FONT_STACK: &str =
    "'Symbols Nerd Font', 'JetBrains Mono', monospace";
const FONT_SET_HASH_DOMAIN: &[u8] = b"termtrace-semantic-font-set-v1\0";
const FONT_BUNDLE_MAGIC: &[u8; 8] = b"TTFONT01";
const FONT_COUNT: u16 = 5;
const FONT_BUNDLE_COMPRESSED: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/semantic-font-set.bundle.zst"));

struct BundledFontSpec {
    role: SemanticFontRole,
    family: &'static str,
    repository_source: &'static str,
    expected_family: &'static str,
    expected_style: &'static str,
}

struct DecompressedFontBundle {
    bytes: Arc<Vec<u8>>,
    ranges: Vec<Range<usize>>,
}

struct FontBytes {
    bundle: Arc<Vec<u8>>,
    range: Range<usize>,
}

impl FontBytes {
    #[expect(
        clippy::indexing_slicing,
        reason = "bundle ranges are validated before FontBytes construction"
    )]
    fn bytes(&self) -> &[u8] {
        return &self.bundle[self.range.clone()];
    }
}

impl AsRef<[u8]> for FontBytes {
    fn as_ref(&self) -> &[u8] {
        return self.bytes();
    }
}

struct SemanticFontFace {
    resource: SemanticFontResource,
    bytes: Arc<FontBytes>,
    coverage: HashSet<u32>,
}

impl SemanticFontFace {
    fn supports_grapheme(&self, grapheme: &str) -> bool {
        let mut meaningful = grapheme
            .chars()
            .filter(|c| return !is_variation_selector(*c));
        let Some(first) = meaningful.next() else {
            return false;
        };

        return self.coverage.contains(&u32::from(first))
            && meaningful.all(|character| return self.coverage.contains(&u32::from(character)));
    }
}

pub(crate) fn is_variation_selector(character: char) -> bool {
    return matches!(
        character,
        '\u{fe00}'..='\u{fe0f}' | '\u{e0100}'..='\u{e01ef}'
    );
}

/// Lazily initialized immutable set of bundled semantic font faces.
pub struct SemanticFontSet {
    faces: [SemanticFontFace; 5],
    font_set_blake3: String,
    font_faces_css: Arc<str>,
    database: Arc<Database>,
}

impl SemanticFontSet {
    /// Ordered exact resource metadata in serialized role order.
    pub fn resources(&self) -> impl ExactSizeIterator<Item = &SemanticFontResource> {
        return self.faces.iter().map(|face| return &face.resource);
    }

    /// Aggregate BLAKE3 identity of all decompressed faces and their roles.
    #[must_use]
    pub fn font_set_blake3(&self) -> &str {
        return &self.font_set_blake3;
    }

    /// Precomputed `@font-face` CSS style block for embedding all bundled fonts in SVG.
    #[must_use]
    pub fn font_faces_css(&self) -> &str {
        return &self.font_faces_css;
    }

    /// Decompressed JetBrains Mono Regular bytes.
    #[must_use]
    pub fn regular_face_bytes(&self) -> &[u8] {
        let [regular, ..] = &self.faces;

        return regular.bytes.bytes();
    }

    pub(crate) fn database(&self) -> &Arc<Database> {
        return &self.database;
    }

    /// Decompressed bytes for the requested semantic font role.
    #[must_use]
    pub fn face_bytes(&self, role: SemanticFontRole) -> &[u8] {
        return self.face(role).bytes.bytes();
    }

    pub(crate) fn supports_grapheme(&self, role: SemanticFontRole, grapheme: &str) -> bool {
        return self.face(role).supports_grapheme(grapheme);
    }

    fn face(&self, role: SemanticFontRole) -> &SemanticFontFace {
        let [regular, bold, italic, bold_italic, symbols] = &self.faces;

        match role {
            SemanticFontRole::Regular => return regular,
            SemanticFontRole::Bold => return bold,
            SemanticFontRole::Italic => return italic,
            SemanticFontRole::BoldItalic => return bold_italic,
            SemanticFontRole::SymbolsFallback => return symbols,
        }
    }
}

/// Return the process-wide bundled semantic font set.
///
/// Initialization decompresses, validates, hashes, and registers every face
/// once. The immutable success or failure is cached for the process lifetime.
///
/// # Errors
///
/// Returns a rendering error when a bundled face cannot be decompressed or
/// does not have the required family and style metadata.
pub fn semantic_font_set() -> Result<&'static SemanticFontSet> {
    static FONT_SET: LazyLock<Result<SemanticFontSet>> = LazyLock::new(load_font_set);

    return match &*FONT_SET {
        Ok(font_set) => Ok(font_set),
        Err(error) => Err(error.clone()),
    };
}

fn load_font_set() -> Result<SemanticFontSet> {
    let DecompressedFontBundle {
        bytes: bundle,
        ranges,
    } = decompress_font_bundle()?;

    let [regular_range, bold_range, italic_range, bold_italic_range, symbols_range] =
        ranges.as_slice()
    else {
        return Err(invalid_font_bundle());
    };
    let faces = [
        load_font(
            BundledFontSpec {
                role: SemanticFontRole::Regular,
                family: SEMANTIC_TEXT_FONT_FAMILY,
                repository_source: "crates/render/assets/fonts/JetBrainsMono-Regular.ttf",
                expected_family: "JetBrains Mono",
                expected_style: "Regular",
            },
            Arc::clone(&bundle),
            regular_range.clone(),
        )?,
        load_font(
            BundledFontSpec {
                role: SemanticFontRole::Bold,
                family: SEMANTIC_TEXT_FONT_FAMILY,
                repository_source: "crates/render/assets/fonts/JetBrainsMono-Bold.ttf",
                expected_family: "JetBrains Mono",
                expected_style: "Bold",
            },
            Arc::clone(&bundle),
            bold_range.clone(),
        )?,
        load_font(
            BundledFontSpec {
                role: SemanticFontRole::Italic,
                family: SEMANTIC_TEXT_FONT_FAMILY,
                repository_source: "crates/render/assets/fonts/JetBrainsMono-Italic.ttf",
                expected_family: "JetBrains Mono",
                expected_style: "Italic",
            },
            Arc::clone(&bundle),
            italic_range.clone(),
        )?,
        load_font(
            BundledFontSpec {
                role: SemanticFontRole::BoldItalic,
                family: SEMANTIC_TEXT_FONT_FAMILY,
                repository_source: "crates/render/assets/fonts/JetBrainsMono-BoldItalic.ttf",
                expected_family: "JetBrains Mono",
                expected_style: "Bold Italic",
            },
            Arc::clone(&bundle),
            bold_italic_range.clone(),
        )?,
        load_font(
            BundledFontSpec {
                role: SemanticFontRole::SymbolsFallback,
                family: SEMANTIC_SYMBOLS_FONT_FAMILY,
                repository_source: "crates/render/assets/fonts/SymbolsNerdFont-Regular.ttf",
                expected_family: "Symbols Nerd Font",
                expected_style: "Regular",
            },
            Arc::clone(&bundle),
            symbols_range.clone(),
        )?,
    ];

    let mut hasher = blake3::Hasher::new();
    hasher.update(FONT_SET_HASH_DOMAIN);

    let mut database = Database::new();
    for face in &faces {
        let role = face.resource.role.as_str().as_bytes();
        let role_length = u16::try_from(role.len())
            .map_err(|error| return render_error(format!("hash semantic font role: {error}")))?;

        let font_length = u64::try_from(face.bytes.bytes().len())
            .map_err(|error| return render_error(format!("hash semantic font size: {error}")))?;

        hasher.update(&role_length.to_le_bytes());
        hasher.update(role);
        hasher.update(&font_length.to_le_bytes());
        hasher.update(face.bytes.bytes());

        let font_data = Arc::<FontBytes>::clone(&face.bytes);
        let font_data: Arc<dyn AsRef<[u8]> + Send + Sync> = font_data;
        database.load_font_source(Source::Binary(font_data));
    }

    let font_faces_css = precompute_font_faces_css(&faces)?;

    return Ok(SemanticFontSet {
        faces,
        font_set_blake3: hasher.finalize().to_hex().to_string(),
        font_faces_css,
        database: Arc::new(database),
    });
}

fn precompute_font_faces_css(faces: &[SemanticFontFace; 5]) -> Result<Arc<str>> {
    let mut total_capacity: usize = 32;
    for face in faces {
        let bytes_len = face.bytes.bytes().len();
        let base64_len = bytes_len
            .saturating_add(2)
            .saturating_div(3)
            .saturating_mul(4);
        total_capacity = total_capacity
            .saturating_add(base64_len)
            .saturating_add(160);
    }

    let mut css = String::with_capacity(total_capacity);
    css.push_str("<style>");

    for face in faces {
        let role = face.resource.role;
        let (family, weight, style) = embedded_font_attributes(role);
        write!(
            css,
            "@font-face{{font-family:'{family}';src:url(data:font/ttf;base64,"
        )?;
        STANDARD.encode_string(face.bytes.bytes(), &mut css);
        write!(
            css,
            ") format('truetype');font-weight:{weight};font-style:{style}}}"
        )?;
    }

    css.push_str("</style>\n");

    return Ok(Arc::from(css));
}

fn embedded_font_attributes(role: SemanticFontRole) -> (&'static str, &'static str, &'static str) {
    match role {
        SemanticFontRole::Regular => {
            return (SEMANTIC_TEXT_FONT_FAMILY, "normal", "normal");
        }
        SemanticFontRole::Bold => {
            return (SEMANTIC_TEXT_FONT_FAMILY, "bold", "normal");
        }
        SemanticFontRole::Italic => {
            return (SEMANTIC_TEXT_FONT_FAMILY, "normal", "italic");
        }
        SemanticFontRole::BoldItalic => {
            return (SEMANTIC_TEXT_FONT_FAMILY, "bold", "italic");
        }
        SemanticFontRole::SymbolsFallback => {
            return (SEMANTIC_SYMBOLS_FONT_FAMILY, "normal", "normal");
        }
    }
}

fn load_font(
    spec: BundledFontSpec,
    bundle: Arc<Vec<u8>>,
    range: Range<usize>,
) -> Result<SemanticFontFace> {
    let bytes = Arc::new(FontBytes { bundle, range });
    let face = Face::parse(bytes.bytes(), 0).map_err(|source| {
        return Error::FontFace {
            reason: format!(
                "bundled semantic font {} has unexpected family/style metadata",
                spec.role.as_str()
            ),
            source,
        };
    })?;

    if !has_name(&face, name_id::FAMILY, spec.expected_family)
        || !has_name(&face, name_id::SUBFAMILY, spec.expected_style)
    {
        return Err(render_error(format!(
            "bundled semantic font {} has unexpected family/style metadata",
            spec.role.as_str()
        )));
    }

    validate_layout_metrics(spec.role, &face)?;

    let mut coverage = HashSet::new();

    if let Some(cmap) = face.tables().cmap {
        for subtable in cmap.subtables {
            if !subtable.is_unicode() {
                continue;
            }

            subtable.codepoints(|codepoint| {
                if let Some(character) = char::from_u32(codepoint)
                    && face.glyph_index(character).is_some()
                {
                    coverage.insert(codepoint);
                }
            });
        }
    }

    let decompressed_size = u64::try_from(bytes.bytes().len())
        .map_err(|error| return render_error(format!("measure semantic font: {error}")))?;

    let resource = SemanticFontResource {
        role: spec.role,
        family: spec.family.to_owned(),
        repository_source: spec.repository_source.to_owned(),
        decompressed_size,
        decompressed_blake3: blake3::hash(bytes.bytes()).to_hex().to_string(),
    };

    return Ok(SemanticFontFace {
        resource,
        bytes,
        coverage,
    });
}

fn decompress_font_bundle() -> Result<DecompressedFontBundle> {
    let bundle = decode_all(Cursor::new(FONT_BUNDLE_COMPRESSED)).map_err(|error| {
        return render_error(format!("decompress bundled semantic font set: {error}"));
    })?;

    let mut cursor = 0;
    if take_bundle_bytes(&bundle, &mut cursor, FONT_BUNDLE_MAGIC.len())
        != Some(FONT_BUNDLE_MAGIC.as_slice())
        || read_bundle_u16(&bundle, &mut cursor) != Some(FONT_COUNT)
    {
        return Err(invalid_font_bundle());
    }

    let mut lengths: Vec<usize> = Vec::with_capacity(usize::from(FONT_COUNT));
    for _ in 0..FONT_COUNT {
        let length = read_bundle_u64(&bundle, &mut cursor)
            .and_then(|length| return usize::try_from(length).ok())
            .ok_or_else(invalid_font_bundle)?;
        lengths.push(length);
    }

    let mut ranges: Vec<Range<usize>> = Vec::with_capacity(lengths.len());
    let mut offset = cursor;

    for length in lengths {
        let end = offset.checked_add(length).ok_or_else(invalid_font_bundle)?;
        if end > bundle.len() {
            return Err(invalid_font_bundle());
        }

        ranges.push(offset..end);
        offset = end;
    }

    if offset != bundle.len() {
        return Err(invalid_font_bundle());
    }
    return Ok(DecompressedFontBundle {
        bytes: Arc::new(bundle),
        ranges,
    });
}

fn take_bundle_bytes<'bundle>(
    bundle: &'bundle [u8],
    cursor: &mut usize,
    length: usize,
) -> Option<&'bundle [u8]> {
    let end = cursor.checked_add(length)?;
    let bytes = bundle.get(*cursor..end)?;
    *cursor = end;

    return Some(bytes);
}

fn read_bundle_u16(bundle: &[u8], cursor: &mut usize) -> Option<u16> {
    let bytes: [u8; 2] = take_bundle_bytes(bundle, cursor, 2)?.try_into().ok()?;

    return Some(u16::from_le_bytes(bytes));
}

fn read_bundle_u64(bundle: &[u8], cursor: &mut usize) -> Option<u64> {
    let bytes: [u8; 8] = take_bundle_bytes(bundle, cursor, 8)?.try_into().ok()?;

    return Some(u64::from_le_bytes(bytes));
}

fn invalid_font_bundle() -> crate::Error {
    return render_error("bundled semantic font set has an invalid index");
}

fn has_name(face: &Face<'_>, name_id: u16, expected: &str) -> bool {
    return face.names().into_iter().any(|name| {
        return name.name_id == name_id
            && name.is_unicode()
            && name.to_string().as_deref() == Some(expected);
    });
}

fn validate_layout_metrics(role: SemanticFontRole, face: &Face<'_>) -> Result<()> {
    let is_valid = match role {
        SemanticFontRole::Regular => {
            let advance = face
                .glyph_index('A')
                .and_then(|glyph| return face.glyph_hor_advance(glyph));

            face.units_per_em() == 1_000 && advance == Some(600)
        }
        SemanticFontRole::SymbolsFallback => {
            face.units_per_em() == 2_048 && face.ascender() == 1_638 && face.descender() == -410
        }
        SemanticFontRole::Bold | SemanticFontRole::Italic | SemanticFontRole::BoldItalic => true,
    };

    if !is_valid {
        return Err(render_error(format!(
            "bundled semantic font {} has unexpected layout metrics",
            role.as_str()
        )));
    }
    return Ok(());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_fonts_preserve_load_bearing_layout_metrics() {
        let font_set = semantic_font_set().expect("initialize bundled semantic font set");
        let regular = Face::parse(font_set.face_bytes(SemanticFontRole::Regular), 0)
            .expect("parse regular bundled face");
        let regular_a = regular.glyph_index('A').expect("regular face contains A");

        assert_eq!(regular.units_per_em(), 1_000);
        assert_eq!(regular.glyph_hor_advance(regular_a), Some(600));

        let symbols = Face::parse(font_set.face_bytes(SemanticFontRole::SymbolsFallback), 0)
            .expect("parse symbols bundled face");
        assert_eq!(symbols.units_per_em(), 2_048);
        assert_eq!(symbols.ascender(), 1_638);
        assert_eq!(symbols.descender(), -410);
    }
}
