//! Build-time compression for bundled semantic font assets.

use std::env;
use std::error::Error;
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use zstd::stream::encode_all;

const FONT_BUNDLE_MAGIC: &[u8; 8] = b"TTFONT01";

const FONT_ASSETS: [&str; 5] = [
    "assets/fonts/JetBrainsMono-Regular.ttf",
    "assets/fonts/JetBrainsMono-Bold.ttf",
    "assets/fonts/JetBrainsMono-Italic.ttf",
    "assets/fonts/JetBrainsMono-BoldItalic.ttf",
    "assets/fonts/SymbolsNerdFont-Regular.ttf",
];

fn main() -> Result<(), Box<dyn Error>> {
    let output_directory = PathBuf::from(env::var_os("OUT_DIR").ok_or("OUT_DIR is not set")?);
    let font_count = u16::try_from(FONT_ASSETS.len())?;

    let mut fonts: Vec<Vec<u8>> = Vec::with_capacity(FONT_ASSETS.len());

    for asset in FONT_ASSETS {
        println!("cargo:rerun-if-changed={asset}");
        fonts.push(fs::read(asset)?);
    }

    let mut bundle = Vec::new();
    bundle.extend_from_slice(FONT_BUNDLE_MAGIC);
    bundle.extend_from_slice(&font_count.to_le_bytes());

    for font in &fonts {
        bundle.extend_from_slice(&u64::try_from(font.len())?.to_le_bytes());
    }

    for font in fonts {
        bundle.extend_from_slice(&font);
    }

    let compressed = encode_all(Cursor::new(bundle), 19)?;
    fs::write(
        output_directory.join("semantic-font-set.bundle.zst"),
        compressed,
    )?;

    return Ok(());
}
