from termtrace import OriginalRecording, Termtrace
from termtrace.types import OriginalLaunchOptionsInput


def misuse(tt: Termtrace, original: OriginalRecording) -> None:
    options: OriginalLaunchOptionsInput = {"rows": 24}
    other: OriginalLaunchOptionsInput = {"fps": 29.97}
    tt.record_original(["/bin/true"], cancel=True)
    flag: str = original.finish()["canonical_success"]
    original.finish(timeout_ms=1.5)
    del options, other, flag
