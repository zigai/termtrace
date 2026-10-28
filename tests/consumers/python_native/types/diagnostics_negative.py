from termtrace import Termtrace
from termtrace.types import RegistryEntry, SaveOutcome, TmuxCoverageGap


def need_text(value: str) -> None:
    pass


def bad(saved: SaveOutcome, client: Termtrace) -> None:
    need_text(saved["manifest"]["files"][0]["byte_count"])
    client.evidence(b"/source").bundle(b"/destination", "automatic")
    client.evidence(b"/source").verify("sig")
    client.registry().restart("demo", {"command": ["/bin/true"]})
    client.instrumentation(b"/source").event("event", {"value": b"bytes"})
    client.tmux().record({"coverage": "secret"})


def need_integer(value: int) -> None:
    pass


def bad_nullable(registered: RegistryEntry, gap: TmuxCoverageGap) -> None:
    need_integer(registered["recorder_process_start_ticks"])
    need_integer(gap["resolved_unix_ns"])
