from termtrace import Termtrace
from termtrace.evidence import EvidenceSaveResult, EvidenceRegistryEntry, EvidenceTmuxGap


def need_text(value: str) -> None:
    pass


def bad(saved: EvidenceSaveResult, client: Termtrace) -> None:
    need_text(saved["manifest"]["files"][0]["byte_count"])
    client.evidence(b"/source").bundle(b"/destination", "automatic")
    client.evidence(b"/source").verify("sig")
    client.registry().restart("demo", {"command": ["/bin/true"]})
    client.instrumentation(b"/source").event("event", {"value": b"bytes"})
    client.tmux().record({"coverage": "secret"})


def need_integer(value: int) -> None:
    pass


def bad_nullable(registered: EvidenceRegistryEntry, gap: EvidenceTmuxGap) -> None:
    need_integer(registered["recorder_process_start_ticks"])
    need_integer(gap["gap"]["resolved_unix_ns"])
