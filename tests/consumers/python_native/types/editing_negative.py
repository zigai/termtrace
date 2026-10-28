from pathlib import Path

from termtrace import Edit, Scope, Select, Termtrace
from termtrace.types import AnnotationPosition


def misuse(client: Termtrace, session: Path) -> None:
    Select.event("7")
    Edit().clip(Select.marker("a"), Select.marker("b"), speed=1.5)
    position: AnnotationPosition = {"anchor": "middle", "x": 0, "y": 0}
    Edit().changes("semantic_cells", "sparkle")
    client.video_render(session, Edit(), session / "out.mp4", renderer="pixels")
    hash_value: int = Edit().normalize()["edit_plan_hash"]
    Edit().annotate("x", scope=Scope.whole_video(), position=position)
    del hash_value
