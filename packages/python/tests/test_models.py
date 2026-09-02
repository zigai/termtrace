"""Cross-language fixtures for the generated protocol boundary."""

import copy
import json
import unittest
from pathlib import Path

from termtrace import (
    ActionBusyError,
    ControlErrorDetails,
    ControlResult,
    Frame,
    InputSequenceBusyError,
    InputSequenceFailureDetails,
    ProtocolMismatchError,
    ResourceLimitError,
    TermtraceOperationError,
    TimelineEntry,
)
from termtrace.transport.generated.driver_response import ScreenFrameFailureDetails
from termtrace.transport.protocol import (
    ControlResult as ModelControlResult,
)
from termtrace.transport.protocol import (
    DriverAttachedData,
    DriverHelloData,
    decode_driver_response,
    encode_driver_request,
)
from termtrace.transport.protocol import (
    Frame as ModelFrame,
)
from termtrace.transport.protocol import (
    TimelineEntry as ModelTimelineEntry,
)

FIXTURES = (
    Path(__file__).parents[3] / "fixtures" / "conformance" / "protocol" / "driver-responses.json"
)


class DriverResponseTests(unittest.TestCase):
    def test_package_root_preserves_public_model_exports(self) -> None:
        self.assertIs(ControlResult, ModelControlResult)
        self.assertIs(Frame, ModelFrame)
        self.assertIs(TimelineEntry, ModelTimelineEntry)

    def setUp(self) -> None:
        self.cases = json.loads(FIXTURES.read_text(encoding="utf-8"))

    def test_decodes_generated_success_variants(self) -> None:
        hello = self.cases["valid"][0]["response"]
        attached = self.cases["valid"][1]["response"]

        parsed_hello = decode_driver_response(
            json.dumps(hello).encode(), hello["request_id"], DriverHelloData
        )
        parsed_attached = decode_driver_response(
            json.dumps(attached).encode(),
            attached["request_id"],
            DriverAttachedData,
        )

        self.assertEqual(parsed_hello.driver_protocol_version, 5)
        self.assertEqual(parsed_attached.canonical_session, "/tmp/example.termtrace")

    def test_rejects_uncorrelated_response(self) -> None:
        hello = self.cases["valid"][0]["response"]

        with self.assertRaisesRegex(ProtocolMismatchError, "request ID"):
            decode_driver_response(
                json.dumps(hello).encode(),
                "00000000-0000-0000-0000-000000000099",
                DriverHelloData,
            )

    def test_preserves_structured_operation_error(self) -> None:
        response = self.cases["valid"][2]["response"]

        with self.assertRaises(TermtraceOperationError) as captured:
            decode_driver_response(
                json.dumps(response).encode(), response["request_id"], DriverHelloData
            )

        self.assertEqual(captured.exception.code, "stale_state")
        self.assertFalse(captured.exception.uncertain)
        details = captured.exception.details
        self.assertIsInstance(details, ControlErrorDetails)
        assert isinstance(details, ControlErrorDetails)
        self.assertEqual(details.scope, "state_guard")
        self.assertEqual(captured.exception.request_id, response["request_id"])

    def test_typed_driver_errors_preserve_retry_evidence_and_details(self) -> None:
        source = self.cases["valid"][2]["response"]
        error_types = (
            ("action_busy", ActionBusyError),
            ("input_sequence_busy", InputSequenceBusyError),
            ("screen_frame_resource_limit", ResourceLimitError),
        )

        for code, error_type in error_types:
            with self.subTest(code=code):
                response = copy.deepcopy(source)
                response["result"]["code"] = code
                response["result"]["retry_safety"] = "safe_same_request_id"
                response["result"]["uncertain"] = True
                if code == "screen_frame_resource_limit":
                    response["result"]["details"]["screen_frame"] = {
                        "cols": 160,
                        "rows": 45,
                        "detail": "cells",
                        "limit_kind": "cell_count",
                        "requested": 2_000_000,
                        "maximum": 1_000_000,
                    }

                with self.assertRaises(error_type) as captured:
                    decode_driver_response(
                        json.dumps(response).encode(),
                        response["request_id"],
                        DriverHelloData,
                    )

                error = captured.exception
                self.assertEqual(error.retry_safety, "safe_same_request_id")
                self.assertTrue(error.uncertain)
                self.assertEqual(error.evidence, "/tmp/example.termtrace")
                self.assertEqual(error.request_id, response["request_id"])
                self.assertIsInstance(error.details, ControlErrorDetails)
                if code == "screen_frame_resource_limit":
                    details = error.details
                    assert isinstance(details, ControlErrorDetails)
                    screen_frame = details.screen_frame
                    assert isinstance(screen_frame, ScreenFrameFailureDetails)
                    self.assertEqual(screen_frame.limit_kind, "cell_count")
                    self.assertEqual(screen_frame.requested, 2_000_000)
                    self.assertEqual(screen_frame.maximum, 1_000_000)

    def test_preserves_exact_partial_input_sequence_details(self) -> None:
        response = self.cases["valid"][3]["response"]

        with self.assertRaises(TermtraceOperationError) as captured:
            decode_driver_response(
                json.dumps(response).encode(), response["request_id"], DriverHelloData
            )

        details = captured.exception.details
        self.assertEqual(captured.exception.retry_safety, "safe_same_request_id")
        self.assertTrue(captured.exception.uncertain)
        self.assertIsInstance(details, ControlErrorDetails)
        self.assertEqual(captured.exception.evidence, "/tmp/example.termtrace")
        self.assertEqual(captured.exception.request_id, response["request_id"])
        assert isinstance(details, ControlErrorDetails)
        sequence = details.input_sequence
        self.assertIsInstance(sequence, InputSequenceFailureDetails)
        assert isinstance(sequence, InputSequenceFailureDetails)
        self.assertEqual(sequence.atoms_applied, 1)
        self.assertEqual(sequence.failed_atom_bytes_written, 2)
        self.assertEqual(sequence.resume_from_atom_index, 2)

    def test_rejects_every_invalid_shared_fixture_without_payload_leakage(self) -> None:
        for case in self.cases["invalid"]:
            with self.subTest(case=case["name"]):
                response = case["response"]
                with self.assertRaises(ProtocolMismatchError) as captured:
                    decode_driver_response(
                        json.dumps(response).encode(),
                        response["request_id"],
                        DriverHelloData,
                    )
                self.assertNotIn("SECRET_SENTINEL", str(captured.exception))

    def test_validates_outgoing_requests_without_payload_leakage(self) -> None:
        request_id = "00000000-0000-0000-0000-000000000020"

        encoded = encode_driver_request({"method": "hello"}, request_id)

        self.assertEqual(json.loads(encoded)["request_id"], request_id)
        with self.assertRaises(ProtocolMismatchError) as captured:
            encode_driver_request(
                {"method": "hello", "secret_payload": "SECRET_SENTINEL"}, request_id
            )
        self.assertNotIn("SECRET_SENTINEL", str(captured.exception))


if __name__ == "__main__":
    unittest.main()
