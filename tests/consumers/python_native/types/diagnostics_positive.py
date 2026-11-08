from termtrace import Termtrace
from termtrace.types import (
    ApplicationAcknowledgement,
    PathValue,
    RegistryEntry,
    RestartConfigurationInput,
    SaveOutcome,
    TimelinePage,
    TmuxCoverageGap,
    VerificationReport,
)


def nested_reports(
    saved: SaveOutcome,
    verified: VerificationReport,
    timeline: TimelinePage,
    registered: RegistryEntry,
    ack: ApplicationAcknowledgement,
) -> tuple[PathValue, int, bool]:
    directory: str = saved["directory"]
    canonical: PathValue = saved["manifest"]["canonical_session"]
    size: int = saved["manifest"]["files"][0]["byte_count"]
    timestamp: int = registered["registered_unix_ns"]
    seq: int = ack["boundaries"]["durable"]["next_event_seq"]
    for entry in timeline["entries"]:
        if entry["item"]["kind"] == "semantic_state":
            state: int = entry["item"]["state_id"]
            timestamp += state
    del directory
    return canonical, timestamp + seq + size, verified["signature_signer_trusted"]


def surfaces(client: Termtrace) -> None:
    evidence = client.evidence(b"/source")
    evidence.save(
        b"/destination",
        ["png", "deep-json"],
        {"selector": "monotonic_ns", "monotonic_ns": 2**63},
    )
    evidence.bundle(b"/bundle", "forensic")
    evidence.verify("signature", b"/allowed-signers")
    timeline: TimelinePage = client.analysis(b"/source").timeline(limit=10)
    client.registry(b"/state").restart("demo", {"command": [b"/bin/true"], "cols": 60})
    client.tmux().record(
        {
            "name": "demo",
            "new_server": True,
            "ghostty_profile": "off",
            "coverage": "strict",
            "retain_bytes": 2**64 - 1,
        }
    )
    client.instrumentation(b"/source").event(
        "example", {"inode": 7, "payload": [1, None]}
    )
    del timeline


def nullable_wide(registered: RegistryEntry, gap: TmuxCoverageGap) -> None:
    copy: RegistryEntry = {**registered, "recorder_process_start_ticks": None}
    options: RestartConfigurationInput = {
        "output_byte_budget": None,
        "max_duration_ms": None,
    }
    gap["resolved_unix_ns"] = None
    assert copy and options
