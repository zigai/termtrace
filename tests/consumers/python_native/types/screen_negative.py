import re

from termtrace import Screen
from termtrace.types import StatePoint, StyleFilterInput


def misuse(screen: Screen, pattern: re.Pattern[str]) -> None:
    screen.get_by_text(pattern)
    point: StatePoint = {"selector": "state_id", "state_id": "1"}
    style: StyleFilterInput = {"bold": None}
    screen.get_by_text("x").wait_for("present")
    screen.frame().rows.append(1)
    del point, style
