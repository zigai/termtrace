use super::*;
use crate::ghostty_vt::terminal::GhosttyVt;

#[test]
fn focus_encoder_emits_standard_reports() {
    assert_eq!(
        encode_focus(FocusEvent::In).expect("focus in").as_bytes(),
        b"\x1b[I"
    );
    assert_eq!(
        encode_focus(FocusEvent::Out).expect("focus out").as_bytes(),
        b"\x1b[O"
    );
}

#[test]
fn mouse_encoder_uses_live_sgr_modes_and_zero_based_cells() {
    let mut terminal = GhosttyVt::new(80, 24, 100).expect("terminal");
    terminal.write(b"\x1b[?1002h\x1b[?1006h");
    let press = terminal
        .encode_mouse(
            MouseAction::Press,
            MouseButton::Left,
            2,
            3,
            80,
            24,
            800,
            480,
        )
        .expect("mouse press");
    let release = terminal
        .encode_mouse(
            MouseAction::Release,
            MouseButton::Left,
            2,
            3,
            80,
            24,
            800,
            480,
        )
        .expect("mouse release");
    let motion = terminal
        .encode_mouse(
            MouseAction::Motion,
            MouseButton::Left,
            3,
            3,
            80,
            24,
            800,
            480,
        )
        .expect("mouse motion");
    assert_eq!(press.as_bytes(), b"\x1b[<0;3;4M");
    assert_eq!(release.as_bytes(), b"\x1b[<0;3;4m");
    assert_eq!(motion.as_bytes(), b"\x1b[<32;4;4M");
}

#[test]
fn key_encoder_tracks_live_cursor_mode_and_modifiers() {
    let mut terminal = GhosttyVt::new(80, 24, 0).expect("create terminal");
    let normal = terminal
        .encode_key(NamedKey::ArrowUp, KeyModifiers::default())
        .expect("normal arrow");
    assert_eq!(normal.data.as_bytes(), b"\x1b[A");

    terminal.write(b"\x1b[?1h");
    let application = terminal
        .encode_key(NamedKey::ArrowUp, KeyModifiers::default())
        .expect("application arrow");
    assert_eq!(application.data.as_bytes(), b"\x1bOA");

    let control_c = terminal
        .encode_key(
            NamedKey::Letter('c'),
            KeyModifiers {
                control: true,
                ..KeyModifiers::default()
            },
        )
        .expect("control c");
    assert_eq!(control_c.data.as_bytes(), b"\x03");
    let shift_tab = terminal
        .encode_key(
            NamedKey::Tab,
            KeyModifiers {
                shift: true,
                ..KeyModifiers::default()
            },
        )
        .expect("shift tab");
    assert_eq!(shift_tab.data.as_bytes(), b"\x1b[Z");
}

#[test]
fn every_supported_symbolic_key_encodes_for_all_modifiers_and_keyboard_modes() {
    let named_keys = [
        NamedKey::Enter,
        NamedKey::Escape,
        NamedKey::Tab,
        NamedKey::Backspace,
        NamedKey::Delete,
        NamedKey::Insert,
        NamedKey::Home,
        NamedKey::End,
        NamedKey::PageUp,
        NamedKey::PageDown,
        NamedKey::ArrowUp,
        NamedKey::ArrowDown,
        NamedKey::ArrowLeft,
        NamedKey::ArrowRight,
    ];
    for mode_bits in 0_u8..16 {
        let mut terminal = GhosttyVt::new(80, 24, 0).expect("create terminal");
        if mode_bits & 1 != 0 {
            terminal.write(b"\x1b[?1h");
        }
        if mode_bits & 2 != 0 {
            terminal.write(b"\x1b=");
        }
        if mode_bits & 4 != 0 {
            terminal.write(b"\x1b[>4;2m");
        }
        if mode_bits & 8 != 0 {
            terminal.write(b"\x1b[=1;1u");
        }

        for modifier_bits in 0_u8..16 {
            let modifiers = modifiers_from_bits(modifier_bits);
            for key in named_keys {
                assert_supported_encoding(&terminal, key, modifiers, mode_bits);
            }
            for number in 1_u8..=24 {
                assert_supported_encoding(
                    &terminal,
                    NamedKey::Function(number),
                    modifiers,
                    mode_bits,
                );
            }
            if modifier_bits == 0 {
                continue;
            }
            for letter in 'a'..='z' {
                assert_supported_encoding(
                    &terminal,
                    NamedKey::Letter(letter),
                    modifiers,
                    mode_bits,
                );
            }
        }
    }
}

fn modifiers_from_bits(bits: u8) -> KeyModifiers {
    return KeyModifiers {
        shift: bits & 1 != 0,
        control: bits & 2 != 0,
        alt: bits & 4 != 0,
        super_key: bits & 8 != 0,
    };
}

fn assert_supported_encoding(
    terminal: &GhosttyVt,
    key: NamedKey,
    modifiers: KeyModifiers,
    mode_bits: u8,
) {
    let encoded = terminal.encode_key(key, modifiers).unwrap_or_else(|error| {
            panic!(
                "supported key {key:?} with modifiers {modifiers:?} failed in keyboard mode matrix {mode_bits:04b}: {error}"
            );
        });
    let uses_extended_fallback = matches!(key, NamedKey::Function(13..=24)) && mode_bits & 8 == 0;
    let expected_encoder = if uses_extended_fallback {
        "termtrace-xterm-extended-function-key-v1"
    } else {
        "libghostty-key-encoder-v1"
    };
    assert_eq!(encoded.encoder, expected_encoder);
    if uses_extended_fallback {
        assert_eq!(
            encoded.data.as_bytes(),
            expected_extended_function_sequence(key, modifiers).as_bytes()
        );
    }
    assert!(
        !encoded.data.as_bytes().is_empty(),
        "supported key {key:?} with modifiers {modifiers:?} emitted no bytes in keyboard mode matrix {mode_bits:04b}"
    );
}

fn expected_extended_function_sequence(key: NamedKey, modifiers: KeyModifiers) -> String {
    let NamedKey::Function(number) = key else {
        panic!("extended function fallback requires a function key");
    };
    let parameter = match number {
        13 => 25,
        14 => 26,
        15 => 28,
        16 => 29,
        17 => 31,
        18 => 32,
        19 => 33,
        20 => 34,
        21 => 42,
        22 => 43,
        23 => 44,
        24 => 45,
        _ => panic!("extended function fallback requires F13 through F24"),
    };
    let modifier = 1_u8
        .saturating_add(u8::from(modifiers.shift))
        .saturating_add(u8::from(modifiers.alt).saturating_mul(2))
        .saturating_add(u8::from(modifiers.control).saturating_mul(4))
        .saturating_add(u8::from(modifiers.super_key).saturating_mul(8));
    if modifier == 1 {
        return format!("\x1b[{parameter}~");
    }
    return format!("\x1b[{parameter};{modifier}~");
}
