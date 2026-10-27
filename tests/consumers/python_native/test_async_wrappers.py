"""Async SDK operations must delegate blocking work off the event-loop thread."""

from __future__ import annotations

import asyncio
import threading
import time
from typing import Any
from unittest.mock import Mock

import pytest
from termtrace import CancelToken, aio

RECORDING_CALLS = [
    ("pointer_click", (10, 5, "left", True)),
    ("pointer_scroll", (10, 5, 2, True)),
    ("pointer_focus", (True, True)),
    ("save", ("capture", ["png"], "state", False)),
    ("read_output", (0, 128)),
    ("wait_for_output", ("ready", 1.0)),
    ("read_events", (0, 10)),
    ("explain", ("state", 2)),
    ("diagnose", (20,)),
    ("last_action", (20,)),
]
CLIENT_CALLS = [
    ("verify_bundle", ("bundle", "source")),
    ("verify_session", ("session", "content")),
    ("recover_session", ("session", "destination")),
    ("registry_list", (True,)),
    ("registry_resolve", ("app",)),
    ("registry_register", ("app", "session")),
    ("registry_remove", ("app",)),
    ("registry_gc", (1, 2, 4096, False)),
    ("tmux_bundle", ("manifest", "bundle")),
    ("tmux_verify_bundle", ("bundle", "manifest")),
    ("probe_capabilities", ()),
]
CALLS = [
    (owner, method, args)
    for owner, calls in [("recording", RECORDING_CALLS), ("client", CLIENT_CALLS)]
    for method, args in calls
]


def wrapper_for(owner: str, sync: Any) -> Any:
    if owner == "recording":
        return aio.AsyncRecording._wrap(sync)
    return aio.AsyncTermtrace._wrap(sync)


@pytest.mark.asyncio
@pytest.mark.parametrize(("owner", "method", "args"), CALLS)
async def test_async_operation_leaves_event_loop_responsive(
    owner: str, method: str, args: tuple[Any, ...]
) -> None:
    loop = asyncio.get_running_loop()
    event_loop_thread = threading.get_ident()
    started = asyncio.Event()
    release = threading.Event()
    result = object()
    sync = Mock()

    def blocking(*received: Any) -> object:
        assert threading.get_ident() != event_loop_thread
        assert received == args
        loop.call_soon_threadsafe(started.set)
        assert release.wait(timeout=5.0), "test did not release worker thread"
        return result

    getattr(sync, method).side_effect = blocking
    wrapper = wrapper_for(owner, sync)
    task = asyncio.create_task(getattr(wrapper, method)(*args))
    try:
        await asyncio.wait_for(started.wait(), timeout=2.0)
        assert not task.done()
        heartbeat = asyncio.Event()
        loop.call_soon(heartbeat.set)
        await asyncio.wait_for(heartbeat.wait(), timeout=2.0)
    finally:
        release.set()
        # Reap the task even if a responsiveness assertion fails.
        returned = await task

    assert returned is result
    getattr(sync, method).assert_called_once_with(*args)


@pytest.mark.asyncio
@pytest.mark.parametrize(("owner", "method", "args"), CALLS)
async def test_async_operation_preserves_underlying_exception(
    owner: str, method: str, args: tuple[Any, ...]
) -> None:
    failure = RuntimeError("operation failed")
    sync = Mock()
    getattr(sync, method).side_effect = failure
    wrapper = wrapper_for(owner, sync)
    with pytest.raises(RuntimeError) as raised:
        await getattr(wrapper, method)(*args)
    assert raised.value is failure
    getattr(sync, method).assert_called_once_with(*args)


class SlowCancellableLocator:
    """Returns only some time after its token is cancelled, like a capture already in flight."""

    def __init__(self) -> None:
        self.returned = threading.Event()
        self.thread: int | None = None

    def wait_for(self, *args: Any) -> object:
        token = args[-1]
        self.thread = threading.get_ident()
        try:
            while not token.cancelled:
                time.sleep(0.01)
            time.sleep(0.2)
            raise RuntimeError("cancelled")
        finally:
            self.returned.set()


@pytest.mark.asyncio
async def test_async_locator_cancellation_surfaces_after_worker_returns() -> None:
    sync = SlowCancellableLocator()
    locator = aio.AsyncLocator._wrap(sync)
    task = asyncio.create_task(locator.wait_for("exists", timeout_secs=30))
    await asyncio.sleep(0.05)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert sync.returned.is_set()
    assert sync.thread != threading.get_ident()


@pytest.mark.asyncio
async def test_async_locator_uses_caller_token() -> None:
    sync = SlowCancellableLocator()
    token = CancelToken()
    task = asyncio.create_task(aio.AsyncLocator._wrap(sync).wait_for(cancel=token))
    await asyncio.sleep(0.05)
    token.cancel()
    with pytest.raises(RuntimeError, match="cancelled"):
        await task
    assert sync.returned.is_set()


@pytest.mark.asyncio
@pytest.mark.parametrize("operation,args", [
    ("run", ()), ("execute", ()), ("pointer_click", (0, 0)),
    ("pointer_scroll", (0, 0)), ("pointer_focus", (True,)),
    ("stop", ()), ("wait_for_completion", ()),
])
async def test_cancelled_mutations_remain_joinable(operation: str, args: tuple[Any, ...]) -> None:
    """Builder and legacy offloads share the recording's pending mutation ownership."""
    started = threading.Event()
    release = threading.Event()
    returned = threading.Event()
    def slow(*args: Any) -> None:
        started.set()
        release.wait(2)
        returned.set()
    sync = Mock()
    getattr(sync.actions.return_value, operation).side_effect = slow
    getattr(sync, operation).side_effect = slow
    wrapper = aio.AsyncRecording._wrap(sync)
    owner = wrapper.actions() if operation in ("run", "execute") else wrapper
    task = asyncio.create_task(getattr(owner, operation)(*args))
    while not started.is_set():
        await asyncio.sleep(.001)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert wrapper._pending
    release.set()
    await asyncio.wait_for(wrapper.wait_pending(), 1)
    assert returned.is_set()
    assert not wrapper._pending


@pytest.mark.asyncio
async def test_cancelled_off_thread_work_can_continue() -> None:
    started = threading.Event()
    release = threading.Event()
    finished = threading.Event()

    class Screen:
        def text(self) -> str:
            started.set()
            release.wait(3)
            finished.set()
            return "already applied"

    task = asyncio.create_task(aio.AsyncScreen._wrap(Screen()).text())
    try:
        assert await asyncio.to_thread(started.wait, 2)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert not finished.is_set()
    finally:
        release.set()
    assert await asyncio.to_thread(finished.wait, 2)
