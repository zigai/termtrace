"""Bounded conversion for durations sent through the driver protocol."""

import math


_MAX_U64 = (1 << 64) - 1


def seconds_to_nanoseconds(seconds: float) -> int:
    """Convert finite nonnegative seconds into a protocol `u64`."""

    return _seconds_to_units(seconds, 1_000_000_000)


def seconds_to_milliseconds(seconds: float) -> int:
    """Convert finite nonnegative seconds into a protocol `u64`."""

    return _seconds_to_units(seconds, 1000)


def _seconds_to_units(seconds: float, units_per_second: int) -> int:
    if not math.isfinite(seconds) or seconds < 0:
        raise ValueError("duration must be finite and nonnegative")
    if seconds > _MAX_U64 / units_per_second:
        raise ValueError("duration exceeds the protocol maximum")
    converted = int(seconds * units_per_second)
    if converted > _MAX_U64:
        raise ValueError("duration exceeds the protocol maximum")
    return converted
