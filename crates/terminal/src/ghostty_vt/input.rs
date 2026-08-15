//! Typed focus, key, and mouse encoding through the pinned Ghostty API.

use std::{
    ffi::{c_char, c_void},
    mem::size_of,
    ptr,
};

use termtrace_model::value::ByteString;

use crate::{Error, Result};

use super::{
    sys,
    terminal::check,
    types::{EncodedKey, FocusEvent, KeyModifiers, MouseAction, MouseButton, NamedKey},
};

/// Encode one focus transition with the pinned terminal encoder.
///
/// # Errors
///
/// Returns an error when the pinned encoder rejects the event.
pub fn encode_focus(event: FocusEvent) -> Result<ByteString> {
    let event = match event {
        FocusEvent::In => sys::GhosttyFocusEvent_GHOSTTY_FOCUS_GAINED,
        FocusEvent::Out => sys::GhosttyFocusEvent_GHOSTTY_FOCUS_LOST,
    };
    let mut required = 0;
    let first = unsafe { sys::ghostty_focus_encode(event, ptr::null_mut(), 0, &raw mut required) };
    if first != sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE && first != 0 {
        check("size encoded focus event", first)?;
    }
    let mut output = vec![0_u8; required];
    let mut written = 0;
    check("encode focus event", unsafe {
        sys::ghostty_focus_encode(
            event,
            output.as_mut_ptr().cast::<c_char>(),
            output.len(),
            &raw mut written,
        )
    })?;
    output.truncate(written);
    return Ok(ByteString::new(output));
}

pub(super) fn encode_key(
    terminal: sys::GhosttyTerminal,
    key: NamedKey,
    modifiers: KeyModifiers,
) -> Result<EncodedKey> {
    let (physical_key, text) = physical_key(key)?;
    let mut encoder = ptr::null_mut();
    check("create key encoder", unsafe {
        sys::ghostty_key_encoder_new(ptr::null(), &raw mut encoder)
    })?;
    let encoder = KeyEncoderGuard(encoder);
    unsafe {
        sys::ghostty_key_encoder_setopt_from_terminal(encoder.0, terminal);
    }
    let option_as_alt = sys::GhosttyOptionAsAlt_GHOSTTY_OPTION_AS_ALT_FALSE;
    unsafe {
        sys::ghostty_key_encoder_setopt(
            encoder.0,
            sys::GhosttyKeyEncoderOption_GHOSTTY_KEY_ENCODER_OPT_MACOS_OPTION_AS_ALT,
            (&raw const option_as_alt).cast::<c_void>(),
        );
    }
    let mut event = ptr::null_mut();
    check("create key event", unsafe {
        sys::ghostty_key_event_new(ptr::null(), &raw mut event)
    })?;
    let event = KeyEventGuard(event);
    unsafe {
        sys::ghostty_key_event_set_action(event.0, sys::GhosttyKeyAction_GHOSTTY_KEY_ACTION_PRESS);
        sys::ghostty_key_event_set_key(event.0, physical_key);
        sys::ghostty_key_event_set_mods(event.0, modifier_bits(modifiers));
        if let Some(text) = text {
            sys::ghostty_key_event_set_utf8(event.0, text.as_ptr().cast::<c_char>(), text.len());
            sys::ghostty_key_event_set_unshifted_codepoint(event.0, u32::from(text[0]));
        }
    }
    let mut required = 0;
    let first = unsafe {
        sys::ghostty_key_encoder_encode(encoder.0, event.0, ptr::null_mut(), 0, &raw mut required)
    };
    if first != sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE && first != 0 {
        check("size encoded key", first)?;
    }
    if required == 0 {
        if let Some(encoded) = extended_function_key_fallback(key, modifiers) {
            return Ok(encoded);
        }
        return Ok(EncodedKey {
            data: ByteString::new(Vec::new()),
            encoder: "libghostty-key-encoder-v1".to_owned(),
        });
    }
    let mut output = vec![0_u8; required];
    let mut written = 0;
    check("encode key", unsafe {
        sys::ghostty_key_encoder_encode(
            encoder.0,
            event.0,
            output.as_mut_ptr().cast::<c_char>(),
            output.len(),
            &raw mut written,
        )
    })?;
    output.truncate(written);
    return Ok(EncodedKey {
        data: ByteString::new(output),
        encoder: "libghostty-key-encoder-v1".to_owned(),
    });
}

pub(super) fn encode_mouse(
    terminal: sys::GhosttyTerminal,
    action: MouseAction,
    button: MouseButton,
    column: u16,
    row: u16,
    cols: u16,
    rows: u16,
    pixel_width: u16,
    pixel_height: u16,
) -> Result<ByteString> {
    if cols == 0 || rows == 0 || column >= cols || row >= rows {
        return Err(Error::GhosttyVt {
            operation: "validate mouse cell",
            result: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
        });
    }
    let cell_width = u32::from(pixel_width.checked_div(cols).unwrap_or(0).max(1));
    let cell_height = u32::from(pixel_height.checked_div(rows).unwrap_or(0).max(1));
    let mut encoder = ptr::null_mut();
    check("create mouse encoder", unsafe {
        sys::ghostty_mouse_encoder_new(ptr::null(), &raw mut encoder)
    })?;
    let encoder = MouseEncoderGuard(encoder);
    unsafe { sys::ghostty_mouse_encoder_setopt_from_terminal(encoder.0, terminal) };
    let size = sys::GhosttyMouseEncoderSize {
        size: size_of::<sys::GhosttyMouseEncoderSize>(),
        screen_width: u32::from(pixel_width),
        screen_height: u32::from(pixel_height),
        cell_width,
        cell_height,
        padding_top: 0,
        padding_bottom: 0,
        padding_right: 0,
        padding_left: 0,
    };
    unsafe {
        sys::ghostty_mouse_encoder_setopt(
            encoder.0,
            sys::GhosttyMouseEncoderOption_GHOSTTY_MOUSE_ENCODER_OPT_SIZE,
            (&raw const size).cast::<c_void>(),
        );
    }
    let mut event = ptr::null_mut();
    check("create mouse event", unsafe {
        sys::ghostty_mouse_event_new(ptr::null(), &raw mut event)
    })?;
    let event = MouseEventGuard(event);
    let action = match action {
        MouseAction::Press => sys::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_PRESS,
        MouseAction::Release => sys::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_RELEASE,
        MouseAction::Motion => sys::GhosttyMouseAction_GHOSTTY_MOUSE_ACTION_MOTION,
    };
    let button = match button {
        MouseButton::Left => sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_LEFT,
        MouseButton::Middle => sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_MIDDLE,
        MouseButton::Right => sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_RIGHT,
        MouseButton::WheelUp => sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_FOUR,
        MouseButton::WheelDown => sys::GhosttyMouseButton_GHOSTTY_MOUSE_BUTTON_FIVE,
    };
    let position = sys::GhosttyMousePosition {
        x: (f32::from(column) + 0.5) * cell_width as f32,
        y: (f32::from(row) + 0.5) * cell_height as f32,
    };
    unsafe {
        sys::ghostty_mouse_event_set_action(event.0, action);
        sys::ghostty_mouse_event_set_button(event.0, button);
        sys::ghostty_mouse_event_set_position(event.0, position);
    }
    let mut required = 0;
    let first = unsafe {
        sys::ghostty_mouse_encoder_encode(encoder.0, event.0, ptr::null_mut(), 0, &raw mut required)
    };
    if first != sys::GhosttyResult_GHOSTTY_OUT_OF_SPACE && first != 0 {
        check("size encoded mouse event", first)?;
    }
    let mut output = vec![0_u8; required];
    let mut written = 0;
    check("encode mouse event", unsafe {
        sys::ghostty_mouse_encoder_encode(
            encoder.0,
            event.0,
            output.as_mut_ptr().cast::<c_char>(),
            output.len(),
            &raw mut written,
        )
    })?;
    output.truncate(written);
    return Ok(ByteString::new(output));
}

struct KeyEncoderGuard(sys::GhosttyKeyEncoder);

impl Drop for KeyEncoderGuard {
    fn drop(&mut self) {
        unsafe { sys::ghostty_key_encoder_free(self.0) };
    }
}

struct KeyEventGuard(sys::GhosttyKeyEvent);

impl Drop for KeyEventGuard {
    fn drop(&mut self) {
        unsafe { sys::ghostty_key_event_free(self.0) };
    }
}

struct MouseEncoderGuard(sys::GhosttyMouseEncoder);

impl Drop for MouseEncoderGuard {
    fn drop(&mut self) {
        unsafe { sys::ghostty_mouse_encoder_free(self.0) };
    }
}

struct MouseEventGuard(sys::GhosttyMouseEvent);

impl Drop for MouseEventGuard {
    fn drop(&mut self) {
        unsafe { sys::ghostty_mouse_event_free(self.0) };
    }
}

fn physical_key(key: NamedKey) -> Result<(sys::GhosttyKey, Option<[u8; 1]>)> {
    let value = match key {
        NamedKey::Enter => sys::GhosttyKey_GHOSTTY_KEY_ENTER,
        NamedKey::Escape => sys::GhosttyKey_GHOSTTY_KEY_ESCAPE,
        NamedKey::Tab => sys::GhosttyKey_GHOSTTY_KEY_TAB,
        NamedKey::Backspace => sys::GhosttyKey_GHOSTTY_KEY_BACKSPACE,
        NamedKey::Delete => sys::GhosttyKey_GHOSTTY_KEY_DELETE,
        NamedKey::Insert => sys::GhosttyKey_GHOSTTY_KEY_INSERT,
        NamedKey::Home => sys::GhosttyKey_GHOSTTY_KEY_HOME,
        NamedKey::End => sys::GhosttyKey_GHOSTTY_KEY_END,
        NamedKey::PageUp => sys::GhosttyKey_GHOSTTY_KEY_PAGE_UP,
        NamedKey::PageDown => sys::GhosttyKey_GHOSTTY_KEY_PAGE_DOWN,
        NamedKey::ArrowUp => sys::GhosttyKey_GHOSTTY_KEY_ARROW_UP,
        NamedKey::ArrowDown => sys::GhosttyKey_GHOSTTY_KEY_ARROW_DOWN,
        NamedKey::ArrowLeft => sys::GhosttyKey_GHOSTTY_KEY_ARROW_LEFT,
        NamedKey::ArrowRight => sys::GhosttyKey_GHOSTTY_KEY_ARROW_RIGHT,
        NamedKey::Function(number) if (1..=24).contains(&number) => {
            return Ok((sys::GhosttyKey_GHOSTTY_KEY_F1 + u32::from(number - 1), None));
        }
        NamedKey::Letter(letter) if letter.is_ascii_alphabetic() => {
            let lower = letter.to_ascii_lowercase();
            let index = u32::from(lower) - u32::from('a');
            let byte = u8::try_from(u32::from(lower)).map_err(|_| {
                return Error::GhosttyVt {
                    operation: "validate semantic key",
                    result: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
                };
            })?;
            return Ok((sys::GhosttyKey_GHOSTTY_KEY_A + index, Some([byte])));
        }
        NamedKey::Function(_) | NamedKey::Letter(_) => {
            return Err(Error::GhosttyVt {
                operation: "validate semantic key",
                result: sys::GhosttyResult_GHOSTTY_INVALID_VALUE,
            });
        }
    };
    return Ok((value, None));
}

fn modifier_bits(modifiers: KeyModifiers) -> sys::GhosttyMods {
    let mut bits = 0_u16;
    if modifiers.shift {
        bits |= 1;
    }
    if modifiers.control {
        bits |= 1 << 1;
    }
    if modifiers.alt {
        bits |= 1 << 2;
    }
    if modifiers.super_key {
        bits |= 1 << 3;
    }
    return bits;
}

fn extended_function_key_fallback(key: NamedKey, modifiers: KeyModifiers) -> Option<EncodedKey> {
    let NamedKey::Function(number) = key else {
        return None;
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
        _ => return None,
    };
    let modifier = 1_u8
        .saturating_add(u8::from(modifiers.shift))
        .saturating_add(u8::from(modifiers.alt).saturating_mul(2))
        .saturating_add(u8::from(modifiers.control).saturating_mul(4))
        .saturating_add(u8::from(modifiers.super_key).saturating_mul(8));
    let sequence = if modifier == 1 {
        format!("\x1b[{parameter}~")
    } else {
        format!("\x1b[{parameter};{modifier}~")
    };
    return Some(EncodedKey {
        data: ByteString::new(sequence.into_bytes()),
        encoder: "termtrace-xterm-extended-function-key-v1".to_owned(),
    });
}

#[cfg(test)]
mod tests;
