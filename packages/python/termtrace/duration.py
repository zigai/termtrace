"""Exact duration values used by every public operation."""

from __future__ import annotations

import math
from dataclasses import dataclass
from decimal import Decimal, InvalidOperation

from termtrace.errors import InvalidDurationError

MAX_U64 = (1 << 64) - 1
NANOSECONDS_PER_SECOND = 1_000_000_000
NANOSECONDS_PER_MILLISECOND = 1_000_000


@dataclass(frozen=True, slots=True, order=True)
class Duration:
    """An exact, nonnegative unsigned 64-bit nanosecond duration."""

    nanoseconds: int

    def __post_init__(self) -> None:
        if isinstance(self.nanoseconds, bool) or not isinstance(self.nanoseconds, int):
            raise InvalidDurationError("duration nanoseconds must be an integer")
        if not 0 <= self.nanoseconds <= MAX_U64:
            raise InvalidDurationError("duration nanoseconds must fit u64")

    @classmethod
    def nanoseconds_exact(cls, value: int) -> Duration:
        return cls(value)

    @classmethod
    def milliseconds(cls, value: int | float | Decimal | str) -> Duration:
        return cls(_exact_units(value, NANOSECONDS_PER_MILLISECOND))

    @classmethod
    def seconds(cls, value: int | float | Decimal | str) -> Duration:
        return cls(_exact_units(value, NANOSECONDS_PER_SECOND))


def duration(value: Duration | int | float | Decimal | str) -> Duration:
    """Normalize a public seconds value while preserving an existing Duration."""
    if isinstance(value, Duration):
        return value
    return Duration.seconds(value)


def seconds_to_nanoseconds(seconds: Duration | int | float | Decimal | str) -> int:
    return duration(seconds).nanoseconds


def seconds_to_milliseconds(seconds: Duration | int | float | Decimal | str) -> int:
    value = duration(seconds).nanoseconds
    quotient, remainder = divmod(value, NANOSECONDS_PER_MILLISECOND)
    if remainder:
        raise InvalidDurationError("duration is not an exact whole millisecond")
    return quotient


def _exact_units(value: int | float | Decimal | str, units: int) -> int:
    if isinstance(value, bool):
        raise InvalidDurationError("duration must be a finite nonnegative number")
    if isinstance(value, float) and not math.isfinite(value):
        raise InvalidDurationError("duration must be finite and nonnegative")
    try:
        decimal = Decimal(str(value))
    except (InvalidOperation, ValueError) as e:
        raise InvalidDurationError("duration must be a finite nonnegative number") from e
    if not decimal.is_finite() or decimal < 0:
        raise InvalidDurationError("duration must be finite and nonnegative")
    converted = decimal * units
    integral = converted.to_integral_value()
    if converted != integral:
        raise InvalidDurationError("duration must resolve to an exact nanosecond")
    result = int(integral)
    if result > MAX_U64:
        raise InvalidDurationError("duration exceeds the protocol maximum")
    return result


__all__ = [
    "MAX_U64",
    "Duration",
    "duration",
    "seconds_to_milliseconds",
    "seconds_to_nanoseconds",
]
