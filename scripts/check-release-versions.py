"""Reject mixed CLI, crate, PyPI, and npm release versions."""

import json
import os
from pathlib import Path

import tomllib


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
    with (root / "crates/bindings-python/pyproject.toml").open("rb") as source:
        versions["pypi:termtrace"] = tomllib.load(source)["project"]["version"]
    bindings_node = root / "crates/bindings-node"
    for manifest in [
        bindings_node / "package.json",
        *sorted((bindings_node / "npm").glob("*/package.json")),
    ]:
        package = json.loads(manifest.read_text())
        versions[f"npm:{package['name']}"] = package["version"]
    expected = versions["termtrace"]
    mismatched = {name: value for name, value in versions.items() if value != expected}
    if mismatched:
        raise SystemExit(f"release versions differ from {expected}: {mismatched}")
    if (
        os.environ.get("GITHUB_REF_TYPE") == "tag"
        and os.environ.get("GITHUB_REF_NAME") != f"v{expected}"
    ):
        raise SystemExit(f"release tag must be v{expected}")


if __name__ == "__main__":
    main()
