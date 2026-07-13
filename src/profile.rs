//! Capture terminal host defaults needed for deterministic semantic replay.

use std::process::Command;

use crate::{
    Error, Result,
    event::TerminalColors,
    ghostty_vt::{GhosttyVt, Rgb},
};

/// Read the effective Ghostty colors, falling back to pinned libghostty defaults.
///
/// # Errors
///
/// Returns an error only if the pinned semantic engine cannot expose defaults or
/// if a successful Ghostty configuration contains malformed explicit colors.
pub fn resolved_terminal_colors() -> Result<TerminalColors> {
    let mut colors = builtin_terminal_colors()?;
    let Ok(output) = Command::new("ghostty").arg("+show-config").output() else {
        tracing::warn!("Ghostty is unavailable; recording pinned libghostty default colors");
        return Ok(colors);
    };
    if !output.status.success() {
        tracing::warn!(status = %output.status, "Ghostty config query failed; recording pinned libghostty default colors");
        return Ok(colors);
    }
    apply_config(&mut colors, &output.stdout)?;
    colors.source = format!(
        "ghostty_show_config_blake3:{}",
        blake3::hash(&output.stdout)
    );
    Ok(colors)
}

/// Read the color defaults compiled into the pinned libghostty engine.
///
/// # Errors
///
/// Returns an error when the engine cannot be initialized or snapshotted.
pub fn builtin_terminal_colors() -> Result<TerminalColors> {
    let mut terminal = GhosttyVt::new(1, 1, 0)?;
    let state = terminal.snapshot()?;
    Ok(TerminalColors {
        source: "libghostty_builtin".to_owned(),
        foreground: rgb_array(state.colors.foreground),
        background: rgb_array(state.colors.background),
        cursor: state.colors.cursor.map(rgb_array),
        palette: state.colors.palette.into_iter().map(rgb_array).collect(),
    })
}

fn apply_config(colors: &mut TerminalColors, config: &[u8]) -> Result<()> {
    let text = String::from_utf8_lossy(config);
    for line in text.lines() {
        let Some((name, value)) = line.split_once(" = ") else {
            continue;
        };
        match name {
            "foreground" => colors.foreground = parse_rgb(value, "foreground")?,
            "background" => colors.background = parse_rgb(value, "background")?,
            "cursor-color" if value.starts_with('#') => {
                colors.cursor = Some(parse_rgb(value, "cursor-color")?);
            }
            "palette" => {
                let Some((index, color)) = value.split_once('=') else {
                    return Err(profile_error(format!("invalid palette entry `{value}`")));
                };
                let index = index
                    .parse::<u8>()
                    .map_err(|error| profile_error(format!("invalid palette index: {error}")))?;
                colors.palette[usize::from(index)] = parse_rgb(color, "palette")?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn parse_rgb(value: &str, field: &str) -> Result<[u8; 3]> {
    let Some(hex) = value.strip_prefix('#').filter(|hex| hex.len() == 6) else {
        return Err(profile_error(format!(
            "{field} must be a six-digit RGB color, got `{value}`"
        )));
    };
    let component = |range| {
        u8::from_str_radix(&hex[range], 16)
            .map_err(|error| profile_error(format!("invalid {field} color: {error}")))
    };
    Ok([component(0..2)?, component(2..4)?, component(4..6)?])
}

fn rgb_array(rgb: Rgb) -> [u8; 3] {
    [rgb.r, rgb.g, rgb.b]
}

fn profile_error(reason: String) -> Error {
    Error::TerminalOperation {
        operation: "parse Ghostty color profile",
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_resolved_ghostty_colors() {
        let mut colors = builtin_terminal_colors().expect("built-in colors");
        apply_config(
            &mut colors,
            b"background = #000000\nforeground = #f2f2f2\ncursor-color = #cccccc\npalette = 1=#ff0080\n",
        )
        .expect("valid profile");
        assert_eq!(colors.background, [0, 0, 0]);
        assert_eq!(colors.foreground, [0xf2, 0xf2, 0xf2]);
        assert_eq!(colors.cursor, Some([0xcc, 0xcc, 0xcc]));
        assert_eq!(colors.palette[1], [0xff, 0, 0x80]);
    }
}
