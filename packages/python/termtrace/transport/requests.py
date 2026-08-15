"""Pure driver request construction and validation."""

from __future__ import annotations

import base64
import math
import os
import uuid
from collections.abc import Sequence
from pathlib import Path

from termtrace.environment import EnvironmentOptions, encode_environment
from termtrace.transport.protocol import JsonObject, JsonValue, encode_driver_request


def correlated_request(
    method: JsonObject,
    request_id: str | None,
    timeout: float | None,
) -> tuple[str, bytes]:
    if timeout is not None and (not math.isfinite(timeout) or timeout <= 0):
        raise ValueError("request timeout must be finite and greater than zero")
    correlation_id = request_id or str(uuid.uuid4())
    return correlation_id, encode_driver_request(method, correlation_id)


def launch_request(
    command: Sequence[str | Path],
    *,
    default_evidence_root: Path | None,
    evidence_root: str | Path | None,
    owner: str,
    cols: int,
    rows: int,
    working_directory: str | Path | None,
    environment: EnvironmentOptions | None,
    output_byte_budget: int | None,
) -> JsonObject:
    root = Path(evidence_root) if evidence_root is not None else default_evidence_root
    if root is None:
        raise ValueError(f"launch requires evidence_root on {owner} or this call")
    encoded: list[JsonValue] = [
        base64.b64encode(os.fsencode(argument)).decode() for argument in command
    ]
    return {
        "method": "launch",
        "evidence_root": str(root),
        "command": encoded,
        "cols": cols,
        "rows": rows,
        "working_directory": None if working_directory is None else str(working_directory),
        "environment": encode_environment(environment),
        "resource_limits": {"output_bytes": output_byte_budget},
    }


def attach_request(session: str | Path) -> JsonObject:
    return {"method": "attach", "session": str(session)}


__all__ = ["attach_request", "correlated_request", "launch_request"]
