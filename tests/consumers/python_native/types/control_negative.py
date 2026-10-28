from termtrace import Recording
from termtrace.types import LaunchOptionsInput, SendStepSpecInput, VisualProfile


def misuse(recording: Recording) -> None:
    recording.actions().send_bytes("text")
    step: SendStepSpecInput = {"step": "input", "atoms": [{"bytes": "text"}]}
    options: LaunchOptionsInput = {"max_duration_ms": 1.5}
    recording.pointer().scroll(0, 0, lines=1.5)
    flag: str = recording.actions().run().matched
    float_size: VisualProfile = {"version": 1, "native": {"font_size_points": 13.5}}
    custom_preset: VisualProfile = {"version": 1, "semantic": {"preset": "custom"}}
    del step, options, flag, float_size, custom_preset
