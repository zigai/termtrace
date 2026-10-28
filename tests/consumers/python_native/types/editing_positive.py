from fractions import Fraction
from pathlib import Path

from termtrace import Edit, Scope, Select, Termtrace, normalize_edit
from termtrace.types import EditPlanInput, NormalizedEdit, PresentationPlan, VideoRender


def build(client: Termtrace, session: Path) -> str:
    edit = (
        Edit(resource_base=session.parent)
        .clip(
            Select.marker("start"),
            Select.event(2**64 - 1),
            speed=Fraction(2, 1),
            hold_ms=500,
        )
        .clip(Select.state(4), Select.native_frame(90), speed=(1, 2), caption="Slow")
        .annotate(
            "Saved",
            scope=Scope.range(Select.at_ns(0), Select.marker("end")),
            position={"anchor": "top_left", "x": 8, "y": -4},
        )
        .annotate(template="{clip}", scope=Scope.clip(0))
        .changes("semantic_cells", "outline", hold_frames=2)
        .caption_layout("overlay")
        .crop(0, 0, 640, 360)
        .callout(10, 10, 100, 20, label="here")
    )
    plan: EditPlanInput = edit.build()
    normalized: NormalizedEdit = normalize_edit(edit)
    presented: PresentationPlan = client.video_plan(
        session, edit, renderer="semantic", fps=30
    )
    frames: int = presented["output_frame_count"]
    rendered: VideoRender = client.video_render(
        session,
        edit.to_source(),
        session / "out.gif",
        renderer="native",
        gif_loop="once",
    )
    del plan, frames
    return normalized["edit_plan_hash"] + rendered["output"]
