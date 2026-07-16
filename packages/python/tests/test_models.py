"""Tests for the typed protocol boundary."""

import unittest

from termtrace import TermtraceProtocolError
from termtrace.models import DriverResponse, JsonValue, parse_error


class DriverResponseTests(unittest.TestCase):
    def test_parses_correlated_success(self) -> None:
        response: JsonValue = {
            "version": 2,
            "request_id": "request-1",
            "result": {"status": "ok", "data": {"kind": "hello"}},
        }

        parsed = DriverResponse.parse(response, "request-1")

        self.assertEqual(parsed.data["kind"], "hello")

    def test_rejects_uncorrelated_response(self) -> None:
        response: JsonValue = {
            "version": 2,
            "request_id": "other-request",
            "result": {"status": "ok", "data": {}},
        }

        with self.assertRaisesRegex(TermtraceProtocolError, "request ID"):
            DriverResponse.parse(response, "request-1")

    def test_preserves_structured_operation_error(self) -> None:
        response: JsonValue = {
            "version": 2,
            "request_id": "request-1",
            "result": {
                "status": "error",
                "code": "stale_state",
                "message": "state changed",
                "retry_safety": "safe",
                "uncertain": False,
                "evidence": "session/events.jsonl",
            },
        }

        error = parse_error(response, "request-1")

        self.assertIsNotNone(error)
        assert error is not None
        self.assertEqual(error["code"], "stale_state")


if __name__ == "__main__":
    unittest.main()
