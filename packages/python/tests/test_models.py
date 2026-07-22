"""Cross-language fixtures for the generated protocol boundary."""

import json
import unittest
from pathlib import Path

from termtrace import (
    ControlErrorDetails,
    InputSequenceFailureDetails,
    TermtraceOperationError,
    TermtraceProtocolError,
)
from termtrace.models import (
    DriverAttachedData,
    DriverHelloData,
    decode_driver_response,
    encode_driver_request,
)


FIXTURES = Path(__file__).parents[3] / "fixtures" / "protocol" / "driver-responses.json"


class DriverResponseTests(unittest.TestCase):
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

        self.assertEqual(parsed_hello.driver_protocol_version, 3)
        self.assertEqual(parsed_attached.canonical_session, "/tmp/example.termtrace")

    def test_rejects_uncorrelated_response(self) -> None:
        hello = self.cases["valid"][0]["response"]

        with self.assertRaisesRegex(TermtraceProtocolError, "request ID"):
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
                with self.assertRaises(TermtraceProtocolError) as captured:
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
        with self.assertRaises(TermtraceProtocolError) as captured:
            encode_driver_request(
                {"method": "hello", "secret_payload": "SECRET_SENTINEL"}, request_id
            )
        self.assertNotIn("SECRET_SENTINEL", str(captured.exception))


if __name__ == "__main__":
    unittest.main()
