import asyncio

import msgspec
import pytest

from termtrace.atoms import TextAtom
from termtrace.errors import ProtocolMismatchError, TermtraceOperationError
from termtrace.screen.frame import FrameRequest, Region
from termtrace.session.actions import ActionOptions
from termtrace.session.async_ import AsyncScreen, AsyncSession
from termtrace.session.sync import Screen, Session
from termtrace.transport.protocol import ControlResult, DriverScreenFrameData, JsonObject


def _wire_frame(state_hash: str) -> dict[str, object]:
    color = {"kind": "none"}
    style = {
        "foreground": color,
        "background": color,
        "underline_color": color,
        "bold": False,
        "faint": False,
        "italic": False,
        "underline": 0,
        "blink": False,
        "inverse": False,
        "invisible": False,
        "strikethrough": False,
        "overline": False,
    }
    appearance = {
        "style": style,
        "resolved_foreground": None,
        "resolved_background": None,
        "selected": False,
    }
    return {
        "version": 1,
        "state_hash": state_hash,
        "frame_hash": f"projection-{state_hash}",
        "presented": True,
        "capture_quality": "live",
        "source": {
            "source": "actor",
            "session_id": "00000000-0000-0000-0000-000000000001",
            "boundaries": {
                "durable": {"event_hash": "d", "next_event_seq": 1, "output_bytes": 1},
                "observed": {"event_hash": "o", "next_event_seq": 1, "output_bytes": 1},
            },
            "observed_monotonic_ns": 1,
        },
        "geometry": {"cols": 1, "rows": 1},
        "requested_region": {"top": 0, "left": 0, "right": 1, "bottom": 1},
        "returned_region": {"top": 0, "left": 0, "right": 1, "bottom": 1},
        "cursor": {
            "row": 0,
            "column": 0,
            "visible": True,
            "blinking": False,
            "style": "block",
            "in_viewport": True,
            "wide_tail": False,
        },
        "modes": {
            "alternate_screen": False,
            "bracketed_paste": False,
            "synchronized_output": False,
            "wraparound": True,
            "values": [],
        },
        "colors": {
            "foreground": {"r": 255, "g": 255, "b": 255},
            "background": {"r": 0, "g": 0, "b": 0},
            "cursor": None,
            "palette": [],
        },
        "data": {
            "detail": "cells",
            "rows": [
                {
                    "row": 0,
                    "soft_wrapped": False,
                    "wrap_continuation": False,
                    "text": "x",
                    "cells": [
                        {
                            "row": 0,
                            "column": 0,
                            "grapheme": "x",
                            "wide_kind": "narrow",
                            "appearance": appearance,
                        }
                    ],
                }
            ],
        },
    }


class _SyncSession:
    def __init__(self) -> None:
        self.requests: list[dict[str, object] | JsonObject] = []
        self._completed = False

    def request(
        self,
        method: JsonObject,
        response_type: type[DriverScreenFrameData],
        request_id: str | None = None,
        request_timeout: float | None = None,
    ) -> DriverScreenFrameData:
        assert request_timeout is None
        self.requests.append(method)
        assert response_type is DriverScreenFrameData
        return msgspec.convert(
            {"frame": _wire_frame("captured")},
            type=DriverScreenFrameData,
            strict=True,
        )

    def control(self, command: dict[str, object]) -> dict[str, object]:
        self.requests.append(command)
        if command["command"] == "capture_stable":
            return {
                "kind": "stable_snapshot",
                "quality": "stable",
                "first_observed_monotonic_ns": 1,
                "observed_monotonic_ns": 2,
                "stable_for_ns": 1,
                "frame": _wire_frame("stable"),
            }
        return _wire_frame("captured")


class _AsyncSession:
    def __init__(self) -> None:
        self.requests: list[dict[str, object] | JsonObject] = []
        self._completed = False

    async def request(
        self,
        method: JsonObject,
        response_type: type[DriverScreenFrameData],
        request_id: str | None = None,
        request_timeout: float | None = None,
    ) -> DriverScreenFrameData:
        assert request_timeout is None
        self.requests.append(method)
        assert response_type is DriverScreenFrameData
        return msgspec.convert(
            {"frame": _wire_frame("captured")},
            type=DriverScreenFrameData,
            strict=True,
        )

    async def control(self, command: dict[str, object]) -> dict[str, object]:
        self.requests.append(command)
        if command["command"] == "capture_stable":
            return {
                "kind": "stable_snapshot",
                "quality": "stable",
                "first_observed_monotonic_ns": 1,
                "observed_monotonic_ns": 2,
                "stable_for_ns": 1,
                "frame": _wire_frame("stable"),
            }
        return _wire_frame("captured")


class _ActionSession(Session):
    def __init__(self) -> None:
        self.requests: list[JsonObject] = []

    def control(
        self,
        command: JsonObject,
        *,
        request_id: str | None = None,
        if_state: str | None = None,
        request_timeout: float | None = None,
    ) -> ControlResult:
        assert request_timeout is None
        self.requests.append(command)
        input_result: dict[str, object] = {
            "atom_count": 1,
            "bytes_written": 1,
            "first_intent_event_seq": 1,
            "last_result_event_seq": 2,
            "pace_ns": 0,
            "sequence_id": "00000000-0000-0000-0000-000000000001",
        }
        if command["include_atom_outcomes"] is True:
            input_result["atoms"] = [
                {
                    "operation_id": "00000000-0000-0000-0000-000000000002",
                    "input_kind": "text",
                    "bytes_written": 1,
                    "intent_event_seq": 1,
                    "result_event_seq": 2,
                }
            ]
        result: ControlResult = msgspec.convert(
            {
                "boundaries": {
                    "durable": {
                        "event_hash": "d",
                        "next_event_seq": 1,
                        "output_bytes": 1,
                    },
                    "observed": {
                        "event_hash": "o",
                        "next_event_seq": 1,
                        "output_bytes": 1,
                    },
                },
                "kind": "action_applied",
                "input": input_result,
                "postcondition": {
                    "kind": "none",
                    "matched": True,
                    "frame": _wire_frame("action"),
                },
            },
            type=ControlResult,
            strict=True,
        )
        return result


class _ActionErrorSession(Session):
    def __init__(self) -> None:
        self.request_id: str | None = None

    def control(
        self,
        command: JsonObject,
        *,
        request_id: str | None = None,
        if_state: str | None = None,
        request_timeout: float | None = None,
    ) -> ControlResult:
        assert request_timeout is None
        self.request_id = request_id
        raise TermtraceOperationError(
            "terminating frame projection failed",
            retry_safety="safe_same_request_id",
        )


class _AsyncActionSession(AsyncSession):
    def __init__(self) -> None:
        self.requests: list[JsonObject] = []

    async def control(
        self,
        command: JsonObject,
        *,
        request_id: str | None = None,
        if_state: str | None = None,
        request_timeout: float | None = None,
    ) -> ControlResult:
        assert request_timeout is None
        self.requests.append(command)
        result: ControlResult = msgspec.convert(
            {
                "boundaries": {
                    "durable": {
                        "event_hash": "d",
                        "next_event_seq": 1,
                        "output_bytes": 1,
                    },
                    "observed": {
                        "event_hash": "o",
                        "next_event_seq": 1,
                        "output_bytes": 1,
                    },
                },
                "kind": "action_applied",
                "input": {
                    "atom_count": 1,
                    "bytes_written": 1,
                    "first_intent_event_seq": 1,
                    "last_result_event_seq": 2,
                    "pace_ns": 0,
                    "sequence_id": "00000000-0000-0000-0000-000000000001",
                },
                "postcondition": {
                    "kind": "none",
                    "matched": True,
                    "frame": _wire_frame("async-action"),
                },
            },
            type=ControlResult,
            strict=True,
        )
        return result


def test_sync_frame_and_stable_use_only_the_actor_returned_exact_frame() -> None:
    session = _SyncSession()
    screen = Screen(session)  # type: ignore[arg-type]
    assert screen.cells(Region(0, 0, 1, 1)).state_hash == "captured"
    capture = screen.capture_stable(frame=FrameRequest("cells", Region(0, 0, 1, 1)))
    assert capture.frame.state_hash == "stable"
    assert [request.get("command") or request.get("method") for request in session.requests] == [
        "screen_frame",
        "capture_stable",
    ]


def test_sync_act_uses_only_its_action_response_frame() -> None:
    session = _ActionSession()
    result = session.act(
        [TextAtom("x")],
        ActionOptions(request_id="00000000-0000-0000-0000-000000000001"),
    )
    assert result.frame.state_hash == "action"
    assert [request["command"] for request in session.requests] == ["act"]


def test_sync_act_rejects_mismatched_canonical_sequence_id() -> None:
    session = _ActionSession()

    with pytest.raises(ProtocolMismatchError, match="canonical sequence ID"):
        session.act(
            [TextAtom("x")],
            ActionOptions(request_id="00000000-0000-0000-0000-000000000099"),
        )


def test_sync_act_retains_verbose_atom_input_kind() -> None:
    session = _ActionSession()

    result = session.act(
        [TextAtom("x")],
        ActionOptions(
            request_id="00000000-0000-0000-0000-000000000001",
            verbose_atom_outcomes=True,
        ),
    )

    assert result.input.atom_outcomes is not None
    assert result.input.atom_outcomes[0].input_kind == "text"


def test_sync_act_exposes_generated_request_id_on_operation_error() -> None:
    session = _ActionErrorSession()

    with pytest.raises(TermtraceOperationError) as captured:
        session.act([TextAtom("x")])

    assert session.request_id is not None
    assert captured.value.request_id == session.request_id


def test_async_act_uses_only_its_action_response_frame() -> None:
    async def exercise() -> None:
        session = _AsyncActionSession()
        result = await session.act(
            [TextAtom("x")],
            ActionOptions(request_id="00000000-0000-0000-0000-000000000001"),
        )
        assert result.frame.state_hash == "async-action"
        assert [request["command"] for request in session.requests] == ["act"]

    asyncio.run(exercise())


def test_async_frame_and_stable_share_domain_types_and_request_counts() -> None:
    async def exercise() -> None:
        session = _AsyncSession()
        screen = AsyncScreen(session)  # type: ignore[arg-type]
        frame = await screen.cells(Region(0, 0, 1, 1))
        capture = await screen.capture_stable(frame=FrameRequest("cells", Region(0, 0, 1, 1)))
        assert type(frame) is type(capture.frame)
        assert capture.frame.state_hash == "stable"
        assert [
            request.get("command") or request.get("method") for request in session.requests
        ] == [
            "screen_frame",
            "capture_stable",
        ]

    asyncio.run(exercise())
