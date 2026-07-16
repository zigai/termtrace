# Termtrace for Python

Typed synchronous and asynchronous clients for the versioned Termtrace driver. The wheel includes the matching `termtrace` executable on supported platforms.

```python
from pathlib import Path

from termtrace import Terminal

with Terminal(evidence_root=Path(".termtrace-evidence")) as terminal:
    with terminal.launch(["my-terminal-app"], evidence="on-failure") as session:
        session.screen.wait_for_text("Ready")
        session.keyboard.type("help")
        session.keyboard.press("enter")

        frame = session.screen.frame()
        assert "Commands" in frame.viewport_text
```

`Frame`, `Observation`, `SemanticDiff`, `DiagnosticReport`, `Completion`, `VerificationReport`, and related return values are concrete generated models. Invalid or version-skewed driver messages raise `TermtraceProtocolError` without including the raw payload.

Use `AsyncTerminal` for the equivalent native-asyncio lifecycle.

The public session façade is handwritten. Internal request and response models are generated from the Rust-owned JSON Schemas and decoded with `msgspec`; do not edit files under `src/termtrace/_generated/`.
