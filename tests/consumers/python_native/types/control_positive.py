import asyncio
import uuid

from termtrace import ActionResult, Recording, TermtraceError
from termtrace.aio import AsyncRecording
from termtrace.control import LaunchOptions, PointerResult, SendStep, SessionStatus


def consume(recording: Recording) -> None:
    builder = recording.actions().with_request_id(uuid.uuid4()).send_bytes(b"\x00\xff")
    request_id: str = builder.request_id
    builder.until_screen_where({"predicate": "contains", "text": "OK"}, timeout_ms=500)
    result: ActionResult = builder.run()
    written: int = result.input["bytes_written"]
    hash_or_none: str | None = result.postcondition["state_hash"]
    pointer: PointerResult = recording.pointer().drag(0, 0, 4, 0, steps=3, strict=True)
    if pointer["kind"] == "applied":
        raw: bytes = pointer["bytes"]
        del raw
    steps: list[SendStep] = [
        {"step": "input", "atoms": [{"bytes": bytearray(b"\x1b")}, {"key": "enter"}]},
        {"step": "pointer", "column": 1, "row": 0, "operation": {"kind": "scroll", "lines": -1}},
    ]
    sent = recording.send(steps, request_id=request_id, until={"condition": "idle", "stable_for_ms": 50})
    last_id: str = sent["last"]["request_id"]
    status: SessionStatus = recording.status()
    if status["kind"] == "live":
        seq: int = status["boundaries"]["durable"]["next_event_seq"]
        del seq
    options: LaunchOptions = {"environment": {"base": "clear", "set": {"X": b"\x01"}}, "max_duration_ms": 100}
    try:
        recording.mark("x")
    except TermtraceError as error:
        failed: str | None = error.request_id
        del failed
    del written, hash_or_none, last_id, options


async def consume_async(recording: AsyncRecording) -> None:
    builder = recording.actions().send_text("x")
    pending = builder.run()
    result: ActionResult = await asyncio.wait_for(pending, 1.0)
    await recording.wait_pending()
    del result
