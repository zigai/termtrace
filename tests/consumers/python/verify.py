"""Verify an installed wheel and its bundled recorder from outside the checkout."""

from pathlib import Path

from termtrace import Terminal

with Terminal(evidence_root=Path("python-evidence")) as terminal:
    with terminal.launch(
        ["/bin/sh", "-c", "printf installed-python-sdk; read -r answer"],
        evidence="always",
    ) as session:
        assert session.output.wait_for_text(
            "installed-python-sdk"
        ).output_end_offset == len("installed-python-sdk")
        span = session.application.start_span("installed SDK")
        session.application.event("consumer diagnostic", {"count": 1})
        session.application.end_span("installed SDK", span)
        assert (
            len(
                [
                    event
                    for event in session.events.page().events
                    if event.event_type == "application"
                ]
            )
            == 3
        )
        session.keyboard.type("done\n")
        assert session.complete(timeout_ms=10_000).exit_code == 0
        assert session.evidence.verify().valid
