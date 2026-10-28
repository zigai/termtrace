from termtrace import Events, Output
from termtrace.types import ByteOutputPage


def need_text(value: str) -> None:
    pass


def bad(page: ByteOutputPage, events: Events, output: Output) -> None:
    need_text(page["bytes"])
    need_text(page["cursor"]["byte_offset"])
    output.wait_for("READY")
    events.page(cursor=page["cursor"])
    events.follow(1.5)
