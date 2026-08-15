"""Session-bound synchronous and asynchronous screen locators."""

from __future__ import annotations

import asyncio
import time
from collections.abc import Awaitable, Callable
from dataclasses import dataclass

from termtrace.duration import Duration, duration
from termtrace.errors import (
    InvalidQueryError,
    LocatorTimeoutError,
    MultipleMatchesError,
    NoMatchError,
    RequestTimeoutError,
)
from termtrace.screen.frame import ScreenFrame
from termtrace.screen.queries import LocatorSpec, ScreenMatch, describe, evaluate


@dataclass(frozen=True, slots=True)
class LocatorWaitResult:
    frame: ScreenFrame
    matches: tuple[ScreenMatch, ...]


class Locator:
    def __init__(self, capture: Callable[[float | None], ScreenFrame], spec: LocatorSpec) -> None:
        self._capture = capture
        self.spec = spec

    def filter(self, spec: LocatorSpec) -> Locator:
        return Locator(self._capture, self.spec & spec)

    def all(self) -> tuple[ScreenMatch, ...]:
        return evaluate(self._capture(None), self.spec)

    def count(self) -> int:
        return len(self.all())

    def single(self) -> ScreenMatch:
        frame = self._capture(None)
        matches = evaluate(frame, self.spec)
        if not matches:
            raise NoMatchError("query did not match the current frame", state_hash=frame.state_hash)
        if len(matches) != 1:
            raise MultipleMatchesError(
                len(matches),
                [match.text for match in matches],
                state_hash=frame.state_hash,
                ranges=[match.ranges for match in matches],
            )
        return matches[0]

    def first(self) -> ScreenMatch:
        frame = self._capture(None)
        matches = evaluate(frame, self.spec)
        if not matches:
            raise NoMatchError("query did not match the current frame", state_hash=frame.state_hash)
        return matches[0]

    def nth(self, index: int) -> ScreenMatch:
        if index < 0:
            raise InvalidQueryError("match index must be nonnegative")
        frame = self._capture(None)
        matches = evaluate(frame, self.spec)
        if index >= len(matches):
            raise NoMatchError(
                f"query has no match at zero-based index {index}",
                state_hash=frame.state_hash,
            )
        return matches[index]

    def wait(
        self,
        *,
        timeout: Duration | int | float = Duration.seconds(10),
        interval: Duration | int | float = Duration.milliseconds(50),
        stable_for: Duration | int | float = Duration(0),
    ) -> LocatorWaitResult:
        timeout_value = duration(timeout)
        interval_value = duration(interval)
        stable_value = duration(stable_for)
        if timeout_value.nanoseconds == 0:
            raise InvalidQueryError("wait timeout must be greater than zero")
        if interval_value.nanoseconds == 0:
            raise InvalidQueryError("wait interval must be greater than zero")
        if stable_value.nanoseconds > timeout_value.nanoseconds:
            raise InvalidQueryError("wait stable interval must not exceed timeout")
        deadline = time.monotonic_ns() + timeout_value.nanoseconds
        matching_state: tuple[str, int] | None = None
        last_frame: ScreenFrame | None = None
        while True:
            remaining = deadline - time.monotonic_ns()
            if remaining <= 0:
                raise LocatorTimeoutError(
                    "locator wait reached its deadline",
                    last_frame=last_frame,
                    query=describe(self.spec),
                )
            try:
                last_frame = self._capture(remaining / 1_000_000_000)
            except RequestTimeoutError:
                raise LocatorTimeoutError(
                    "locator wait reached its deadline",
                    last_frame=last_frame,
                    query=describe(self.spec),
                ) from None
            observed_at = time.monotonic_ns()
            if observed_at >= deadline:
                raise LocatorTimeoutError(
                    "locator wait reached its deadline",
                    last_frame=last_frame,
                    query=describe(self.spec),
                )
            matches = evaluate(last_frame, self.spec)
            if matches:
                if stable_value.nanoseconds == 0:
                    return LocatorWaitResult(last_frame, matches)
                if matching_state is None or matching_state[0] != last_frame.state_hash:
                    matching_state = (last_frame.state_hash, observed_at)
                elif observed_at - matching_state[1] >= stable_value.nanoseconds:
                    return LocatorWaitResult(last_frame, matches)
            else:
                matching_state = None
            remaining = deadline - observed_at
            if remaining <= 0:
                raise LocatorTimeoutError(
                    "locator wait reached its deadline",
                    last_frame=last_frame,
                    query=describe(self.spec),
                )
            time.sleep(min(interval_value.nanoseconds, remaining) / 1_000_000_000)


class AsyncLocator:
    def __init__(
        self,
        capture: Callable[[float | None], Awaitable[ScreenFrame]],
        spec: LocatorSpec,
    ) -> None:
        self._capture = capture
        self.spec = spec

    def filter(self, spec: LocatorSpec) -> AsyncLocator:
        return AsyncLocator(self._capture, self.spec & spec)

    async def all(self) -> tuple[ScreenMatch, ...]:
        return evaluate(await self._capture(None), self.spec)

    async def count(self) -> int:
        return len(await self.all())

    async def single(self) -> ScreenMatch:
        frame = await self._capture(None)
        matches = evaluate(frame, self.spec)
        if not matches:
            raise NoMatchError("query did not match the current frame", state_hash=frame.state_hash)
        if len(matches) != 1:
            raise MultipleMatchesError(
                len(matches),
                [match.text for match in matches],
                state_hash=frame.state_hash,
                ranges=[match.ranges for match in matches],
            )
        return matches[0]

    async def first(self) -> ScreenMatch:
        frame = await self._capture(None)
        matches = evaluate(frame, self.spec)
        if not matches:
            raise NoMatchError("query did not match the current frame", state_hash=frame.state_hash)
        return matches[0]

    async def nth(self, index: int) -> ScreenMatch:
        if index < 0:
            raise InvalidQueryError("match index must be nonnegative")
        frame = await self._capture(None)
        matches = evaluate(frame, self.spec)
        if index >= len(matches):
            raise NoMatchError(
                f"query has no match at zero-based index {index}",
                state_hash=frame.state_hash,
            )
        return matches[index]

    async def wait(
        self,
        *,
        timeout: Duration | int | float = Duration.seconds(10),
        interval: Duration | int | float = Duration.milliseconds(50),
        stable_for: Duration | int | float = Duration(0),
    ) -> LocatorWaitResult:
        timeout_value = duration(timeout)
        interval_value = duration(interval)
        stable_value = duration(stable_for)
        if timeout_value.nanoseconds == 0:
            raise InvalidQueryError("wait timeout must be greater than zero")
        if interval_value.nanoseconds == 0:
            raise InvalidQueryError("wait interval must be greater than zero")
        if stable_value.nanoseconds > timeout_value.nanoseconds:
            raise InvalidQueryError("wait stable interval must not exceed timeout")
        deadline = time.monotonic_ns() + timeout_value.nanoseconds
        matching_state: tuple[str, int] | None = None
        last_frame: ScreenFrame | None = None
        while True:
            remaining = deadline - time.monotonic_ns()
            if remaining <= 0:
                raise LocatorTimeoutError(
                    "locator wait reached its deadline",
                    last_frame=last_frame,
                    query=describe(self.spec),
                )
            try:
                async with asyncio.timeout(remaining / 1_000_000_000):
                    captured_frame = await self._capture(remaining / 1_000_000_000)
            except (TimeoutError, RequestTimeoutError):
                raise LocatorTimeoutError(
                    "locator wait reached its deadline",
                    last_frame=last_frame,
                    query=describe(self.spec),
                ) from None
            last_frame = captured_frame
            observed_at = time.monotonic_ns()
            if observed_at >= deadline:
                raise LocatorTimeoutError(
                    "locator wait reached its deadline",
                    last_frame=last_frame,
                    query=describe(self.spec),
                )
            matches = evaluate(last_frame, self.spec)
            if matches:
                if stable_value.nanoseconds == 0:
                    return LocatorWaitResult(last_frame, matches)
                if matching_state is None or matching_state[0] != last_frame.state_hash:
                    matching_state = (last_frame.state_hash, observed_at)
                elif observed_at - matching_state[1] >= stable_value.nanoseconds:
                    return LocatorWaitResult(last_frame, matches)
            else:
                matching_state = None
            remaining = deadline - observed_at
            if remaining <= 0:
                raise LocatorTimeoutError(
                    "locator wait reached its deadline",
                    last_frame=last_frame,
                    query=describe(self.spec),
                )
            await asyncio.sleep(min(interval_value.nanoseconds, remaining) / 1_000_000_000)


__all__ = ["AsyncLocator", "Locator", "LocatorWaitResult"]
