"""Regression checks for acceptance decisions, not latency measurements."""

from __future__ import annotations

import hashlib
import io
import json
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import patch

from acceptance_models import Gate, Sample, latency_result, percentile
from models import main as emit_model


class AcceptanceEvaluationTests(unittest.TestCase):
    def test_nearest_rank_uses_29th_sample_for_thirty_p95_values(self) -> None:
        values = list(range(1, 31))
        self.assertEqual(
            [percentile(values, rank) for rank in (50, 95, 99)], [15, 29, 30]
        )

    def test_actor_clock_uncertainty_cannot_turn_boundary_failure_into_pass(
        self,
    ) -> None:
        gate = Gate("notification", "latency_delta", maximum_delta_ms=50)
        samples = [Sample(50_000_000, 1024, 1, ("wait",), "wait.json", 1)] * 30
        result = latency_result(gate, samples, True, False, 536_870_912)
        self.assertEqual(result.status, "failed")
        self.assertEqual(result.distribution.p95_ns, 50_000_000)

    def test_fast_samples_on_wrong_host_are_not_acceptance(self) -> None:
        gate = Gate("status", "latency", maximum_ms=100)
        samples = [Sample(10, 1024, 1, ("status",), "status.json")] * 30
        self.assertEqual(
            latency_result(gate, samples, False, False, 536_870_912).status,
            "ineligible",
        )
        self.assertEqual(
            latency_result(gate, samples, True, True, 536_870_912).status, "reduced"
        )

    def test_memory_violation_cannot_hide_behind_fast_latency(self) -> None:
        gate = Gate("status", "latency", maximum_ms=100)
        samples = [Sample(10, 536_870_913, 1, ("status",), "status.json")] * 30
        self.assertEqual(
            latency_result(gate, samples, True, False, 536_870_912).status, "failed"
        )

    def test_manifest_labels_actual_binary_not_checkout_revision(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            binary = Path(directory) / "old-termtrace"
            binary.write_text(
                f'#!{sys.executable}\nprint(\'{{"build_revision":"old-build"}}\')\n'
            )
            binary.chmod(0o700)
            argv = [
                "models.py",
                "emit-run-manifest",
                "--command",
                "agent",
                "--revision",
                "new-checkout",
                "--binary",
                str(binary),
                "--started-unix-ns",
                "1",
                "--host",
                "test-host",
            ]
            stdout = io.StringIO()
            with patch.object(sys, "argv", argv), redirect_stdout(stdout):
                emit_model()
            result = json.loads(stdout.getvalue())
            self.assertEqual(result["build_revision"], "old-build")
            self.assertEqual(result["source_checkout_commit"], "new-checkout")
            self.assertEqual(
                result["binary_sha256"], hashlib.sha256(binary.read_bytes()).hexdigest()
            )
            self.assertEqual(result["build_profile"], "unverified-existing-artifact")


if __name__ == "__main__":
    unittest.main()
