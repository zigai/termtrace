import re

from termtrace import Screen
from termtrace.screen import StatePoint, StyleFilter


def misuse(screen: Screen, pattern: re.Pattern[str]) -> None:
    screen.get_by_regex(pattern)
    point: StatePoint = {"selector": "state_id", "state_id": "1"}
    style: StyleFilter = {"bold": None}
    screen.get_by_text("x").wait_for("present")
    screen.frame().rows.append(1)
    del point, style
