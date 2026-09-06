"""Reject mixed binary, SDK, and platform-package release versions."""

import json
import os
import tomllib
from pathlib import Path


def main() -> None:
    root = Path(__file__).resolve().parent.parent
    versions = {}
    for manifest in [
        root / "Cargo.toml",
        *sorted((root / "crates").glob("*/Cargo.toml")),
    ]:
        with manifest.open("rb") as source:
            package = tomllib.load(source)["package"]
        versions[package["name"]] = package["version"]
    with (root / "packages/python/pyproject.toml").open("rb") as source:
        versions["python"] = tomllib.load(source)["project"]["version"]
    typescript = json.loads((root / "packages/typescript/package.json").read_text())
    versions["typescript"] = typescript["version"]
    versions.update(typescript["optionalDependencies"])
    expected = versions["termtrace"]
    mismatched = {name: value for name, value in versions.items() if value != expected}
    if mismatched:
        raise SystemExit(f"release versions differ from {expected}: {mismatched}")
    if os.environ.get("GITHUB_REF_TYPE") == "tag":
        if os.environ.get("GITHUB_REF_NAME") != f"v{expected}":
            raise SystemExit(f"release tag must be v{expected}")


if __name__ == "__main__":
    main()
