"""Deterministic packaged, environment, and PATH binary discovery."""

import os
import shutil
import sys
from pathlib import Path
from typing import NoReturn


def resolve_binary(explicit: str | Path | None = None) -> Path:
    """Resolve an explicit override before packaged, environment, or PATH binaries."""
    candidates: list[Path] = []
    if explicit is not None:
        candidates.append(Path(explicit))
    environment = os.environ.get("TERMTRACE_BINARY")
    if environment:
        candidates.append(Path(environment))
    candidates.append(Path(__file__).with_name("bin") / "termtrace")
    path_binary = shutil.which("termtrace")
    if path_binary is not None:
        candidates.append(Path(path_binary))
    for candidate in candidates:
        if candidate.is_file() and os.access(candidate, os.X_OK):
            return candidate.resolve()
    raise FileNotFoundError(
        "termtrace binary not found; install a platform wheel, set TERMTRACE_BINARY, "
        "or pass binary_path"
    )


def main() -> NoReturn:
    """Replace the Python launcher with the packaged Termtrace binary."""
    try:
        binary = resolve_binary()
        launcher = Path(sys.argv[0]).resolve()
        if binary == launcher:
            raise FileNotFoundError("packaged termtrace binary not found; install a platform wheel")
    except FileNotFoundError as e:
        print(f"termtrace: {e}", file=sys.stderr)
        raise SystemExit(127) from None
    os.execv(binary, [str(binary), *sys.argv[1:]])
