from termtrace import ScreenFrame
from termtrace.testing import (
    assert_count,
    assert_screen_text,
    assert_snapshot_file,
    masked_snapshot,
    terminal_test,
)


def invalid(frame: ScreenFrame) -> None:
    assert_screen_text("READY", "READY")
    masked_snapshot(frame, {"profile": "pixels"})
    assert_count("READY", 1)
    assert_snapshot_file(frame, "/tmp/snapshot", update="yes")
    terminal_test(evidence_policy="never")
