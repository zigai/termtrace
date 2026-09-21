"""Asynchronous Termtrace client for asyncio-based agent loops and workflows."""

from __future__ import annotations

import asyncio
from collections.abc import Mapping, Sequence
from typing import Any, Self

from termtrace._termtrace import (
    Recording as _SyncRecording,
)
from termtrace._termtrace import (
    ResolvedState,
    ScreenFrame,
    execute_action_async,
    stop_recording_async,
    wait_for_idle_async,
    wait_for_text_async,
)
from termtrace._termtrace import (
    Termtrace as _SyncTermtrace,
)

def _cleanup_recording(rec: Any) -> None:
    try:
        rec.force_close()
    except Exception:
        try:
            rec.stop()
        except Exception:
            pass


class AsyncScreen:
    """Asynchronous screen inspection handle."""

    def __init__(self, screen_sync: Any) -> None:
        self._screen = screen_sync

    async def frame(self) -> ScreenFrame:
        """Capture and return the current screen frame."""
        return await asyncio.to_thread(self._screen.frame)

    async def text(self) -> str:
        """Return the current screen plain text."""
        return await asyncio.to_thread(self._screen.text)

    async def wait_for_text(self, pattern: str, timeout: float | None = None) -> ResolvedState:
        """Asynchronously wait until the screen text contains pattern."""
        return await wait_for_text_async(self._screen, pattern, timeout)

    async def wait_for_idle(self, timeout: float | None = None) -> ResolvedState:
        """Asynchronously wait until the screen remains idle and unchanged."""
        return await wait_for_idle_async(self._screen, timeout)


class AsyncRecording:
    """Asynchronous recording session with async context manager support."""

    def __init__(self, recording_sync: _SyncRecording) -> None:
        self._recording = recording_sync

    @property
    def session_id(self) -> str:
        """Canonical session ID."""
        return self._recording.session_id

    @property
    def path(self) -> str:
        """Filesystem path to session evidence."""
        return self._recording.path

    def is_active(self) -> bool:
        """Check whether the recording is still running."""
        return self._recording.is_active()

    def screen(self) -> AsyncScreen:
        """Access async screen capabilities."""
        return AsyncScreen(self._recording.screen())

    def actions(self) -> Any:
        """Access action builder."""
        return self._recording.actions()

    async def execute_action(self, actions_builder: Any) -> ResolvedState | None:
        """Asynchronously execute an action sequence."""
        return await execute_action_async(actions_builder)

    def pointer_click(
        self, column: int, row: int, button: str | None = None, strict: bool | None = None
    ) -> None:
        """Click mouse button at a terminal cell."""
        self._recording.pointer_click(column, row, button, strict)

    def pointer_scroll(
        self, column: int, row: int, lines: int | None = None, strict: bool | None = None
    ) -> None:
        """Scroll mouse wheel at a terminal cell."""
        self._recording.pointer_scroll(column, row, lines, strict)

    def pointer_focus(self, focused: bool, strict: bool | None = None) -> None:
        """Report terminal focus state."""
        self._recording.pointer_focus(focused, strict)

    def save(
        self,
        destination: str,
        formats: Sequence[str] | None = None,
        state_hash: str | None = None,
        include_hidden: bool | None = None,
    ) -> str:
        """Atomically publish visual representations of a state point."""
        return self._recording.save(destination, formats, state_hash, include_hidden)

    def read_output(
        self, offset: int | None = None, limit: int | None = None
    ) -> tuple[bytes, int, bool]:
        """Read raw child PTY output bytes using an optional byte offset cursor."""
        return self._recording.read_output(offset, limit)

    def wait_for_output(self, pattern: str, timeout_secs: float | None = None) -> bool:
        """Wait for exact child output bytes to appear in the canonical stream."""
        return self._recording.wait_for_output(pattern, timeout_secs)

    def read_events(self, cursor: int | None = None, limit: int | None = None) -> str:
        """Read canonical event records as pretty JSON."""
        return self._recording.read_events(cursor, limit)

    def explain(self, target_hash: str | None = None, context: int | None = None) -> str:
        """Explain VT and semantic evidence around a target state point."""
        return self._recording.explain(target_hash, context)

    def diagnose(self, broad_change_percent: int | None = None) -> str:
        """Run conservative evidence-backed rendering diagnostics on the session."""
        return self._recording.diagnose(broad_change_percent)

    def last_action(self, broad_change_percent: int | None = None) -> str:
        """Perform full triage analysis on the most recent canonical action."""
        return self._recording.last_action(broad_change_percent)

    async def stop(self) -> None:
        """Asynchronously stop and finalize the recording."""
        await stop_recording_async(self._recording)

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        exc_type: type[BaseException] | None,
        exc_val: BaseException | None,
        exc_tb: object,
    ) -> None:
        await self.stop()


class AsyncTermtrace:
    """Asynchronous entry point for Termtrace operations."""

    def __init__(
        self,
        binary: str | None = None,
        evidence_root: str | None = None,
    ) -> None:
        self._sync = _SyncTermtrace(binary, evidence_root)

    async def record(
        self,
        command: Sequence[str],
        *,
        rows: int | None = None,
        cols: int | None = None,
        env: Mapping[str, str] | None = None,
        cwd: str | None = None,
    ) -> AsyncRecording:
        """Launch a recorded session asynchronously."""
        env_dict = dict(env) if env is not None else None
        cmd_list = list(command)

        def _launch() -> _SyncRecording:
            return self._sync.record(cmd_list, rows, cols, env_dict, cwd)

        launch_task = asyncio.ensure_future(asyncio.to_thread(_launch))
        try:
            rec = await asyncio.shield(launch_task)
            return AsyncRecording(rec)
        except asyncio.CancelledError:
            if launch_task.done() and not launch_task.cancelled():
                try:
                    rec = launch_task.result()
                    _cleanup_recording(rec)
                except Exception:
                    pass
            else:
                def _on_done(fut: asyncio.Future[Any]) -> None:
                    try:
                        if not fut.cancelled():
                            rec = fut.result()
                            _cleanup_recording(rec)
                    except Exception:
                        pass
                launch_task.add_done_callback(_on_done)
            raise

    def verify_bundle(self, bundle_path: str, source_session: str | None = None) -> bool:
        """Verify an evidence bundle (.tar.zst or directory) offline."""
        return self._sync.verify_bundle(bundle_path, source_session)

    def verify_session(self, session_path: str, level: str | None = None) -> str:
        """Verify a completed session's structure and evidence bindings offline."""
        return self._sync.verify_session(session_path, level)

    def recover_session(self, session_path: str, destination: str) -> str:
        """Recover the longest integrity-checked event prefix from a damaged session."""
        return self._sync.recover_session(session_path, destination)

    def registry_list(self, include_all: bool | None = None) -> str:
        """List registered sessions in the default user registry."""
        return self._sync.registry_list(include_all)

    def registry_resolve(self, name: str) -> str | None:
        """Resolve a friendly session name to its registered session path."""
        return self._sync.registry_resolve(name)

    def registry_register(self, name: str, session_path: str) -> str:
        """Register a friendly name for an existing canonical session directory."""
        return self._sync.registry_register(name, session_path)

    def registry_remove(self, name: str) -> bool:
        """Remove a friendly name binding."""
        return self._sync.registry_remove(name)

    def registry_gc(
        self,
        older_than_days: int | None = None,
        keep_latest: int | None = None,
        max_completed_bytes: int | None = None,
        apply: bool | None = None,
    ) -> str:
        """Plan or execute garbage collection over completed sessions."""
        return self._sync.registry_gc(older_than_days, keep_latest, max_completed_bytes, apply)

    def tmux_bundle(self, manifest_path: str, destination: str) -> str:
        """Bundle an aggregate tmux recording into an evidentiary streaming bundle."""
        return self._sync.tmux_bundle(manifest_path, destination)

    def tmux_verify_bundle(self, bundle_directory: str, manifest_path: str) -> bool:
        """Verify an aggregate tmux streaming bundle against its manifest."""
        return self._sync.tmux_verify_bundle(bundle_directory, manifest_path)

    def probe_capabilities(self) -> str:
        """Probe runtime system capabilities across engine, media, and evidence signers."""
        return self._sync.probe_capabilities()
