//! Ghostty effect callbacks that record what a program asked the terminal to do.
//!
//! Payloads that may hold secrets, such as clipboard data and notification text, are reduced
//! to a length and short hash inside the callback and never leave it.

use super::{check, sys, Effects};
use crate::Result;
use std::{ffi::c_void, ptr, slice};
use termtrace_model::{
    terminal::effect::{
        ClipboardRepresentation, ClipboardTarget, PayloadDigest, ProgressState, PromptKind,
        TerminalEffect, UnknownSequenceKind, UNKNOWN_SEQUENCE_IDENTIFIER_BYTES,
    },
    value::ByteString,
};

pub(super) fn register(terminal: sys::GhosttyTerminal) -> Result<()> {
    #[allow(
        clippy::fn_to_numeric_cast_any,
        reason = "Ghostty's generic C option setter accepts callback pointers as void pointers"
    )]
    let callbacks = [
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_BELL,
            bell as *const (),
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_TITLE_CHANGED,
            title_changed as *const (),
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_PWD_CHANGED,
            pwd_changed as *const (),
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE,
            clipboard_write as *const (),
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_DESKTOP_NOTIFICATION,
            desktop_notification as *const (),
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT,
            progress_report as *const (),
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_SEMANTIC_PROMPT,
            semantic_prompt as *const (),
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_RESET,
            reset as *const (),
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_RESIZE_WINDOW,
            resize_window as *const (),
        ),
        (
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_UNKNOWN_SEQUENCE,
            unknown_sequence as *const (),
        ),
    ];

    for (option, callback) in callbacks {
        check("set terminal effect callback", unsafe {
            sys::ghostty_terminal_set(terminal, option, callback.cast::<c_void>())
        })?;
    }

    // Only the identifying prefix is kept, which bounds what Ghostty buffers per sequence.
    let unknown_max_bytes = UNKNOWN_SEQUENCE_IDENTIFIER_BYTES;

    return check("limit unknown sequence capture", unsafe {
        sys::ghostty_terminal_set(
            terminal,
            sys::GhosttyTerminalOption_GHOSTTY_TERMINAL_OPT_UNKNOWN_MAX_BYTES,
            (&raw const unknown_max_bytes).cast::<c_void>(),
        )
    });
}

fn report(userdata: *mut c_void, effect: TerminalEffect) {
    if userdata.is_null() {
        return;
    }

    let effects = unsafe { &mut *userdata.cast::<Effects>() };
    effects.reported.push(effect);
}

// Ghostty strings are borrowed for the callback and may be empty with any pointer.
unsafe fn borrowed<'callback>(string: sys::GhosttyString) -> &'callback [u8] {
    if string.ptr.is_null() || string.len == 0 {
        return &[];
    }
    return unsafe { slice::from_raw_parts(string.ptr, string.len) };
}

fn terminal_string(terminal: sys::GhosttyTerminal, data: sys::GhosttyTerminalData) -> ByteString {
    let mut string = sys::GhosttyString {
        ptr: ptr::null(),
        len: 0,
    };
    let result =
        unsafe { sys::ghostty_terminal_get(terminal, data, (&raw mut string).cast::<c_void>()) };

    if result != sys::GhosttyResult_GHOSTTY_SUCCESS {
        return ByteString::default();
    }
    return ByteString::from(unsafe { borrowed(string) });
}

unsafe extern "C" fn bell(_terminal: sys::GhosttyTerminal, userdata: *mut c_void) {
    report(userdata, TerminalEffect::Bell);
}

unsafe extern "C" fn title_changed(terminal: sys::GhosttyTerminal, userdata: *mut c_void) {
    let title = terminal_string(
        terminal,
        sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_TITLE,
    );

    report(userdata, TerminalEffect::TitleChanged { title });
}

unsafe extern "C" fn pwd_changed(terminal: sys::GhosttyTerminal, userdata: *mut c_void) {
    let location = terminal_string(terminal, sys::GhosttyTerminalData_GHOSTTY_TERMINAL_DATA_PWD);
    report(
        userdata,
        TerminalEffect::WorkingDirectoryChanged { location },
    );
}

unsafe extern "C" fn clipboard_write(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    write: *const sys::GhosttyClipboardWrite,
) {
    let Some(write) = (unsafe { write.as_ref() }) else {
        return;
    };
    if write.size < size_of::<sys::GhosttyClipboardWrite>() {
        return;
    }

    let target = match write.location {
        sys::GhosttyClipboardLocation_GHOSTTY_CLIPBOARD_LOCATION_STANDARD => {
            ClipboardTarget::Standard
        }
        sys::GhosttyClipboardLocation_GHOSTTY_CLIPBOARD_LOCATION_SELECTION => {
            ClipboardTarget::Selection
        }
        sys::GhosttyClipboardLocation_GHOSTTY_CLIPBOARD_LOCATION_PRIMARY => {
            ClipboardTarget::Primary
        }
        _ => ClipboardTarget::Other,
    };

    let contents = if write.contents.is_null() || write.contents_len == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(write.contents, write.contents_len) }
    };

    let representations = contents
        .iter()
        .map(|content| {
            return ClipboardRepresentation {
                mime: String::from_utf8_lossy(unsafe { borrowed(content.mime) }).into_owned(),
                data: PayloadDigest::of(unsafe { borrowed(content.data) }),
            };
        })
        .collect();

    report(
        userdata,
        TerminalEffect::ClipboardWrite {
            target,
            representations,
        },
    );

    // Replay never writes a clipboard; answering keeps Kitty's reply ENOSYS as without a callback.
    if let Some(reply) = write.reply {
        let answer = sys::GhosttyClipboardWriteReply {
            size: size_of::<sys::GhosttyClipboardWriteReply>(),
            result: sys::GhosttyClipboardWriteResult_GHOSTTY_CLIPBOARD_WRITE_RESULT_UNSUPPORTED,
            remember: false,
        };
        unsafe { reply(write, &raw const answer) };
    }
}

unsafe extern "C" fn desktop_notification(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    notification: *const sys::GhosttyTerminalDesktopNotification,
) {
    let Some(notification) = (unsafe { notification.as_ref() }) else {
        return;
    };
    if notification.size < size_of::<sys::GhosttyTerminalDesktopNotification>() {
        return;
    }

    report(
        userdata,
        TerminalEffect::DesktopNotification {
            title: PayloadDigest::of(unsafe { borrowed(notification.title) }),
            body: PayloadDigest::of(unsafe { borrowed(notification.body) }),
        },
    );
}

unsafe extern "C" fn progress_report(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    progress: *const sys::GhosttyTerminalProgressReport,
) {
    let Some(progress) = (unsafe { progress.as_ref() }) else {
        return;
    };
    if progress.size < size_of::<sys::GhosttyTerminalProgressReport>() {
        return;
    }

    let state = match progress.state {
        sys::GhosttyTerminalProgressState_GHOSTTY_TERMINAL_PROGRESS_STATE_REMOVE => {
            ProgressState::Remove
        }
        sys::GhosttyTerminalProgressState_GHOSTTY_TERMINAL_PROGRESS_STATE_SET => ProgressState::Set,
        sys::GhosttyTerminalProgressState_GHOSTTY_TERMINAL_PROGRESS_STATE_ERROR => {
            ProgressState::Error
        }
        sys::GhosttyTerminalProgressState_GHOSTTY_TERMINAL_PROGRESS_STATE_INDETERMINATE => {
            ProgressState::Indeterminate
        }
        sys::GhosttyTerminalProgressState_GHOSTTY_TERMINAL_PROGRESS_STATE_PAUSE => {
            ProgressState::Pause
        }
        _ => ProgressState::Other,
    };

    report(
        userdata,
        TerminalEffect::ProgressReport {
            state,
            percent: u8::try_from(progress.progress).ok(),
        },
    );
}

unsafe extern "C" fn semantic_prompt(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    event: *const sys::GhosttyTerminalSemanticPrompt,
) {
    let Some(event) = (unsafe { event.as_ref() }) else {
        return;
    };
    if event.size < size_of::<sys::GhosttyTerminalSemanticPrompt>() {
        return;
    }

    let optional = |string| {
        let bytes = unsafe { borrowed(string) };

        return (!bytes.is_empty()).then(|| return ByteString::from(bytes));
    };
    let effect = match event.kind {
        sys::GhosttySemanticPromptKind_GHOSTTY_SEMANTIC_PROMPT_PROMPT_START => {
            let prompt = match event.prompt_kind {
                sys::GhosttySemanticPromptPromptKind_GHOSTTY_SEMANTIC_PROMPT_PROMPT_PRIMARY => {
                    PromptKind::Primary
                }
                sys::GhosttySemanticPromptPromptKind_GHOSTTY_SEMANTIC_PROMPT_PROMPT_RIGHT => {
                    PromptKind::Right
                }
                sys::GhosttySemanticPromptPromptKind_GHOSTTY_SEMANTIC_PROMPT_PROMPT_CONTINUATION => {
                    PromptKind::Continuation
                }
                sys::GhosttySemanticPromptPromptKind_GHOSTTY_SEMANTIC_PROMPT_PROMPT_SECONDARY => {
                    PromptKind::Secondary
                }
                _ => PromptKind::Other,
            };

            TerminalEffect::PromptStart { prompt }
        }
        sys::GhosttySemanticPromptKind_GHOSTTY_SEMANTIC_PROMPT_INPUT_START => {
            TerminalEffect::InputStart
        }
        sys::GhosttySemanticPromptKind_GHOSTTY_SEMANTIC_PROMPT_OUTPUT_START => {
            TerminalEffect::CommandStart {
                command: optional(event.command),
            }
        }
        sys::GhosttySemanticPromptKind_GHOSTTY_SEMANTIC_PROMPT_COMMAND_END => {
            TerminalEffect::CommandEnd {
                exit_code: event.has_exit_code.then_some(event.exit_code),
                error: optional(event.error),
            }
        }
        _ => return,
    };

    report(userdata, effect);
}

unsafe extern "C" fn reset(_terminal: sys::GhosttyTerminal, userdata: *mut c_void) {
    report(userdata, TerminalEffect::FullReset);
}

unsafe extern "C" fn resize_window(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    request: *const sys::GhosttyTerminalResizeWindow,
) {
    let Some(request) = (unsafe { request.as_ref() }) else {
        return;
    };
    if request.tag != sys::GhosttyTerminalResizeWindowTag_GHOSTTY_TERMINAL_RESIZE_WINDOW_GRID {
        return;
    }

    let grid = unsafe { request.value.grid };

    report(
        userdata,
        TerminalEffect::WindowResizeRequest {
            rows: (grid.rows != 0).then_some(grid.rows),
            columns: (grid.columns != 0).then_some(grid.columns),
        },
    );
}

unsafe extern "C" fn unknown_sequence(
    _terminal: sys::GhosttyTerminal,
    userdata: *mut c_void,
    sequence: *const sys::GhosttyTerminalUnknownSequence,
) {
    let Some(sequence) = (unsafe { sequence.as_ref() }) else {
        return;
    };
    let (kind, content) = match sequence.tag {
        sys::GhosttyTerminalUnknownSequenceTag_GHOSTTY_TERMINAL_UNKNOWN_SEQUENCE_APC => {
            (UnknownSequenceKind::Apc, unsafe {
                sequence.value.apc.content
            })
        }
        sys::GhosttyTerminalUnknownSequenceTag_GHOSTTY_TERMINAL_UNKNOWN_SEQUENCE_OSC => {
            (UnknownSequenceKind::Osc, unsafe {
                sequence.value.osc.content
            })
        }
        _ => return,
    };

    report(
        userdata,
        TerminalEffect::UnknownSequence {
            kind,
            identifier: ByteString::from(sequence_identifier(unsafe { borrowed(content) })),
        },
    );
}

// The identifier is the leading command name or number; parameters may carry secrets.
fn sequence_identifier(content: &[u8]) -> &[u8] {
    let length = content
        .iter()
        .take(UNKNOWN_SEQUENCE_IDENTIFIER_BYTES)
        .take_while(|byte| {
            return byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.');
        })
        .count();

    return content.get(..length).unwrap_or_default();
}
