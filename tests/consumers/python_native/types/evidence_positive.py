from termtrace import Events, Output
from termtrace.evidence import EvidenceEventPage, EvidenceOutputPage


def byte_offset(page: EvidenceOutputPage) -> int:
    exact: bytes = page["bytes"]
    assert isinstance(exact, bytes)
    return page["cursor"]["byte_offset"]


def event_sequence(page: EvidenceEventPage) -> int:
    event = page["events"][0]["event"]
    if event is not None and event["type"] == "output":
        raw: bytes = event["data"]
        assert isinstance(raw, bytes)
    return page["cursor"]["next_event_seq"]


output = Output(b"/tmp/source")
events = Events(b"/tmp/source")
byte_offset(output.page())
event_sequence(events.page(event_types=["output"]))
output.wait_for(b"READY", 1.25)
events.follow(2**53 + 1)
