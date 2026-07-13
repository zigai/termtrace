//! Capture terminal host defaults needed for deterministic semantic replay.

use std::{
    env::temp_dir,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use uuid::Uuid;

use crate::{
    Error, Result,
    artifact::{create_private_directory, write_secure},
    event::ByteString,
    event::TerminalColors,
    ghostty_vt::{GhosttyVt, Rgb},
    session::{GhosttyVisualOverrides, GhosttyVisualProvenance},
};

/// Canonical Ghostty configuration resolved for one recording.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedGhosttyProfile {
    /// Flattened effective Ghostty configuration bytes.
    pub effective_config: Vec<u8>,
    /// Persisted source and convenience-override provenance.
    pub provenance: GhosttyVisualProvenance,
}

/// Resolve a custom Ghostty config and focused overrides into one profile.
///
/// A supplied config file replaces Ghostty's default user config. Without one,
/// the current effective user config is the base. Convenience overrides are
/// applied last and therefore always win.
///
/// # Errors
///
/// Returns an error when the source cannot be read by Ghostty, an override is
/// invalid, or the temporary profile cannot be persisted.
pub fn resolve_ghostty_profile(
    source_config: Option<&Path>,
    overrides: GhosttyVisualOverrides,
) -> Result<ResolvedGhosttyProfile> {
    let (base, source_config) = if let Some(source) = source_config {
        let canonical = fs_err::canonicalize(source).map_err(|source_error| {
            return Error::Filesystem {
                path: source.to_path_buf(),
                source: source_error,
            };
        })?;
        let Some(path) = canonical.to_str() else {
            return Err(profile_error("Ghostty config path must be valid UTF-8"));
        };
        let wrapper = format!("config-file = \"{}\"\n", escape_config_string(path));
        (
            query_isolated_config(wrapper.as_bytes())?,
            Some(ByteString::from(canonical.as_os_str().as_bytes())),
        )
    } else {
        (query_default_config()?, None)
    };
    let mut combined = flatten_config(&base);
    append_overrides(&mut combined, &overrides);
    let effective_config = flatten_config(&query_isolated_config(&combined)?);
    let effective_config_hash = blake3::hash(&effective_config).to_hex().to_string();
    return Ok(ResolvedGhosttyProfile {
        effective_config,
        provenance: GhosttyVisualProvenance {
            source_config,
            overrides,
            effective_config: "ghostty-config.txt".to_owned(),
            effective_config_hash,
        },
    });
}

/// Resolve terminal colors from a canonical Ghostty profile.
///
/// # Errors
///
/// Returns an error when the pinned defaults or explicit profile colors are
/// invalid.
pub fn terminal_colors_for_profile(profile: &ResolvedGhosttyProfile) -> Result<TerminalColors> {
    let mut colors = builtin_terminal_colors()?;
    apply_config(&mut colors, &profile.effective_config)?;
    colors.source = format!(
        "ghostty_show_config_blake3:{}",
        profile.provenance.effective_config_hash
    );
    return Ok(colors);
}

pub(crate) fn materialize_profile(root: &Path, config: &[u8]) -> Result<PathBuf> {
    create_private_directory(root)?;
    let ghostty = root.join("ghostty");
    create_private_directory(&ghostty)?;
    write_secure(&ghostty.join("config.ghostty"), config)?;
    return Ok(root.to_path_buf());
}

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
    return Ok(colors);
}

fn query_default_config() -> Result<Vec<u8>> {
    let output = Command::new("ghostty")
        .arg("+show-config")
        .output()
        .map_err(|error| return profile_error(format!("run Ghostty config query: {error}")))?;
    return checked_config_output(output, "query default Ghostty config");
}

fn query_isolated_config(config: &[u8]) -> Result<Vec<u8>> {
    let root = temp_dir().join(format!("termtrace-ghostty-profile-{}", Uuid::now_v7()));
    let result = materialize_profile(&root, config).and_then(|_| {
        return Command::new("ghostty")
            .arg("+show-config")
            .env("XDG_CONFIG_HOME", &root)
            .output()
            .map_err(|error| {
                return profile_error(format!("run isolated Ghostty config query: {error}"));
            })
            .and_then(|output| return checked_config_output(output, "resolve Ghostty config"));
    });
    let cleanup = if root.exists() {
        fs_err::remove_dir_all(&root).map_err(|source| {
            return Error::Filesystem { path: root, source };
        })
    } else {
        Ok(())
    };
    return finish_with_cleanup(result, cleanup);
}

fn checked_config_output(output: Output, operation: &'static str) -> Result<Vec<u8>> {
    if !output.status.success() {
        return Err(profile_error(format!(
            "{operation} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    return Ok(output.stdout);
}

fn flatten_config(config: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(config);
    let mut flattened = text
        .lines()
        .filter(|line| {
            let name = line
                .split_once('=')
                .map_or_else(|| return line.trim(), |(name, _)| return name.trim());
            return name != "config-file" && name != "config-default-files";
        })
        .collect::<Vec<_>>()
        .join("\n")
        .into_bytes();
    if !flattened.is_empty() {
        flattened.push(b'\n');
    }
    return flattened;
}

fn append_overrides(config: &mut Vec<u8>, overrides: &GhosttyVisualOverrides) {
    let mut lines = Vec::new();
    if let Some(value) = &overrides.font_family {
        lines.push("font-family = \"\"".to_owned());
        lines.push(format!("font-family = \"{}\"", escape_config_string(value)));
    }
    if let Some(value) = &overrides.font_size {
        lines.push(format!("font-size = {value}"));
    }
    if let Some(value) = overrides.padding_x {
        lines.push(format!("window-padding-x = {value}"));
    }
    if let Some(value) = overrides.padding_y {
        lines.push(format!("window-padding-y = {value}"));
    }
    if let Some(value) = &overrides.foreground {
        lines.push(format!("foreground = {value}"));
    }
    if let Some(value) = &overrides.background {
        lines.push(format!("background = {value}"));
    }
    if !lines.is_empty() {
        config.extend_from_slice(lines.join("\n").as_bytes());
        config.push(b'\n');
    }
}

fn escape_config_string(value: &str) -> String {
    return value.replace('\\', "\\\\").replace('"', "\\\"");
}

fn finish_with_cleanup<T>(primary: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (primary, cleanup) {
        (Ok(value), Ok(())) => return Ok(value),
        (Err(primary), Ok(())) => return Err(primary),
        (Ok(_), Err(cleanup)) => return Err(cleanup),
        (Err(primary), Err(cleanup)) => {
            return Err(Error::CleanupFailed {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            });
        }
    }
}

/// Read the color defaults compiled into the pinned libghostty engine.
///
/// # Errors
///
/// Returns an error when the engine cannot be initialized or snapshotted.
pub fn builtin_terminal_colors() -> Result<TerminalColors> {
    let mut terminal = GhosttyVt::new(1, 1, 0)?;
    let state = terminal.snapshot()?;
    if state.colors.palette.len() != 256 {
        return Err(profile_error(format!(
            "pinned libghostty returned {} palette entries instead of 256",
            state.colors.palette.len()
        )));
    }
    return Ok(TerminalColors {
        source: "libghostty_builtin".to_owned(),
        foreground: rgb_array(state.colors.foreground),
        background: rgb_array(state.colors.background),
        cursor: state.colors.cursor.map(rgb_array),
        palette: state.colors.palette.into_iter().map(rgb_array).collect(),
    });
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
                let index = index.parse::<u8>().map_err(|error| {
                    return profile_error(format!("invalid palette index: {error}"));
                })?;
                let parsed = parse_rgb(color, "palette")?;
                let slot = colors.palette.get_mut(usize::from(index)).ok_or_else(|| {
                    return profile_error("terminal profile has an incomplete palette".to_owned());
                })?;
                *slot = parsed;
            }
            _ => {}
        }
    }
    return Ok(());
}

fn parse_rgb(value: &str, field: &str) -> Result<[u8; 3]> {
    let Some(hex) = value.strip_prefix('#').filter(|hex| return hex.len() == 6) else {
        return Err(profile_error(format!(
            "{field} must be a six-digit RGB color, got `{value}`"
        )));
    };
    let component = |range| {
        let digits = hex
            .get(range)
            .ok_or_else(|| return profile_error(format!("invalid {field} color width")))?;
        return u8::from_str_radix(digits, 16)
            .map_err(|error| return profile_error(format!("invalid {field} color: {error}")));
    };
    return Ok([component(0..2)?, component(2..4)?, component(4..6)?]);
}

fn rgb_array(rgb: Rgb) -> [u8; 3] {
    return [rgb.r, rgb.g, rgb.b];
}

fn profile_error(reason: impl Into<String>) -> Error {
    return Error::TerminalOperation {
        operation: "resolve Ghostty profile",
        reason: reason.into(),
    };
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

    #[test]
    fn flattens_recursive_config_controls() {
        let flattened = flatten_config(
            b"font-size = 13\nconfig-file = /tmp/source\nconfig-default-files = false\nbackground = #010203\n",
        );
        assert_eq!(
            flattened,
            b"font-size = 13\nbackground = #010203\n".to_vec()
        );
    }

    #[test]
    fn appends_focused_overrides_after_the_base() {
        let mut config = b"font-size = 10\nbackground = #000000\n".to_vec();
        append_overrides(
            &mut config,
            &GhosttyVisualOverrides {
                font_family: Some("Example Mono".to_owned()),
                font_size: Some("15.5".to_owned()),
                padding_x: Some(8),
                padding_y: Some(6),
                foreground: Some("#f0f0f0".to_owned()),
                background: Some("#010203".to_owned()),
            },
        );
        let text = String::from_utf8(config).expect("valid UTF-8 config");
        assert!(text.ends_with(
            "font-family = \"\"\nfont-family = \"Example Mono\"\nfont-size = 15.5\nwindow-padding-x = 8\nwindow-padding-y = 6\nforeground = #f0f0f0\nbackground = #010203\n"
        ));
    }
}
