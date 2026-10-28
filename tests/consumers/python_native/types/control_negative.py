from termtrace import Recording
from termtrace.control import LaunchOptions, SendStep
from termtrace.types import VisualProfile


def misuse(recording: Recording) -> None:
    recording.actions().send_bytes("text")
    step: SendStep = {"step": "input", "atoms": [{"bytes": "text"}]}
    options: LaunchOptions = {"max_duration_ms": 1.5}
    recording.pointer().click(0, 0, button="side")
    flag: str = recording.actions().run().matched
    float_size: VisualProfile = {"version": 1, "native": {"font_size_points": 13.5}}
    custom_preset: VisualProfile = {"version": 1, "semantic": {"preset": "custom"}}
    del step, options, flag, float_size, custom_preset
