from pathlib import Path

from termtrace import OriginalCaptureFailedError, OriginalRecording, Termtrace
from termtrace.aio import AsyncOriginalRecording, AsyncTermtrace
from termtrace.control import OriginalLaunchOptions, OriginalRecordingResult


def finish(tt: Termtrace) -> bool:
    options: OriginalLaunchOptions = {
        "grid": {"cols": 80, "rows": 24},
        "canvas": {"width": 1280, "height": 720},
        "fps": 30,
    }
    original: OriginalRecording = tt.record_original(["/bin/true"], options)
    original.session().resize(100, 30)
    try:
        result: OriginalRecordingResult = original.finish(5000)
    except OriginalCaptureFailedError as error:
        exit_code = error.completion["exit_code"] if error.completion else None
        del exit_code
        return False
    seq: int = result["capture"]["source"]["boundary"]["next_event_seq"]
    del seq
    return result["canonical_success"]


async def finish_async(tt: AsyncTermtrace) -> int | None:
    original: AsyncOriginalRecording = await tt.record_original(["/bin/true"])
    result = await original.wait_finished()
    return result["completion"]["exit_code"]


def attach(tt: Termtrace, state: Path) -> int:
    named: OriginalRecording = tt.attach_original_target("name:demo", state_root=state)
    exact = tt.attach_original_target(b"/evidence/\xff/session", state_root=b"/state")
    path_bytes: bytes = exact.path_bytes
    session_bytes: bytes = exact.session().path_bytes
    del path_bytes, session_bytes
    pid: int = named.capture_status()["supervisor_pid"]
    return pid


async def attach_async(tt: AsyncTermtrace) -> bytes:
    named = await tt.attach_original_target(Path("/evidence/session"))
    await named.session().resize(100, 30)
    return named.session().path_bytes
