from termtrace import ScreenFrame
from termtrace.types import MediaArtifactResult


def misuse(value: MediaArtifactResult) -> None:
    value["directory_bytes"] = "AA=="
    value["source"]["boundary"]["next_event_seq"] = "9007199254740993"
    value["product_kind"] = "movie"
    ScreenFrame.from_json(b"{}")
