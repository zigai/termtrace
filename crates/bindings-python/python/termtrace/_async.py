"""Runtime shared by the generated asynchronous wrappers in `termtrace.aio`."""

from __future__ import annotations

import asyncio
import logging
from collections.abc import Callable
from typing import TYPE_CHECKING, Any, TypeVar, cast

from termtrace._termtrace import CancelToken, LocatorWaitResult, TermtraceError
from termtrace._termtrace import Recording as _SyncRecording

if TYPE_CHECKING:
    from typing_extensions import Self

    from termtrace import types
    from termtrace.aio import AsyncRecording

_logger = logging.getLogger("termtrace.aio")
_pending_launches: set[asyncio.Task[Any]] = set()
_pending_cleanups: set[asyncio.Future[list[tuple[str, BaseException]]]] = set()
_T = TypeVar("_T")


def _retrieve(worker: asyncio.Future[Any]) -> None:
    if not worker.cancelled():
        worker.exception()


async def offload_mutation(
    pending: set[asyncio.Future[Any]], call: Callable[[], _T]
) -> _T:
    """Run one mutation off the loop and wait for it.

    Cancelling the await only stops the caller from waiting: the admitted mutation keeps running
    and is tracked in ``pending`` until it returns, so teardown can wait for it.
    """

    def invoke() -> tuple[_T | None, BaseException | None]:
        try:
            return call(), None
        except BaseException as error:  # noqa: BLE001 - re-raised unchanged unless caller cancelled
            return None, error

    # A shielded Future itself settles successfully with an outcome. Python 3.14 otherwise logs
    # a late inner exception even after retrieval when the caller cancelled the outer shield.
    worker = asyncio.get_running_loop().run_in_executor(None, invoke)
    pending.add(worker)
    worker.add_done_callback(pending.discard)
    worker.add_done_callback(_retrieve)
    value, error = await asyncio.shield(worker)
    if error is not None:
        raise error
    return cast(_T, value)


async def run_cancellable(
    call: Callable[[CancelToken], _T], cancel: CancelToken | None
) -> _T:
    """Run a blocking wait off the loop; on cancellation, cancel its token and await its return.

    `CancelledError` therefore surfaces only after the worker thread has finished, so no capture
    is left outstanding.
    """
    token = cancel if cancel is not None else CancelToken()
    worker = asyncio.get_running_loop().run_in_executor(None, call, token)
    try:
        return await asyncio.shield(worker)
    except asyncio.CancelledError:
        token.cancel()
        while not worker.done():
            try:
                await asyncio.shield(worker)
            except asyncio.CancelledError:
                continue
            except Exception:  # noqa: BLE001 - the worker's own outcome is superseded by cancellation
                break
        raise


def _cleanup_launch_blocking(
    launch: asyncio.Future[_SyncRecording],
) -> list[tuple[str, BaseException]]:
    # The worker owns process I/O. Return diagnostics; logging happens atomically back on the
    # loop only after fallback cleanup has finished, never announcing an unfinished cleanup.
    if launch.cancelled():
        return []
    failures: list[tuple[str, BaseException]] = []
    try:
        recording = launch.result()
    except Exception as error:  # noqa: BLE001 - launch failure remains secondary to cancellation
        return [("Failed to clean up cancelled recording launch", error)]
    try:
        recording.force_close()
    except TermtraceError as error:
        failures.append(
            ("Failed to force-close cancelled recording; trying stop", error)
        )
        try:
            recording.stop()
        except Exception as error:  # noqa: BLE001 - report fallback failure without replacing cancellation
            failures.append(("Failed to clean up cancelled recording launch", error))
    except Exception as error:  # noqa: BLE001 - cleanup must not replace cancellation
        failures.append(("Failed to clean up cancelled recording launch", error))
    return failures


def _cleanup_launch(launch: asyncio.Future[_SyncRecording]) -> None:
    # Completion callbacks run on the loop; process teardown must not run there.
    worker = asyncio.get_running_loop().run_in_executor(
        None, _cleanup_launch_blocking, launch
    )
    _pending_cleanups.add(worker)

    def finished(done: asyncio.Future[list[tuple[str, BaseException]]]) -> None:
        _pending_cleanups.discard(done)
        if not done.cancelled():
            for message, error in done.result():
                _logger.error(message, exc_info=error)

    worker.add_done_callback(finished)


async def launch(call: Callable[[], _SyncRecording]) -> _SyncRecording:
    """Start a recording off the loop; a cancelled await force-closes the late recording."""
    task = asyncio.create_task(asyncio.to_thread(call))
    _pending_launches.add(task)
    task.add_done_callback(_pending_launches.discard)
    try:
        return await asyncio.shield(task)
    except asyncio.CancelledError:
        if task.done():
            _cleanup_launch(task)
        else:
            task.add_done_callback(_cleanup_launch)
        raise


class Wrapper:
    """Common state of an asynchronous wrapper: the native object and its pending mutations."""

    _sync: Any
    _pending: set[asyncio.Future[Any]]

    @classmethod
    def _wrap(cls, sync: Any, pending: set[asyncio.Future[Any]] | None = None) -> Self:
        wrapper = cls.__new__(cls)
        wrapper._sync = sync
        wrapper._pending = pending if pending is not None else set()
        return wrapper

    async def wait_pending(self) -> None:
        """Wait until every mutation whose await was cancelled or abandoned has returned."""
        while self._pending:
            await asyncio.wait(set(self._pending))


class LocatorBase(Wrapper):
    """Locator waits that delegate to the cancellable `wait_for`."""

    if TYPE_CHECKING:

        async def wait_for(
            self,
            condition: types.LocatorCondition | None = None,
            timeout_secs: float | None = None,
            interval_secs: float | None = None,
            stable_for_secs: float | None = None,
            cancel: CancelToken | None = None,
        ) -> LocatorWaitResult: ...

    async def wait(
        self, timeout_secs: float | None = None, interval_secs: float | None = None
    ) -> LocatorWaitResult:
        """Wait until at least one match exists; cancellable like `wait_for`."""
        return await self.wait_for(
            "exists", timeout_secs=timeout_secs, interval_secs=interval_secs
        )

    async def wait_stable(
        self,
        timeout_secs: float | None = None,
        interval_secs: float | None = None,
        stable_for_secs: float | None = None,
    ) -> LocatorWaitResult:
        """Wait until a match holds in one unchanged state; cancellable like `wait_for`."""
        return await self.wait_for(
            "exists",
            timeout_secs=timeout_secs,
            interval_secs=interval_secs,
            stable_for_secs=stable_for_secs,
        )


class RecordingBase(Wrapper):
    """Recording teardown that first waits for abandoned mutations."""

    async def stop(self) -> None:
        """Wait for abandoned mutations, then stop and finalize the recording."""
        await self.wait_pending()
        await offload_mutation(self._pending, self._sync.stop)

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: object,
    ) -> None:
        from pathlib import Path

        from termtrace.testing import AsyncTerminalTestContext, TerminalTestContext

        context = TerminalTestContext(
            _owned_root=str(Path(self._sync.path).parent), evidence_policy="manual"
        )
        context._recordings.append(self._sync)
        adapter = AsyncTerminalTestContext(context)
        adapter._recordings.append(cast("AsyncRecording", self))
        await adapter.__aexit__(exc_type, exc_val, exc_tb)
