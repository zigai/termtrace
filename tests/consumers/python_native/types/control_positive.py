import asyncio
import uuid

from termtrace import ActionResult, Recording, Termtrace
from termtrace.aio import AsyncRecording
from termtrace.types import (
    LaunchOptionsInput,
    PointerOutcome,
    SendStepSpecInput,
    SessionStatus,
    VisualProfile,
    VisualProfileReport,
)


def consume(recording: Recording) -> None:
    builder = recording.actions().with_request_id(uuid.uuid4()).send_bytes(b"\x00\xff")
    request_id: str = builder.request_id
    builder.until_screen_where({"predicate": "contains", "text": "OK"}, timeout_ms=500)
    result: ActionResult = builder.run()
    written: int = result.input["bytes_written"]
    postcondition = result.postcondition
    if postcondition["kind"] == "screen":
        hash_or_none: str | None = postcondition["state_hash"]
        del hash_or_none
    pointer: PointerOutcome = recording.pointer().drag(0, 0, 4, 0, steps=3, strict=True)
    if pointer["kind"] == "applied":
        raw: bytes = pointer["bytes"]
        del raw
    steps: list[SendStepSpecInput] = [
        {"step": "input", "atoms": [{"bytes": b"\x1b"}, {"key": "enter"}]},
        {
            "step": "pointer",
            "column": 1,
            "row": 0,
            "operation": {"kind": "scroll", "lines": -1},
        },
    ]
    recording.send(
        steps, request_id=request_id, until={"condition": "idle", "stable_for_ms": 50}
    )
    status: SessionStatus = recording.status()
    if status["kind"] == "live":
        seq: int = status["boundaries"]["durable"]["next_event_seq"]
        del seq
    options: LaunchOptionsInput = {
        "environment": {"base": "clear", "set": {"X": b"\x01"}},
        "max_duration_ms": 100,
    }
    profile: VisualProfile = {
        "version": 1,
        "native": {
            "font_family": "Example Mono",
            "font_size_points": "13.5",
            "padding_points": {"x": 0, "y": 4},
        },
        "semantic": {"preset": "bundled-v1"},
    }
    options["visual_profile"] = profile
    client = Termtrace()
    report: VisualProfileReport = client.preflight_visual_profile(
        profile, cols=80, rows=24
    )
    profile_hash: str = report["profile_hash"]
    launched: Recording = client.launch(["/bin/true"], options)
    del written, profile_hash, launched


async def consume_async(recording: AsyncRecording) -> None:
    builder = recording.actions().send_text("x")
    pending = builder.run()
    result: ActionResult = await asyncio.wait_for(pending, 1.0)
    await recording.wait_pending()
    del result
