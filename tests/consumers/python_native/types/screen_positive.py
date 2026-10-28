from termtrace import CancelToken, LocatorSpec, LocatorTimeoutError, Screen, ScreenFrame
from termtrace.types import (
    CursorState,
    MatchedCell,
    StatePoint,
    StyleFilterInput,
    WideKind,
)


def consume(screen: Screen) -> None:
    frame = screen.frame(
        detail="cells", region={"top": 0, "left": 0, "bottom": 2, "right": 10}
    )
    height: int = frame.rows
    content = frame.content
    if content["detail"] == "cells":
        wide: WideKind = content["rows"][0]["cells"][0]["wide_kind"]
        del wide
    cursor: CursorState = frame.cursor
    row: int | None = cursor["row"]
    style: StyleFilterInput = {"bold": True, "resolved_foreground": None}
    spec = LocatorSpec.text("Save").with_style(style).in_row(0)
    match = screen.locator(spec).single()
    cells: list[MatchedCell] = match.cells
    width: int = cells[0]["width"]
    screen.get_by_text("Loading").wait_for(
        "absent", timeout_secs=5.0, cancel=CancelToken()
    )
    screen.get_by_regex("(?i)save").wait_for({"count": 2})
    point: StatePoint = {"selector": "state_id", "state_id": 2**64 - 1}
    observation = screen.observe(point)
    state_id: int | None = observation["state_id"]
    at_point: ScreenFrame = screen.frame_at(point, detail="text")
    capture = screen.capture_stable(
        stable_for_secs=0.05, reaction={"timeout_secs": 0.1}
    )
    zstd: bytes | None = capture["semantic_state_zstd"]
    last: ScreenFrame = capture["frame"]
    outcome = screen.wait_for({"predicate": "row_contains", "row": 0, "text": "READY"})
    satisfied: bool = outcome["satisfied"]
    try:
        screen.get_by_text("x").wait_for(timeout_secs=0.001)
    except LocatorTimeoutError as error:
        message: str = str(error)
        del message
    del height, row, width, state_id, at_point, zstd, last, satisfied
