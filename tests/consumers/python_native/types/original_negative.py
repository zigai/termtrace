from termtrace import OriginalRecording, Termtrace
from termtrace.control import OriginalLaunchOptions


def misuse(tt: Termtrace, original: OriginalRecording) -> None:
    options: OriginalLaunchOptions = {"rows": 24}
    other: OriginalLaunchOptions = {"fps": 25}
    tt.record_original(["/bin/true"], cancel=True)
    flag: str = original.finish()["canonical_success"]
    original.finish(timeout_ms=1.5)
    del options, other, flag
