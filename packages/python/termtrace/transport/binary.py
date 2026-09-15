"""Deterministic packaged, environment, and PATH binary discovery."""

import os
import shutil
import sys
from pathlib import Path
from typing import NoReturn


def _is_python_script_wrapper(path: Path) -> bool:
    """Check whether an executable path is a Python entrypoint script wrapper."""
    try:
        resolved = path.resolve()
        if sys.argv and sys.argv[0]:
            try:
                if Path(sys.argv[0]).resolve() == resolved:
                    return True
            except OSError:
                pass
        with path.open("rb") as f:
            first_line = f.readline(256)
            if first_line.startswith(b"#!") and b"python" in first_line.lower():
                content = f.read(1024)
                if b"termtrace" in content and b"binary" in content:
                    return True
    except (OSError, UnicodeDecodeError):
        pass
    return False


def resolve_binary(
    explicit: str | Path | None = None,
    ignore_launchers: set[Path] | None = None,
) -> Path:
    """Resolve an explicit override before packaged, environment, or PATH binaries."""
    candidates: list[Path] = []
    if explicit is not None:
        candidates.append(Path(explicit))
    environment = os.environ.get("TERMTRACE_BINARY")
    if environment:
        candidates.append(Path(environment))
    candidates.append(Path(__file__).parent.parent / "bin" / "termtrace")
    path_binary = shutil.which("termtrace")
    if path_binary is not None:
        candidates.append(Path(path_binary))

    ignored = {p.resolve() for p in ignore_launchers} if ignore_launchers else set()

    for candidate in candidates:
        try:
            resolved = candidate.resolve()
            if resolved in ignored:
                continue
            if candidate.is_file() and os.access(candidate, os.X_OK):
                if _is_python_script_wrapper(candidate):
                    continue
                return resolved
        except OSError:
            continue
    raise FileNotFoundError(
        "termtrace binary not found; install a platform wheel, set TERMTRACE_BINARY, "
        "or pass binary_path"
    )


def main() -> NoReturn:
    """Replace the Python launcher with the packaged Termtrace binary."""
    try:
        launcher = Path(sys.argv[0]).resolve()
        binary = resolve_binary(ignore_launchers={launcher})
    except FileNotFoundError as e:
        print(f"termtrace: {e}", file=sys.stderr)
        raise SystemExit(127) from None
    os.execv(binary, [str(binary), *sys.argv[1:]])
