import asyncio
import math
import sys

import pytest

from termtrace.async_client import AsyncSession, AsyncTerminal
from termtrace.client import DriverTransport, Session


@pytest.mark.parametrize(
    "duration", [-1.0, math.nan, math.inf, 1e30, sys.float_info.max]
)
def test_screen_waits_reject_invalid_durations_before_driver_io(
    duration: float,
) -> None:
    session = Session(
        object.__new__(DriverTransport),
        "00000000-0000-0000-0000-000000000001",
        "00000000-0000-0000-0000-000000000002",
        "/unused",
        owned=False,
        evidence_policy="manual",
        completed=False,
    )
    async_session = AsyncSession(
        object.__new__(AsyncTerminal),
        "00000000-0000-0000-0000-000000000001",
        "00000000-0000-0000-0000-000000000002",
        "/unused",
        owned=False,
        evidence_policy="manual",
        completed=False,
    )

    with pytest.raises(ValueError, match="duration"):
        session.screen.wait_for_text("ready", timeout=duration)
    with pytest.raises(ValueError, match="duration"):
        asyncio.run(async_session.screen.wait_for_text("ready", timeout=duration))
