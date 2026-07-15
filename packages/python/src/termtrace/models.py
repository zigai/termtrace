"""Parsed driver boundary models."""

from dataclasses import dataclass
from typing import TypeAlias

from termtrace.errors import TermtraceProtocolError

JsonScalar: TypeAlias = None | bool | int | float | str
JsonValue: TypeAlias = JsonScalar | list["JsonValue"] | dict[str, "JsonValue"]


@dataclass(frozen=True, slots=True)
class DriverResponse:
    request_id: str
    data: dict[str, JsonValue]

    @classmethod
    def parse(cls, value: JsonValue, expected_request_id: str) -> "DriverResponse":
        root = _require_mapping(value, "response")
        if root.get("version") != 1 or root.get("request_id") != expected_request_id:
            raise TermtraceProtocolError("driver response version or request ID does not match")
        result = _require_mapping(root.get("result"), "response.result")
        status = result.get("status")
        if status != "ok":
            raise TermtraceProtocolError("non-success response must be parsed as an operation error")
        return cls(expected_request_id, _require_mapping(result.get("data"), "response.result.data"))


def parse_error(value: JsonValue, expected_request_id: str) -> dict[str, JsonValue] | None:
    root = _require_mapping(value, "response")
    if root.get("version") != 1 or root.get("request_id") != expected_request_id:
        raise TermtraceProtocolError("driver response version or request ID does not match")
    result = _require_mapping(root.get("result"), "response.result")
    if result.get("status") == "ok":
        return None
    if result.get("status") != "error":
        raise TermtraceProtocolError("driver response status is invalid")
    return result


def _require_mapping(value: JsonValue, path: str) -> dict[str, JsonValue]:
    if not isinstance(value, dict):
        raise TermtraceProtocolError(f"{path} must be an object")
    return value
