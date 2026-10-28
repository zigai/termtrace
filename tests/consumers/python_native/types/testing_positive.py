from termtrace import QueryResult, ScreenFrame, ScreenMatch
from termtrace.testing import (
    FinalizationReport,
    assert_count,
    assert_cursor,
    assert_match_style,
    assert_not,
    assert_screen_text,
    assert_snapshot_file,
    async_terminal_test,
    masked_snapshot,
    normalized_snapshot,
    terminal_test,
)


def captured(frame: ScreenFrame, query: QueryResult, selected: ScreenMatch) -> str:
    assert_screen_text(frame, "READY")
    assert_count(query, 1)
    assert_match_style(selected, {"bold": True})
    assert_cursor(
        frame,
        {
            "blinking": False,
            "in_viewport": True,
            "row": 0,
            "column": 4,
            "style": "block",
            "visible": True,
        },
    )
    assert_not(lambda: assert_screen_text(frame, "ERROR"))
    masked_snapshot(frame, {"profile": "cells", "mask_regions": []})
    assert_snapshot_file(frame, "/tmp/baseline", update=False)
    return normalized_snapshot(frame, profile="spans")


async def lifecycle() -> None:
    with terminal_test(evidence_policy="manual") as ctx:
        rec = ctx.record(["/bin/sh"])
        assert_screen_text(rec.screen().frame(), "READY")
        report: FinalizationReport = ctx.finalize("passed")
        del report
    async with async_terminal_test(evidence_policy="always") as async_ctx:
        async_rec = await async_ctx.record(["/bin/sh"])
        assert_screen_text(await async_rec.screen().frame(), "READY")
