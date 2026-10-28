from termtrace import ActionResult, ScreenFrame
from termtrace.types import ArtifactProductKind, MediaArtifactResult


def consume(
    value: MediaArtifactResult, frame: ScreenFrame, result: ActionResult
) -> None:
    wide: int = value["members"][0]["byte_length"]
    raw: bytes = value["directory_bytes"]
    seq: int = value["source"]["boundary"]["next_event_seq"]
    kind: ArtifactProductKind = value["product_kind"]
    if value["provenance"]["kind"] == "presentation_output":
        source_kind = value["provenance"]["source_kind"]
        del source_kind
    schema: str | None = value["members"][0]["schema_type"]
    encoded: str = frame.to_json()
    decoded: ScreenFrame = ScreenFrame.from_json(encoded)
    replayed: ActionResult = ActionResult.from_json(result.to_json())
    del wide, raw, seq, kind, schema, decoded, replayed
