use super::*;

pub(super) fn input_sequence_atom_for_cli(
    atom: &str,
    operation_id: uuid::Uuid,
    caller_intent: Option<String>,
) -> Result<InputSequenceAtom> {
    if let Some((key, modifiers)) = parse_named_key(atom)? {
        return Ok(InputSequenceAtom::Key {
            operation_id,
            key,
            modifiers,
            caller_intent,
        });
    }

    if let Some(paste) = atom.strip_prefix("paste:") {
        return Ok(InputSequenceAtom::Paste {
            operation_id,
            data: ByteString::new(paste.as_bytes().to_vec()),
            caller_intent,
        });
    }

    let (data, input_kind) = if let Some(text) = atom.strip_prefix("text:") {
        (text.as_bytes().to_vec(), InputKind::Text)
    } else if let Some(raw) = atom.strip_prefix("raw:") {
        (raw.as_bytes().to_vec(), InputKind::Raw)
    } else if let Some(path) = atom.strip_prefix("raw-file:") {
        (fs_err::read(path).into_diagnostic()?, InputKind::Raw)
    } else if atom.chars().count() == 1
        && atom.chars().all(|character| return !character.is_control())
    {
        (atom.as_bytes().to_vec(), InputKind::Text)
    } else {
        return Err(miette::miette!(
            "unknown input atom; run `termtrace send --help` or use text:VALUE, paste:VALUE, raw:VALUE, or raw-file:PATH"
        ));
    };

    return Ok(InputSequenceAtom::Bytes {
        operation_id,
        data: ByteString::new(data),
        input_kind,
        caller_intent,
    });
}

pub(super) fn parse_named_key(atom: &str) -> Result<Option<(NamedKey, KeyModifiers)>> {
    let normalized = atom.to_ascii_lowercase().replace('+', "-");
    let mut remaining = normalized.as_str();
    let mut modifiers = KeyModifiers::default();

    let mut modifier_count = 0_u8;
    loop {
        let (next, selected) = if let Some(next) = remaining.strip_prefix("ctrl-") {
            if modifiers.control {
                return Err(miette::miette!(
                    "control modifier was supplied more than once"
                ));
            }

            modifiers.control = true;
            (next, true)
        } else if let Some(next) = remaining.strip_prefix("control-") {
            if modifiers.control {
                return Err(miette::miette!(
                    "control modifier was supplied more than once"
                ));
            }

            modifiers.control = true;
            (next, true)
        } else if let Some(next) = remaining.strip_prefix("shift-") {
            if modifiers.shift {
                return Err(miette::miette!(
                    "shift modifier was supplied more than once"
                ));
            }

            modifiers.shift = true;
            (next, true)
        } else if let Some(next) = remaining.strip_prefix("alt-") {
            if modifiers.alt {
                return Err(miette::miette!("alt modifier was supplied more than once"));
            }

            modifiers.alt = true;
            (next, true)
        } else if let Some(next) = remaining.strip_prefix("super-") {
            if modifiers.super_key {
                return Err(miette::miette!(
                    "super modifier was supplied more than once"
                ));
            }

            modifiers.super_key = true;
            (next, true)
        } else {
            (remaining, false)
        };
        if !selected {
            break;
        }

        modifier_count = modifier_count.saturating_add(1);
        remaining = next;
    }

    let key = match remaining {
        "enter" => NamedKey::Enter,
        "escape" | "esc" => NamedKey::Escape,
        "tab" => NamedKey::Tab,
        "backspace" => NamedKey::Backspace,
        "delete" => NamedKey::Delete,
        "insert" => NamedKey::Insert,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "page-up" => NamedKey::PageUp,
        "page-down" => NamedKey::PageDown,
        "up" => NamedKey::ArrowUp,
        "down" => NamedKey::ArrowDown,
        "left" => NamedKey::ArrowLeft,
        "right" => NamedKey::ArrowRight,
        value if value.len() >= 2 && value.starts_with('f') => {
            let suffix = value
                .strip_prefix('f')
                .ok_or_else(|| miette::miette!("function key must be f1 through f24"))?;

            let number = suffix
                .parse::<u8>()
                .map_err(|_| miette::miette!("function key must be f1 through f24"))?;

            if !(1..=24).contains(&number) {
                return Err(miette::miette!("function key must be f1 through f24"));
            }

            NamedKey::Function(number)
        }
        value if modifier_count > 0 && value.len() == 1 => {
            let letter = value
                .chars()
                .next()
                .ok_or_else(|| miette::miette!("modified letter is missing"))?;

            if !letter.is_ascii_alphabetic() {
                return Err(miette::miette!(
                    "modified printable keys currently support ASCII letters; run `termtrace send --help`"
                ));
            }

            NamedKey::Letter(letter)
        }
        _ => return Ok(None),
    };

    return Ok(Some((key, modifiers)));
}
