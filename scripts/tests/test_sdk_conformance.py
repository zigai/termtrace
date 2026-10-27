"""Fault injection for the single conformance aggregator (no parallel engine)."""

import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "sdk_conformance", Path(__file__).parents[1] / "sdk-conformance.py"
)
engine = importlib.util.module_from_spec(spec)
spec.loader.exec_module(engine)


class ConformanceEngineTests(unittest.TestCase):
    def test_verification_lock_selection(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "checkout"
            with patch.object(engine, "ROOT", root):
                self.assertEqual(
                    engine.verification_lock({}), root / "target/sdk-conformance.lock"
                )
                self.assertEqual(
                    engine.verification_lock({"TERMTRACE_VERIFICATION_LOCK": ""}),
                    root / "target/sdk-conformance.lock",
                )
                self.assertEqual(
                    engine.verification_lock(
                        {"TERMTRACE_VERIFICATION_LOCK": "shared/native.lock"}
                    ),
                    root / "shared/native.lock",
                )
                absolute = Path(directory) / "external/native.lock"
                self.assertEqual(
                    engine.verification_lock(
                        {"TERMTRACE_VERIFICATION_LOCK": str(absolute)}
                    ),
                    absolute,
                )

    def test_native_command_creates_and_uses_configured_lock(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / "checkout"
            for configured, expected in [
                (None, root / "target/sdk-conformance.lock"),
                ("shared/native.lock", root / "shared/native.lock"),
                (
                    str(Path(directory) / "external/native.lock"),
                    Path(directory) / "external/native.lock",
                ),
            ]:
                env = (
                    {}
                    if configured is None
                    else {"TERMTRACE_VERIFICATION_LOCK": configured}
                )
                with (
                    patch.object(engine, "ROOT", root),
                    patch.object(engine.subprocess, "Popen") as popen,
                ):
                    popen.return_value.communicate.return_value = ("ok", None)
                    popen.return_value.returncode = 0
                    result = engine.command(
                        ["test-adapter"], root / "logs", env, native=True
                    )
                    self.assertEqual(result["exit_code"], 0)
                    self.assertTrue(expected.parent.is_dir())
                    self.assertEqual(
                        popen.call_args.args[0],
                        ["flock", "--close", str(expected), "test-adapter"],
                    )
                    self.assertEqual(popen.call_args.kwargs["env"], env)
                    self.assertEqual(
                        result["command"], engine.shlex.join(popen.call_args.args[0])
                    )
            unused = root / "unused/native.lock"
            with patch.object(engine.subprocess, "Popen") as popen:
                popen.return_value.communicate.return_value = ("ok", None)
                popen.return_value.returncode = 0
                engine.command(
                    ["test-adapter"],
                    root / "logs",
                    {"TERMTRACE_VERIFICATION_LOCK": str(unused)},
                    native=False,
                )
                self.assertEqual(popen.call_args.args[0], ["test-adapter"])
                self.assertFalse(unused.parent.exists())

    def test_matrix_rejects_duplicate_unknown_and_stale_ids(self):
        matrix, _ = engine.inventory()
        for mutate in [
            lambda x: x["rows"].append(x["rows"][0]),
            lambda x: x["rows"][0]["cases"].update(positive="not-a-case"),
            lambda x: x["rows"][0]["languages"].pop("python"),
            lambda x: x["rows"][0]["core"].update(path="missing-source.rs"),
        ]:
            with tempfile.TemporaryDirectory() as directory:
                fixtures = Path(directory)
                (fixtures / "cases").mkdir()
                (fixtures / "cases/pending.json").write_text(
                    (engine.FIXTURES / "cases/pending.json").read_text()
                )
                (fixtures / "cases/portable-invalid.json").write_text(
                    (engine.FIXTURES / "cases/portable-invalid.json").read_text()
                )
                bad = copy.deepcopy(matrix)
                mutate(bad)
                (fixtures / "matrix.json").write_text(json.dumps(bad))
                with (
                    patch.object(engine, "FIXTURES", fixtures),
                    self.assertRaises(ValueError),
                ):
                    engine.inventory()

    def test_summary_without_evidence_is_not_certification(self):
        fixture = json.loads((engine.FIXTURES / "cases/foundation.json").read_text())
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            observation = dict(
                version=1,
                language="rust",
                case_id="foundation",
                rows=fixture["rows"],
                cols=fixture["cols"],
                text=fixture["text"],
                output_hex=fixture["output_hex"],
                active=False,
                stale=dict(
                    kind="stale_state",
                    code="stale_observation",
                    effect=dict(kind="not_applied"),
                    retry="new_request",
                ),
                session_path=str(root / "evidence/forged"),
            )
            (root / "observation.json").write_text(json.dumps(observation))
            with self.assertRaises(OSError):
                engine.validate_observation(root, "rust", fixture)

    def test_missing_adapter_and_timeout_are_not_success(self):
        with tempfile.TemporaryDirectory() as directory:
            result = engine.command(["/missing/adapter"], Path(directory), {})
            self.assertEqual(result["execution"], "unavailable")
            self.assertIsNone(result["exit_code"])
            result = engine.command(
                ["python3", "-c", "import time; time.sleep(10)"],
                Path(directory),
                dict(__import__("os").environ, SDK_CONFORMANCE_TIMEOUT_SECONDS="0.01"),
            )
            self.assertEqual(result["execution"], "timeout")
            self.assertIsNone(result["exit_code"])

    def test_forged_success_and_missing_adapter_fail_aggregation(self):
        def fake_command(argv, directory, env, *, native=False):
            directory.mkdir(parents=True, exist_ok=True)
            log = directory / "command.log"
            log.write_text("FORGED SUCCESS")
            return dict(
                command="forged", exit_code=0, execution="executed", log=str(log)
            )

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "run"
            with (
                patch(
                    "sys.argv",
                    [
                        "sdk-conformance.py",
                        "--output",
                        str(output),
                        "--without-adapter",
                        "node",
                    ],
                ),
                patch.object(engine, "command", fake_command),
                patch.object(
                    engine.subprocess, "check_output", return_value="test-version"
                ),
            ):
                self.assertEqual(engine.main(), 1)
            report = json.loads((output / "report.json").read_text())
            self.assertEqual(report["verdict"], "failed")
            self.assertTrue(
                all(entry["verdict"] == "failed" for entry in report["entries"])
            )
            self.assertTrue(
                all(
                    entry["execution"] == "missing"
                    and entry["availability"] == "unavailable"
                    for entry in report["entries"]
                    if entry["language"] == "node"
                )
            )


if __name__ == "__main__":
    unittest.main()
