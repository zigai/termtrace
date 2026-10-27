from termtrace import ScreenFrame, QueryResult, ScreenMatch
from termtrace.testing import (assert_screen_text, assert_count, assert_match_style, assert_cursor,
    normalized_snapshot, masked_snapshot, assert_snapshot_file, terminal_test, async_terminal_test)

def captured(frame: ScreenFrame, query: QueryResult, selected: ScreenMatch) -> str:
    assert_screen_text(frame, "READY")
    assert_count(query, 1)
    assert_match_style(selected, {"bold": True})
    assert_cursor(frame, frame.cursor)
    masked_snapshot(frame, {"profile": "cells", "mask_regions": []})
    assert_snapshot_file(frame, "/tmp/baseline", update=False)
    return normalized_snapshot(frame, profile="spans")

async def lifecycle() -> None:
    with terminal_test(evidence_policy="manual") as ctx:
        rec = ctx.record(["/bin/sh"])
        assert_screen_text(rec.screen().frame(), "READY")
    async with async_terminal_test(evidence_policy="always") as async_ctx:
        async_rec = await async_ctx.record(["/bin/sh"])
        assert_screen_text(await async_rec.screen().frame(), "READY")
