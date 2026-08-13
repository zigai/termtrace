"""Typed child-environment options shared by both client transports."""

import base64
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from typing import Literal

from termtrace.models import JsonValue


@dataclass(frozen=True, slots=True)
class EnvironmentOptions:
    """Deterministic child-environment policy transported only through driver stdin."""

    inherit_env: bool = True
    set: Mapping[str, str | bytes] = field(default_factory=lambda: dict[str, str | bytes]())
    copy: Sequence[str] = ()
    unset: Sequence[str] = ()
    color: Literal["auto", "always", "never"] = "auto"


def encode_environment(options: EnvironmentOptions | None) -> dict[str, JsonValue]:
    """Encode environment values without placing their contents in diagnostics."""
    selected = options or EnvironmentOptions()
    encoded: dict[str, JsonValue] = {}
    for name, value in selected.set.items():
        raw = value.encode() if isinstance(value, str) else value
        encoded[name] = base64.b64encode(raw).decode()
    return {
        "base": "inherit" if selected.inherit_env else "clear",
        "set": encoded,
        "copy": list(selected.copy),
        "unset": list(selected.unset),
        "color": selected.color,
    }
