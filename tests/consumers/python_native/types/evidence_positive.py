from termtrace import Events, Output
from termtrace.types import (
    ByteOutputPage,
    EventPageCursor,
    ProjectedEventPage,
    WaitOutcome,
)


def byte_offset(page: ByteOutputPage) -> int:
    exact: bytes = page["bytes"]
    assert isinstance(exact, bytes)
    return page["cursor"]["byte_offset"]


def event_sequence(page: ProjectedEventPage) -> int:
    event = page["events"][0]["event"]
    if event is not None and event["type"] == "output":
        raw: bytes = event["data"]
        assert isinstance(raw, bytes)
    return page["cursor"]["next_event_seq"]


output = Output(b"/source")
events = Events(b"/source")
byte_offset(output.page())
first = events.page(event_types=["output"])
event_sequence(first)
resumed: EventPageCursor = events.cursor_from_json(
    events.cursor_to_json(first["cursor"])
)
event_sequence(events.page(cursor=resumed, metadata_only=True))
waited: WaitOutcome = output.wait_for(b"READY", 1.25)
end: int = waited["output_end_offset"]
events.follow(2**53 + 1)
