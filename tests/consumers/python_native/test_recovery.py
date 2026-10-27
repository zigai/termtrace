"""Native recovery binds byte paths, never changes its source or publishes partial trees."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
from pathlib import Path

import pytest
from termtrace import Events, Termtrace, TermtraceError
from test_evidence_pages import BINARY, evidence_root


def hashes(root: bytes) -> dict[str, str]:
    return {
        os.path.relpath(os.path.join(directory, name), root).hex(): hashlib.sha256(
            Path(os.fsdecode(os.path.join(directory, name))).read_bytes()
        ).hexdigest()
        for directory, _, files in os.walk(root)
        for name in files
    }


def assert_path(value: object, expected: bytes) -> None:
    if isinstance(value, str):
        assert value.encode() == expected
    else:
        assert isinstance(value, list)
        assert bytes(value) == expected
        with pytest.raises(UnicodeDecodeError):
            expected.decode()


@pytest.mark.parametrize(
    ("source_suffix", "destination_suffix"),
    [
        ("source-é".encode(), "dest-λ".encode()),
        (b"source-\xff", b"dest"),
        (b"source", b"dest-\xfe"),
        (b"source-\xff", b"dest-\xfe"),
    ],
)
def test_non_utf8_recovery_source_hashes_and_atomic_failures(
    tmp_path: Path,
    source_suffix: bytes,
    destination_suffix: bytes,
) -> None:
    root = evidence_root(tmp_path)
    root.chmod(0o700)
    client = Termtrace(binary=BINARY, evidence_root=str(root))
    recording = client.launch(["/bin/sh", "-c", "printf RECOVERY; read line"])
    try:
        recording.output().wait_for(b"RECOVERY", timeout_secs=5)
        recording.stop()
    finally:
        if recording.is_active():
            recording.stop()
    source = os.fsencode(root) + b"/" + source_suffix
    destination = os.fsencode(root) + b"/" + destination_suffix
    shutil.copytree(os.fsencode(recording.path), source)
    with open(source + b"/events.jsonl", "ab") as stream:
        stream.write(b'{"torn":')
    before = hashes(source)
    report = client.evidence(source).recover(destination)
    assert os.fsencode(report["source"]) == source + b"/events.jsonl"
    assert os.fsencode(report["recovered"]) == destination + b"/events.jsonl"
    assert isinstance(report["valid_events"], int) and report["valid_events"] > 0
    assert report["discarded_tail"] is not None
    after = hashes(source)
    assert before == after
    diagnostic = json.loads(
        Path(os.fsdecode(destination + b"/recovery.json")).read_bytes()
    )
    assert diagnostic["complete_evidence"] is False
    assert_path(diagnostic["source_session"], source)
    assert_path(diagnostic["recovery"]["source"], source + b"/events.jsonl")
    assert_path(diagnostic["recovery"]["recovered"], destination + b"/events.jsonl")
    assert not os.path.exists(destination + b"/completion.json")
    # Validate the entire finite recovered prefix through the shared native scanner.
    events = Events(destination).page(limit=1000)
    assert len(events["events"]) == report["valid_events"]
    assert events["has_more"] is False and events["incomplete_tail"] is False
    published = hashes(destination)
    with pytest.raises(TermtraceError):
        client.evidence(source).recover(destination)
    assert published == hashes(destination)
    with pytest.raises(TermtraceError):
        client.evidence(source).recover(source)
    assert before == hashes(source)
    with pytest.raises(TermtraceError):
        client.evidence(source).recover(source + b"/nested-recovery")
    alias = os.fsencode(root) + b"/source-link"
    os.symlink(source, alias)
    with pytest.raises(TermtraceError):
        client.evidence(source).recover(alias + b"/nested-recovery")
    assert not os.path.lexists(source + b"/nested-recovery")
    assert before == hashes(source)
    # Individual log recovery uses the same byte-bound report serializer/projection.
    log = client.evidence(source + b"/events.jsonl").recover_log(
        os.fsencode(root) + b"/prefix-\xfd.jsonl"
    )
    assert os.fsencode(log["source"]) == source + b"/events.jsonl"
    assert log["valid_events"] == report["valid_events"]
    Path(os.fsdecode(source + b"/events.jsonl")).write_bytes(b"invalid header\n")
    broken = hashes(source)
    absent = os.fsencode(root) + b"/absent-\xfc"
    with pytest.raises(TermtraceError):
        client.evidence(source).recover(absent)
    assert not os.path.lexists(absent)
    assert broken == hashes(source)
    assert not any(root.glob(".termtrace-recovery-*"))
    (root / "recovery-observed.json").write_text(
        json.dumps(
            {
                "source_hex": source.hex(),
                "destination_hex": destination.hex(),
                "source_hashes_before": before,
                "source_hashes_after": after,
                "valid_events": report["valid_events"],
                "existing_destination_unchanged": True,
                "failed_destination_absent": True,
                "broken_source_hashes": broken,
            },
            indent=2,
        )
    )
