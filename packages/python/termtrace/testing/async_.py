"""Native-async ownership for terminal tests without hidden event loops."""

from __future__ import annotations

import asyncio
import shutil
from collections.abc import Awaitable
from pathlib import Path
from types import TracebackType
from typing import Literal, Self, TypeVar

from termtrace.screen.frame import ScreenFrame
from termtrace.screen.locators import AsyncLocator
from termtrace.screen.queries import LocatorSpec, ScreenMatch
from termtrace.screen.snapshot import FrameSnapshot, SnapshotProfile
from termtrace.session.async_ import AsyncKeyboard, AsyncScreen, AsyncSession
from termtrace.testing.assertions import (
    assert_cursor,
    assert_frame_text,
    assert_matches,
    assert_no_matches,
    assert_snapshot_file,
)
from termtrace.testing.lifecycle import (
    FinalizationReport,
    TermtraceTestConfig,
    TestOutcome,
    _bounded_error,
    _contained_path,
    _session_evidence_policy,
    _validate_snapshot_name,
    _write_evidence_manifest,
    classify_outcome,
    create_evidence_directory,
)
from termtrace.transport.async_ import AsyncTerminal

PhaseT = TypeVar("PhaseT")


class AsyncTerminalAssertions:
    """Runner-neutral assertions bound to one native-async session."""

    def __init__(self, owner: AsyncTermtraceTest) -> None:
        self._owner = owner

    async def screen_text(self, expected: str) -> None:
        assert_frame_text(await self._owner.session.screen.frame(), expected)

    async def matches(
        self,
        spec: LocatorSpec,
        *,
        count: int | None = None,
        frame: ScreenFrame | None = None,
    ) -> tuple[ScreenMatch, ...]:
        selected = frame or await self._owner.session.screen.frame()
        return assert_matches(selected, spec, count=count)

    async def no_matches(
        self,
        spec: LocatorSpec,
        *,
        frame: ScreenFrame | None = None,
    ) -> None:
        selected = frame or await self._owner.session.screen.frame()
        assert_no_matches(selected, spec)

    async def cursor(
        self,
        *,
        row: int | None = None,
        column: int | None = None,
        visible: bool | None = None,
        frame: ScreenFrame | None = None,
    ) -> None:
        selected = frame or await self._owner.session.screen.frame()
        assert_cursor(
            selected,
            row=row,
            column=column,
            visible=visible,
        )

    async def snapshot(
        self,
        name: str,
        *,
        profile: SnapshotProfile = "spans",
        frame: ScreenFrame | None = None,
        stable_for: float = 0.25,
    ) -> FrameSnapshot:
        return await self._owner.assert_snapshot(
            name,
            profile=profile,
            frame=frame,
            stable_for=stable_for,
        )


class AsyncTermtraceTest:
    """Own exactly one native-async driver and session for a test attempt."""

    def __init__(
        self,
        config: TermtraceTestConfig,
        terminal: AsyncTerminal,
        session: AsyncSession,
        evidence_directory: Path,
    ) -> None:
        self.config = config
        self._terminal = terminal
        self.session = session
        self.evidence_directory = evidence_directory
        self._lock = asyncio.Lock()
        self._report: FinalizationReport | None = None
        self._finalizing = False
        self.assertions = AsyncTerminalAssertions(self)
        self._snapshot_modes: dict[str, str] = {}

    @classmethod
    async def start(cls, config: TermtraceTestConfig) -> AsyncTermtraceTest:
        if not config.command:
            raise ValueError("terminal test command must not be empty")
        evidence_directory = create_evidence_directory(config)
        terminal: AsyncTerminal | None = None
        try:
            terminal = await AsyncTerminal.create(
                binary_path=config.binary_path,
                evidence_root=evidence_directory,
            )
            session = await terminal.launch(
                config.command,
                cols=config.cols,
                rows=config.rows,
                working_directory=config.working_directory,
                evidence=_session_evidence_policy(config.evidence_policy),
                environment=config.environment,
                output_byte_budget=config.output_byte_budget,
            )
        except BaseException:
            if terminal is not None:
                await terminal.close()
            await asyncio.to_thread(shutil.rmtree, evidence_directory, True)
            raise
        return cls(config, terminal, session, evidence_directory)

    @property
    def screen(self) -> AsyncScreen:
        return self.session.screen

    @property
    def keyboard(self) -> AsyncKeyboard:
        return self.session.keyboard

    def locator(self, spec: LocatorSpec) -> AsyncLocator:
        return self.session.screen.locator(spec)

    @property
    def report(self) -> FinalizationReport | None:
        return self._report

    async def assert_snapshot(
        self,
        name: str,
        *,
        profile: SnapshotProfile = "spans",
        frame: ScreenFrame | None = None,
        stable_for: float = 0.25,
    ) -> FrameSnapshot:
        logical_name = _validate_snapshot_name(name)
        previous_mode = self._snapshot_modes.setdefault(logical_name, profile)
        if previous_mode != profile:
            raise ValueError(
                f"snapshot {logical_name!r} was already used with profile {previous_mode!r}"
            )
        if profile == "cells" and frame is None:
            raise ValueError("cells snapshots require an explicitly region-bounded ScreenFrame")
        selected = frame
        if selected is None:
            selected = (await self.session.screen.capture_stable(stable_for=stable_for)).frame
        destination = self.config.snapshot_root / f"{logical_name}.json"
        return await asyncio.to_thread(
            assert_snapshot_file,
            selected,
            destination,
            profile=profile,
            update=self.config.update_snapshots,
        )

    async def finalize(
        self,
        *,
        outcome: TestOutcome,
        primary_failure: BaseException | None = None,
    ) -> FinalizationReport:
        cancellation: asyncio.CancelledError | None = None
        async with self._lock:
            if self._report is None:
                if self._finalizing:
                    raise RuntimeError("terminal test finalization state is invalid")
                self._finalizing = True
                self._report, cancellation = await self._finalize_once(outcome)
            report = self._report
        if primary_failure is not None:
            for message in report.secondary_failures:
                primary_failure.add_note(f"Termtrace finalization: {message}")
        if cancellation is not None:
            raise cancellation
        return report

    async def _recover_closed_driver(self, phases: list[str], secondary: list[str]) -> bool:
        if not self._terminal.closed:
            return False
        canonical_session = self.session.canonical_session
        try:
            await self._terminal.close()
        except Exception as e:
            secondary.append(_bounded_error("closed driver cleanup", e))
        replacement: AsyncTerminal | None = None
        try:
            replacement = await AsyncTerminal.create(
                binary_path=self.config.binary_path,
                evidence_root=self.evidence_directory,
            )
            session = await replacement.attach(canonical_session)
        except Exception as e:
            if replacement is not None:
                await replacement.close()
            secondary.append(_bounded_error("driver recovery", e))
            return False
        self._terminal = replacement
        self.session = session
        phases.append("driver_recovered")
        return True

    async def _phase(self, awaitable: Awaitable[PhaseT]) -> PhaseT:
        async with asyncio.timeout(self.config.finalization_timeout):
            return await awaitable

    async def _finalize_once(
        self, outcome: TestOutcome
    ) -> tuple[FinalizationReport, asyncio.CancelledError | None]:
        secondary: list[str] = []
        phases: list[str] = []
        artifacts: list[Path] = []
        verified_artifacts: list[Path] = []
        body_passed = outcome == "passed"
        completion_succeeded = False
        stop_error: Exception | None = None
        cancellation: asyncio.CancelledError | None = None
        recovered_driver = await self._recover_closed_driver(phases, secondary)
        try:
            await self._phase(self.session.stop())
            phases.append("stop_requested")
        except asyncio.CancelledError as e:
            cancellation = e
        except Exception as e:
            stop_error = e
        if self._terminal.closed:
            recovered_driver = (
                await self._recover_closed_driver(phases, secondary) or recovered_driver
            )

        try:
            await self._phase(
                self.session.evidence.complete(timeout=self.config.finalization_timeout)
            )
            phases.append("completed")
            completion_succeeded = True
        except asyncio.CancelledError as e:
            if cancellation is None:
                cancellation = e
        except Exception as e:
            if stop_error is not None:
                secondary.append(_bounded_error("session stop", stop_error))
            secondary.append(_bounded_error("session completion", e))

        if not body_passed and completion_succeeded:
            try:
                result = await self._phase(
                    self.session.screen.screenshot(self.evidence_directory / "semantic-screenshot")
                )
                screenshot_artifact = _contained_path(
                    self.evidence_directory, Path(result.directory)
                )
                artifacts.append(screenshot_artifact)
                verified_artifacts.append(screenshot_artifact)
                phases.append("semantic_screenshot")
            except asyncio.CancelledError as e:
                if cancellation is None:
                    cancellation = e
            except Exception as e:
                secondary.append(_bounded_error("semantic screenshot", e))
            try:
                bundle_result = await self._phase(
                    self.session.evidence.bundle(
                        self.evidence_directory / "screen-bundle",
                        profile="screen",
                    )
                )
                bundle_artifact = _contained_path(
                    self.evidence_directory, Path(bundle_result.directory)
                )
                artifacts.append(bundle_artifact)
                verified_artifacts.append(bundle_artifact)
                phases.append("screen_bundle")
            except asyncio.CancelledError as e:
                if cancellation is None:
                    cancellation = e
            except Exception as e:
                secondary.append(_bounded_error("screen bundle", e))
            try:
                verification = await self._phase(self.session.evidence.verify("content"))
                if not verification.valid:
                    secondary.append("canonical evidence verifier reported invalid evidence")
                else:
                    canonical_artifact = _contained_path(
                        self.evidence_directory,
                        Path(verification.session),
                    )
                    artifacts.append(canonical_artifact)
                    verified_artifacts.append(canonical_artifact)
                    phases.append("canonical_verified")
            except asyncio.CancelledError as e:
                if cancellation is None:
                    cancellation = e
            except Exception as e:
                secondary.append(_bounded_error("canonical verification", e))
        if completion_succeeded:
            try:
                if recovered_driver:
                    await self._phase(self.session.close())
                    phases.append("session_detached")
                else:
                    await self._phase(self.session.finish(passed=False))
                    phases.append("session_finished")
            except asyncio.CancelledError as e:
                if cancellation is None:
                    cancellation = e
            except Exception as e:
                secondary.append(_bounded_error("session finish", e))
        try:
            await self._phase(self._terminal.close())
            phases.append("driver_closed")
        except asyncio.CancelledError as e:
            if cancellation is None:
                cancellation = e
        except Exception as e:
            secondary.append(_bounded_error("driver close", e))

        retained = (
            self.config.evidence_policy != "on_failure"
            or not body_passed
            or bool(secondary)
            or cancellation is not None
        )
        if retained:
            try:
                manifest_path = await asyncio.to_thread(
                    _write_evidence_manifest,
                    config=self.config,
                    evidence_directory=self.evidence_directory,
                    session_id=self.session.session_id,
                    outcome=outcome,
                    artifacts=artifacts,
                    secondary_errors=secondary,
                    lifecycle_phases=phases,
                )
                artifacts.append(manifest_path)
                phases.append("manifest_written")
            except asyncio.CancelledError as e:
                if cancellation is None:
                    cancellation = e
            except Exception as e:
                secondary.append(_bounded_error("adapter manifest", e))
        else:
            try:
                await asyncio.to_thread(shutil.rmtree, self.evidence_directory)
                phases.append("evidence_discarded")
            except asyncio.CancelledError as e:
                retained = True
                if cancellation is None:
                    cancellation = e
            except Exception as e:
                retained = True
                secondary.append(_bounded_error("evidence discard", e))
                try:
                    manifest_path = await asyncio.to_thread(
                        _write_evidence_manifest,
                        config=self.config,
                        evidence_directory=self.evidence_directory,
                        session_id=self.session.session_id,
                        outcome=outcome,
                        artifacts=artifacts,
                        secondary_errors=secondary,
                        lifecycle_phases=phases,
                    )
                    artifacts.append(manifest_path)
                    phases.append("manifest_written")
                except asyncio.CancelledError as e:
                    if cancellation is None:
                        cancellation = e
                except Exception as manifest_error:
                    secondary.append(_bounded_error("adapter manifest", manifest_error))

        report = FinalizationReport(
            outcome=outcome,
            evidence_directory=self.evidence_directory if retained else None,
            retained=retained,
            artifacts=tuple(artifacts),
            verified_artifacts=tuple(verified_artifacts),
            secondary_failures=tuple(secondary),
            lifecycle_phases=tuple(phases),
        )
        return report, cancellation

    async def __aenter__(self) -> Self:
        return self

    async def __aexit__(
        self,
        error_type: type[BaseException] | None,
        error: BaseException | None,
        traceback: TracebackType | None,
    ) -> Literal[False]:
        report = await self.finalize(
            outcome=classify_outcome(error),
            primary_failure=error,
        )
        if error is None and report.secondary_failures:
            raise RuntimeError("; ".join(report.secondary_failures))
        return False
