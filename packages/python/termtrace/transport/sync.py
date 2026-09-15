"""Synchronous driver process ownership and correlated transport."""

import math
import os
import select
import subprocess
import threading
import time
from collections.abc import Sequence
from pathlib import Path
from types import TracebackType
from typing import IO, TYPE_CHECKING, Literal, Self, TypeVar

import msgspec

from termtrace.environment import EnvironmentOptions
from termtrace.errors import ProtocolMismatchError, RequestTimeoutError
from termtrace.transport.binary import resolve_binary
from termtrace.transport.protocol import (
    DriverAttachedData,
    DriverHelloData,
    JsonObject,
    decode_driver_response,
)
from termtrace.transport.requests import attach_request, correlated_request, launch_request

if TYPE_CHECKING:
    from termtrace.session.sync import Session


ResponseDataT = TypeVar("ResponseDataT", bound=msgspec.Struct)
DRIVER_RESPONSE_STREAM_LIMIT = 4 * 1024 * 1024 + 1


class DriverTransport:
    """Own one driver process and correlated request stream."""

    _process: subprocess.Popen[bytes]
    _stdin: IO[bytes]
    _stdout: IO[bytes]

    def __init__(self, binary_path: str | Path) -> None:
        self._process = subprocess.Popen(
            [str(binary_path), "agent"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
        )
        if self._process.stdin is None or self._process.stdout is None:
            raise ProtocolMismatchError("driver pipes were not created")
        self._stdin = self._process.stdin
        self._stdout = self._process.stdout
        os.set_blocking(self._stdin.fileno(), False)
        os.set_blocking(self._stdout.fileno(), False)
        self._lock = threading.Lock()
        self._read_buffer = bytearray()
        self._closed = False

    def _write_all(self, data: bytes, timeout: float | None, deadline: float | None) -> None:
        total_written = 0
        fd = self._stdin.fileno()
        view = memoryview(data)
        while total_written < len(data):
            if deadline is not None:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    self._abort()
                    assert timeout is not None
                    raise RequestTimeoutError(timeout)
                _, writable, _ = select.select([], [self._stdin], [], remaining)
                if not writable:
                    self._abort()
                    assert timeout is not None
                    raise RequestTimeoutError(timeout)
            else:
                select.select([], [self._stdin], [])
            try:
                written = os.write(fd, view[total_written:])
            except BlockingIOError:
                continue
            except (BrokenPipeError, OSError) as error:
                self._abort()
                raise ProtocolMismatchError(f"driver closed stdin: {error}") from None
            if written == 0:
                self._abort()
                raise ProtocolMismatchError("driver closed stdin during write")
            total_written += written

    def request(
        self,
        method: JsonObject,
        response_type: type[ResponseDataT],
        request_id: str | None = None,
        timeout: float | None = None,
    ) -> ResponseDataT:
        if self._closed:
            raise ProtocolMismatchError("driver is closed")
        correlation_id, payload = correlated_request(method, request_id, timeout)
        if timeout is None:
            deadline = None
            acquired = self._lock.acquire()
        else:
            deadline = time.monotonic() + timeout
            remaining_lock = max(0.0, deadline - time.monotonic())
            acquired = self._lock.acquire(timeout=remaining_lock)
        if not acquired:
            assert timeout is not None
            raise RequestTimeoutError(timeout)
        try:
            self._write_all(payload + b"\n", timeout, deadline)
            remaining = None if deadline is None else deadline - time.monotonic()
            if remaining is not None and remaining <= 0:
                self._abort()
                assert timeout is not None
                raise RequestTimeoutError(timeout)
            line = self._readline(remaining)
        except RequestTimeoutError:
            assert timeout is not None
            raise RequestTimeoutError(timeout) from None
        finally:
            self._lock.release()
        if not line:
            raise ProtocolMismatchError("driver closed before its correlated response")
        return decode_driver_response(line, correlation_id, response_type)

    def _readline(self, timeout: float | None) -> bytes:
        if timeout is not None and (not math.isfinite(timeout) or timeout <= 0):
            raise ValueError("request timeout must be finite and greater than zero")

        deadline = None if timeout is None else time.monotonic() + timeout
        search_start = 0
        while True:
            newline = self._read_buffer.find(b"\n", search_start)
            if newline >= 0:
                line = bytes(self._read_buffer[: newline + 1])
                del self._read_buffer[: newline + 1]
                return line

            search_start = max(0, len(self._read_buffer) - 1)

            if deadline is not None:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    self._abort()
                    assert timeout is not None
                    raise RequestTimeoutError(timeout)
                ready, _, _ = select.select([self._stdout], [], [], remaining)
                if not ready:
                    self._abort()
                    assert timeout is not None
                    raise RequestTimeoutError(timeout)
            else:
                select.select([self._stdout], [], [])

            try:
                chunk = os.read(self._stdout.fileno(), 64 * 1024)
            except BlockingIOError:
                continue
            if not chunk:
                return b""
            self._read_buffer.extend(chunk)
            if len(self._read_buffer) > DRIVER_RESPONSE_STREAM_LIMIT:
                self._abort()
                raise ProtocolMismatchError("driver response exceeded the bounded stream limit")

    def _abort(self) -> None:
        if self._closed:
            return
        self._closed = True
        try:
            self._stdin.close()
        except Exception:
            pass
        if self._process.poll() is None:
            try:
                self._process.terminate()
                self._process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                try:
                    self._process.kill()
                    self._process.wait(timeout=1)
                except (subprocess.TimeoutExpired, ProcessLookupError):
                    pass
            except ProcessLookupError:
                pass
        try:
            self._stdout.close()
        except Exception:
            pass

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        try:
            self._stdin.close()
        except Exception:
            pass
        if self._process.poll() is None:
            try:
                self._process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                try:
                    self._process.terminate()
                    self._process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    try:
                        self._process.kill()
                        self._process.wait(timeout=2)
                    except (subprocess.TimeoutExpired, ProcessLookupError):
                        pass
                except ProcessLookupError:
                    pass
        try:
            self._stdout.close()
        except Exception:
            pass

    def __del__(self) -> None:
        try:
            self.close()
        except Exception:
            pass

    @property
    def closed(self) -> bool:
        return self._closed


class Terminal:
    """Context-managed synchronous client over one driver process."""

    def __init__(
        self,
        *,
        binary_path: str | Path | None = None,
        evidence_root: str | Path | None = None,
    ) -> None:
        self._transport = DriverTransport(resolve_binary(binary_path))
        self._evidence_root = None if evidence_root is None else Path(evidence_root)
        try:
            self._transport.request({"method": "hello"}, DriverHelloData)
        except BaseException:
            self._transport.close()
            raise

    def launch(
        self,
        command: Sequence[str | Path],
        *,
        evidence_root: str | Path | None = None,
        cols: int = 160,
        rows: int = 45,
        working_directory: str | Path | None = None,
        evidence: Literal["always", "on-failure", "manual"] = "always",
        environment: EnvironmentOptions | None = None,
        output_byte_budget: int | None = None,
    ) -> "Session":

        from termtrace.session.sync import Session

        data = self._transport.request(
            launch_request(
                command,
                default_evidence_root=self._evidence_root,
                evidence_root=evidence_root,
                owner="Terminal",
                cols=cols,
                rows=rows,
                working_directory=working_directory,
                environment=environment,
                output_byte_budget=output_byte_budget,
            ),
            DriverAttachedData,
        )
        return Session(
            self._transport,
            data.handle,
            data.session_id,
            data.canonical_session,
            owned=True,
            evidence_policy=evidence,
            completed=data.completed,
        )

    def attach(self, session: str | Path) -> "Session":
        from termtrace.session.sync import Session

        data = self._transport.request(attach_request(session), DriverAttachedData)
        return Session(
            self._transport,
            data.handle,
            data.session_id,
            data.canonical_session,
            owned=False,
            evidence_policy="manual",
            completed=data.completed,
        )

    def close(self) -> None:
        self._transport.close()

    @property
    def closed(self) -> bool:
        return self._transport.closed

    def __enter__(self) -> Self:
        return self

    def __exit__(
        self,
        error_type: type[BaseException] | None,
        error: BaseException | None,
        traceback: TracebackType | None,
    ) -> None:
        self.close()
