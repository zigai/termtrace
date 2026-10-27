from termtrace import Termtrace
from termtrace.evidence import (
    EvidenceApplicationAcknowledgement,
    EvidenceRegistryEntry,
    EvidenceSaveResult,
    EvidenceTimeline,
    EvidenceVerification,
)


def nested_reports(
    saved: EvidenceSaveResult,
    verified: EvidenceVerification,
    timeline: EvidenceTimeline,
    registered: EvidenceRegistryEntry,
    ack: EvidenceApplicationAcknowledgement,
) -> tuple[bytes, int, bool]:
    directory: bytes = saved["directory"]["bytes"]
    timestamp: int = registered["registered_unix_ns"]
    seq: int = ack["boundaries"]["durable"]["next_event_seq"]
    for entry in timeline["entries"]:
        if entry["item"]["kind"] == "semantic_state":
            state: int = entry["item"]["state_id"]
            timestamp += state
    return directory, timestamp + seq, verified["signature_signer_trusted"]


def surfaces(client: Termtrace) -> None:
    evidence = client.evidence(b"/source")
    evidence.save(
        b"/destination",
        ["png", "deep-json"],
        {"selector": "monotonic_ns", "monotonic_ns": 2**63},
    )
    evidence.verify("signature", b"/allowed-signers")
    client.registry(b"/state").restart("demo", {"command": [b"/bin/true"], "cols": 60})
    client.tmux().record(
        {
            "name": "demo",
            "new_server": True,
            "ghostty_profile": "off",
            "retain_bytes": 2**64 - 1,
        }
    )
    client.instrumentation(b"/source").event(
        "example", {"inode": 7, "payload": [1, None]}
    )


def nullable_wide(registered: EvidenceRegistryEntry) -> None:
    from termtrace.evidence import EvidenceRestartConfiguration, EvidenceTmuxGap

    copy: EvidenceRegistryEntry = {**registered, "recorder_process_start_ticks": None}
    options: EvidenceRestartConfiguration = {
        "output_byte_budget": None,
        "max_duration_ms": None,
    }

    def unresolved(gap: EvidenceTmuxGap) -> None:
        gap["gap"]["resolved_unix_ns"] = None

    assert copy and options
