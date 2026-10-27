from termtrace import Recording
from termtrace.control import LaunchOptions, SendStep


def misuse(recording: Recording) -> None:
    recording.actions().send_bytes("text")
    step: SendStep = {"step": "input", "atoms": [{"bytes": "text"}]}
    options: LaunchOptions = {"max_duration_ms": 1.5}
    recording.pointer().click(0, 0, button="side")
    flag: str = recording.actions().run().matched
    del step, options, flag
