"""Pure payload builders for terminal session commands and requests.

Eliminates duplicate payload construction across synchronous and asynchronous session clients.
"""

from __future__ import annotations

import base64
import uuid
from collections.abc import Sequence
from pathlib import Path
from typing import Literal

from termtrace.atoms import KeyboardAtom, encode_input_sequence
from termtrace.duration import Duration, duration
from termtrace.screen.frame import FrameRequest
from termtrace.screen.wire import encode_frame_request
from termtrace.transport.protocol import JsonObject, JsonValue


def keyboard_press_command(
    key: str,
    *,
    caller_intent: str | None = None,
) -> JsonObject:
    return {
        "command": "key",
        "key": key,
        "modifiers": {
            "shift": False,
            "control": False,
            "alt": False,
            "super_key": False,
        },
        "caller_intent": caller_intent,
    }


def keyboard_type_command(
    text: str,
    *,
    caller_intent: str | None = None,
) -> JsonObject:
    return {
        "command": "input",
        "data": base64.b64encode(text.encode()).decode(),
        "input_kind": "text",
        "caller_intent": caller_intent,
    }


def keyboard_paste_command(
    data: str | bytes | memoryview,
    *,
    caller_intent: str | None = None,
) -> JsonObject:
    raw = data.encode() if isinstance(data, str) else bytes(data)
    return {
        "command": "paste",
        "data": base64.b64encode(raw).decode(),
        "caller_intent": caller_intent,
    }


def keyboard_sequence_command(
    atoms: Sequence[KeyboardAtom],
    *,
    pace_ms: int = 0,
    verbose: bool = False,
    sequence_id: str | None = None,
) -> JsonObject:
    seq_id = sequence_id or str(uuid.uuid4())
    return encode_input_sequence(
        atoms,
        pace_ms=pace_ms,
        verbose=verbose,
        sequence_id=seq_id,
    )


def pointer_click_command(
    column: int,
    row: int,
    *,
    button: str = "left",
    strict: bool = False,
) -> JsonObject:
    return {
        "command": "pointer",
        "column": column,
        "row": row,
        "operation": {"kind": "click", "button": button},
        "strict": strict,
    }


def pointer_scroll_command(
    column: int,
    row: int,
    lines: int,
    *,
    strict: bool = False,
) -> JsonObject:
    return {
        "command": "pointer",
        "column": column,
        "row": row,
        "operation": {"kind": "scroll", "lines": lines},
        "strict": strict,
    }


def pointer_multi_click_command(
    column: int,
    row: int,
    count: int,
    *,
    button: str = "left",
    strict: bool = False,
) -> JsonObject:
    return {
        "command": "pointer",
        "column": column,
        "row": row,
        "operation": {"kind": "multi_click", "button": button, "count": count},
        "strict": strict,
    }


def pointer_drag_command(
    from_column: int,
    from_row: int,
    to_column: int,
    to_row: int,
    *,
    button: str = "left",
    steps: int = 8,
    strict: bool = False,
) -> JsonObject:
    return {
        "command": "pointer",
        "column": from_column,
        "row": from_row,
        "operation": {
            "kind": "drag",
            "button": button,
            "to_column": to_column,
            "to_row": to_row,
            "steps": steps,
        },
        "strict": strict,
    }


def pointer_move_command(
    column: int,
    row: int,
    *,
    strict: bool = False,
) -> JsonObject:
    return {
        "command": "pointer",
        "column": column,
        "row": row,
        "operation": {"kind": "move"},
        "strict": strict,
    }


def pointer_focus_command(
    focused: bool,
    *,
    strict: bool = False,
) -> JsonObject:
    return {"command": "focus", "focused": focused, "strict": strict}


def pointer_down_command(
    column: int,
    row: int,
    *,
    button: str = "left",
    strict: bool = False,
) -> JsonObject:
    return {
        "command": "pointer",
        "column": column,
        "row": row,
        "operation": {"kind": "down", "button": button},
        "strict": strict,
    }


def pointer_up_command(
    column: int,
    row: int,
    *,
    button: str = "left",
    strict: bool = False,
) -> JsonObject:
    return {
        "command": "pointer",
        "column": column,
        "row": row,
        "operation": {"kind": "up", "button": button},
        "strict": strict,
    }


def screen_wait_command(
    predicate: JsonObject,
    *,
    stable_for: Duration | int | float = Duration(0),
    timeout: Duration | int | float = Duration.seconds(10),
    frame: FrameRequest = FrameRequest(),
) -> JsonObject:
    stable_value = duration(stable_for)
    timeout_value = duration(timeout)
    if timeout_value.nanoseconds == 0:
        raise ValueError("screen wait timeout must be greater than zero")
    return {
        "command": "wait_screen",
        "predicate": predicate,
        "stable_for_ns": stable_value.nanoseconds,
        "timeout_ns": timeout_value.nanoseconds,
        "projection": encode_frame_request(frame),
    }


def screen_frame_request(
    options: FrameRequest = FrameRequest(),
) -> JsonObject:
    return {
        "method": "screen_frame",
        "point": {"selector": "latest_presented"},
        "include_hidden": False,
        "projection": encode_frame_request(options),
    }


def screen_observe_request() -> JsonObject:
    return {
        "method": "observe",
        "point": {"selector": "latest_presented"},
        "include_hidden": False,
    }


def output_wait_payload(data: bytes | memoryview) -> str:
    return base64.b64encode(bytes(data)).decode()


def events_page_request(
    *,
    from_event_seq: int = 0,
    limit: int = 256,
    metadata_only: bool = False,
) -> JsonObject:
    return {
        "method": "events",
        "from_event_seq": from_event_seq,
        "limit": limit,
        "metadata_only": metadata_only,
    }


def native_screenshot_request(
    destination: str | Path,
    *,
    point: JsonObject | None = None,
    viewport: tuple[int, int] | None = None,
) -> JsonObject:
    target: JsonValue = None if viewport is None else {"width": viewport[0], "height": viewport[1]}
    return {
        "method": "screenshot",
        "point": point or {"selector": "latest_presented"},
        "destination": str(destination),
        "renderer": "native",
        "viewport": target,
    }


def evidence_verify_request(
    level: Literal["structure", "content", "durability_record"] = "content",
) -> JsonObject:
    return {"method": "verify", "level": level}


def evidence_bundle_request(
    destination: str | Path,
    profile: Literal["screen", "debug", "forensic"] = "screen",
) -> JsonObject:
    return {
        "method": "bundle",
        "destination": str(destination),
        "profile": profile,
    }
