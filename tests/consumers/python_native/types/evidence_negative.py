from termtrace import Events, Output
from termtrace.evidence import EvidenceOutputPage


def need_text(value: str) -> None:
    pass


def bad(page: EvidenceOutputPage, events: Events, output: Output) -> None:
    need_text(page["bytes"])
    need_text(page["cursor"]["byte_offset"])
    output.wait_for("READY")
    events.page(event_types=["invented"])
    events.follow(1.5)
