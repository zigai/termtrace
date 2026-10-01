import contextlib
import importlib.util
import io
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "check_architecture", Path(__file__).with_name("check-architecture.py")
)
assert spec is not None and spec.loader is not None
architecture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(architecture)


def package(name: str, dependencies: list[dict[str, str]]) -> dict[str, object]:
    return {
        "id": name,
        "name": name,
        "manifest_path": f"/workspace/{name}/Cargo.toml",
        "dependencies": dependencies,
    }


def metadata(*packages: dict[str, object]) -> dict[str, object]:
    return {
        "packages": list(packages),
        "workspace_members": [entry["id"] for entry in packages],
    }


class ArchitecturePolicyTests(unittest.TestCase):
    def check(self, workspace: dict[str, object]) -> tuple[int, str]:
        output = io.StringIO()
        errors = io.StringIO()
        with (
            patch.object(architecture, "load_metadata", return_value=workspace),
            contextlib.redirect_stdout(output),
            contextlib.redirect_stderr(errors),
        ):
            result = architecture.main()
        return result, errors.getvalue()

    def test_allows_removing_an_unused_dependency(self) -> None:
        workspace = metadata(
            package("termtrace-control", []), package("termtrace-model", [])
        )
        self.assertEqual(self.check(workspace), (0, ""))

    def test_allows_a_subset_of_permitted_dependencies(self) -> None:
        workspace = metadata(
            package(
                "termtrace-control",
                [{"name": "termtrace-model", "path": "/workspace/termtrace-model"}],
            ),
            package("termtrace-model", []),
        )
        self.assertEqual(self.check(workspace), (0, ""))

    def test_rejects_a_dependency_in_the_forbidden_direction(self) -> None:
        workspace = metadata(
            package(
                "termtrace-model",
                [{"name": "termtrace-engine", "path": "/workspace/termtrace-engine"}],
            ),
            package("termtrace-engine", []),
        )
        result, errors = self.check(workspace)
        self.assertEqual(result, 1)
        self.assertIn("termtrace-model -> termtrace-engine", errors)

    def test_rejects_an_unclassified_workspace_package(self) -> None:
        result, errors = self.check(metadata(package("termtrace-unclassified", [])))
        self.assertEqual(result, 1)
        self.assertIn("termtrace-unclassified", errors)

    def test_resolves_a_renamed_dependency_by_its_package_path(self) -> None:
        workspace = metadata(
            package(
                "termtrace-model",
                [
                    {
                        "name": "termtrace-engine",
                        "rename": "engine_alias",
                        "path": "/workspace/termtrace-engine",
                    }
                ],
            ),
            package("termtrace-engine", []),
        )
        result, errors = self.check(workspace)
        self.assertEqual(result, 1)
        self.assertIn("termtrace-model -> termtrace-engine", errors)

    def test_external_package_with_a_workspace_name_is_not_an_internal_edge(self) -> None:
        workspace = metadata(
            package("termtrace-model", [{"name": "termtrace-engine"}]),
            package("termtrace-engine", []),
        )
        self.assertEqual(architecture.internal_edges(workspace)[1], set())


if __name__ == "__main__":
    unittest.main()
