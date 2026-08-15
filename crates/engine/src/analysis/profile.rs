//! Pure Ghostty replay-profile normalization and validation.

use termtrace_model::{
    execution::event::TerminalColors,
    execution::session::{
        GHOSTTY_REPLAY_PROFILE_NAME, GhosttyVisualOverrides, GhosttyVisualProvenance,
    },
    terminal::state::Rgb,
    value::ByteString,
};
use termtrace_terminal::GhosttyVt;

use crate::{Error, Result};

/// Canonical Ghostty configuration resolved for one recording.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedGhosttyProfile {
    /// Exact configuration bytes chosen as the replay input.
    pub replay_config: Vec<u8>,
    /// Exact effective configuration bytes reported after loading the replay profile.
    pub effective_config: Vec<u8>,
    /// Persisted source and convenience-override provenance.
    pub provenance: GhosttyVisualProvenance,
}

impl ResolvedGhosttyProfile {
    /// BLAKE3 of the exact effective configuration.
    #[must_use]
    pub fn effective_config_hash(&self) -> String {
        return blake3::hash(&self.effective_config).to_hex().to_string();
    }

    /// Validate both persisted profile hashes and the canonical artifact name.
    pub fn validate(&self) -> Result<()> {
        if self.provenance.replay_profile != GHOSTTY_REPLAY_PROFILE_NAME {
            return Err(profile_error(format!(
                "recorded Ghostty replay profile must be the canonical root artifact `{GHOSTTY_REPLAY_PROFILE_NAME}`"
            )));
        }
        if hash(&self.replay_config) != self.provenance.replay_profile_hash {
            return Err(profile_error(
                "recorded Ghostty replay profile hash does not match provenance",
            ));
        }
        if self.effective_config_hash() != self.provenance.effective_config_hash {
            return Err(profile_error(
                "recorded Ghostty effective configuration hash does not match provenance",
            ));
        }
        return Ok(());
    }
}

/// Normalize an already-resolved base configuration and apply focused overrides.
///
/// The supplied adapter observes the replay config through its process owner;
/// this function transforms those bytes and binds their provenance.
pub fn resolve_profile(
    base_effective_config: &[u8],
    source_config: Option<ByteString>,
    overrides: GhosttyVisualOverrides,
    resolve_effective: impl FnOnce(&[u8]) -> Result<Vec<u8>>,
) -> Result<ResolvedGhosttyProfile> {
    let mut replay_config = flatten_config(base_effective_config);
    append_overrides(&mut replay_config, &overrides);
    let effective_config = resolve_effective(&replay_config)?;
    return Ok(ResolvedGhosttyProfile {
        provenance: GhosttyVisualProvenance {
            source_config,
            overrides,
            replay_profile: GHOSTTY_REPLAY_PROFILE_NAME.to_owned(),
            replay_profile_hash: hash(&replay_config),
            effective_config_hash: hash(&effective_config),
        },
        replay_config,
        effective_config,
    });
}

/// Restore and validate persisted profile bytes against caller-observed effective config.
pub fn restore_profile(
    replay_config: Vec<u8>,
    provenance: GhosttyVisualProvenance,
    resolve_effective: impl FnOnce(&[u8]) -> Result<Vec<u8>>,
) -> Result<ResolvedGhosttyProfile> {
    if provenance.replay_profile != GHOSTTY_REPLAY_PROFILE_NAME {
        return Err(profile_error(format!(
            "recorded Ghostty replay profile must be the canonical root artifact `{GHOSTTY_REPLAY_PROFILE_NAME}`"
        )));
    }
    if hash(&replay_config) != provenance.replay_profile_hash {
        return Err(profile_error(
            "recorded Ghostty replay profile hash does not match provenance",
        ));
    }
    let effective_config = resolve_effective(&replay_config)?;
    let profile = ResolvedGhosttyProfile {
        replay_config,
        effective_config,
        provenance,
    };
    profile.validate()?;
    return Ok(profile);
}

/// Resolve terminal colors from a validated canonical Ghostty profile.
pub fn terminal_colors_for_profile(profile: &ResolvedGhosttyProfile) -> Result<TerminalColors> {
    profile.validate()?;
    return terminal_colors_from_config(&profile.effective_config);
}

pub(crate) fn terminal_colors_from_config(config: &[u8]) -> Result<TerminalColors> {
    let mut colors = builtin_terminal_colors()?;
    apply_config(&mut colors, config)?;
    colors.source = format!("ghostty_show_config_blake3:{}", hash(config));
    return Ok(colors);
}

/// Read color defaults compiled into the pinned semantic engine.
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
    let mut lines: Vec<String> = Vec::new();
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
    if lines.is_empty() {
        return;
    }
    config.extend_from_slice(lines.join("\n").as_bytes());
    config.push(b'\n');
}

fn escape_config_string(value: &str) -> String {
    return value.replace('\\', "\\\\").replace('"', "\\\"");
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
                    return profile_error("terminal profile has an incomplete palette");
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

fn hash(bytes: &[u8]) -> String {
    return blake3::hash(bytes).to_hex().to_string();
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

    #[test]
    fn replays_valid_config_without_reusing_non_idempotent_output_as_input() {
        fn escape_backslashes(config: &[u8]) -> Result<Vec<u8>> {
            let mut output = Vec::with_capacity(config.len().saturating_mul(2));
            for byte in config {
                output.push(*byte);
                if *byte == b'\\' {
                    output.push(*byte);
                }
            }
            return Ok(output);
        }

        let replay_config = b"background = #000000\nkeybind = alt+f24=text:\\x1b\\x1b[24;2~\n";
        let profile = resolve_profile(
            replay_config,
            None,
            GhosttyVisualOverrides::default(),
            escape_backslashes,
        )
        .expect("valid replay profile");
        let second_generation =
            escape_backslashes(&profile.effective_config).expect("second serialized generation");

        assert_eq!(profile.replay_config, replay_config);
        assert_ne!(profile.effective_config, profile.replay_config);
        assert_ne!(second_generation, profile.effective_config);
        assert_eq!(
            profile.provenance.replay_profile,
            GHOSTTY_REPLAY_PROFILE_NAME
        );
        assert_eq!(
            profile.provenance.replay_profile_hash,
            blake3::hash(replay_config).to_hex().to_string()
        );
        assert_eq!(
            profile.provenance.effective_config_hash,
            blake3::hash(&profile.effective_config).to_hex().to_string()
        );

        let restored = restore_profile(
            profile.replay_config.clone(),
            profile.provenance.clone(),
            escape_backslashes,
        )
        .expect("restored replay profile");
        assert_eq!(restored, profile);
    }

    #[test]
    fn preserves_and_hashes_exact_effective_query_bytes() {
        let replay_config = b"background = #000000\n".to_vec();
        let effective_config =
            b"config-file = /isolated/config\nconfig-default-files = false\nbackground = #000000"
                .to_vec();
        let profile = resolve_profile(
            &replay_config,
            None,
            GhosttyVisualOverrides::default(),
            |_| return Ok(effective_config.clone()),
        )
        .expect("valid replay profile");

        assert_eq!(profile.effective_config, effective_config);
        assert_eq!(
            profile.provenance.effective_config_hash,
            blake3::hash(&effective_config).to_hex().to_string()
        );
    }

    #[test]
    fn constructed_profile_validation_binds_both_byte_sequences() {
        let profile = resolve_profile(
            b"background = #000000\n",
            None,
            GhosttyVisualOverrides::default(),
            |config| {
                let mut effective = config.to_vec();
                effective.extend_from_slice(b"foreground = #ffffff\n");
                return Ok(effective);
            },
        )
        .expect("valid replay profile");
        profile.validate().expect("valid byte commitments");

        let mut tampered_replay = profile.clone();
        tampered_replay.replay_config.push(b' ');
        assert!(tampered_replay.validate().is_err());

        let mut tampered_effective = profile.clone();
        tampered_effective.effective_config.push(b' ');
        assert!(tampered_effective.validate().is_err());

        let mut relocated = profile;
        relocated.provenance.replay_profile = "nested/profile.ghostty".to_owned();
        assert!(relocated.validate().is_err());
    }

    #[test]
    fn restore_rejects_tampered_replay_profile_before_querying_ghostty() {
        let profile = resolve_profile(
            b"background = #000000\n",
            None,
            GhosttyVisualOverrides::default(),
            |config| return Ok(config.to_vec()),
        )
        .expect("valid replay profile");
        let mut tampered = profile.replay_config;
        tampered.extend_from_slice(b"foreground = #ffffff\n");

        let error = restore_profile(tampered, profile.provenance, |_| -> Result<Vec<u8>> {
            panic!("Ghostty query must not run for unauthenticated replay bytes");
        })
        .expect_err("tampered replay profile must fail");

        assert!(matches!(
            error,
            Error::TerminalOperation { reason, .. }
                if reason == "recorded Ghostty replay profile hash does not match provenance"
        ));
    }

    #[test]
    fn restore_rejects_changed_effective_ghostty_output() {
        let replay_config = b"background = #000000\n".to_vec();
        let profile = resolve_profile(
            &replay_config,
            None,
            GhosttyVisualOverrides::default(),
            |_| return Ok(b"background = #000000\n".to_vec()),
        )
        .expect("valid replay profile");

        let error = restore_profile(replay_config, profile.provenance, |_| {
            return Ok(b"background = #ffffff\n".to_vec());
        })
        .expect_err("changed effective output must fail");

        assert!(matches!(
            error,
            Error::TerminalOperation { reason, .. }
                if reason == "recorded Ghostty effective configuration hash does not match provenance"
        ));
    }
}
