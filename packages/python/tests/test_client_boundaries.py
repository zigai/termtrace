import asyncio
import math
import sys
import uuid
from pathlib import Path

import pytest

from termtrace import BytesAtom, KeyAtom, PasteAtom, TextAtom
from termtrace.atoms import encode_input_sequence
from termtrace.errors import ProtocolMismatchError, RequestTimeoutError
from termtrace.session.async_ import AsyncSession
from termtrace.session.sync import Session
from termtrace.transport.async_ import AsyncTerminal
from termtrace.transport.protocol import DriverHelloData, encode_driver_request
from termtrace.transport.sync import DriverTransport


@pytest.mark.parametrize("duration", [-1.0, math.nan, math.inf, 1e30, sys.float_info.max])
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


def test_async_request_cancellation_closes_desynchronized_transport() -> None:
    async def exercise() -> None:
        script = (
            "import sys, time\n"
            "sys.stdin.buffer.readline()\n"
            "sys.stderr.write('received\\n')\n"
            "sys.stderr.flush()\n"
            "time.sleep(60)\n"
        )
        process = await asyncio.create_subprocess_exec(
            sys.executable,
            "-c",
            script,
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        terminal = AsyncTerminal(process, None)
        request = asyncio.create_task(terminal.request({"method": "hello"}, DriverHelloData))
        assert process.stderr is not None
        assert await process.stderr.readline() == b"received\n"

        request.cancel()
        with pytest.raises(asyncio.CancelledError):
            await request
        with pytest.raises(ProtocolMismatchError, match="driver is closed"):
            await terminal.request({"method": "hello"}, DriverHelloData)

        await terminal.close()
        assert process.returncode is not None

    asyncio.run(exercise())


def test_sync_request_timeout_terminates_desynchronized_transport(
    tmp_path: Path,
) -> None:
    driver = tmp_path / "driver"
    driver.write_text(
        "#!/bin/sh\nIFS= read -r request\nexec sleep 30\n",
        encoding="utf-8",
    )
    driver.chmod(0o700)
    transport = DriverTransport(driver)

    with pytest.raises(RequestTimeoutError) as captured:
        transport.request(
            {"method": "hello"},
            DriverHelloData,
            timeout=0.02,
        )
    assert captured.value.uncertain is True
    assert captured.value.retry_safety == "unsafe"
    with pytest.raises(ProtocolMismatchError, match="driver is closed"):
        transport.request({"method": "hello"}, DriverHelloData)
    transport.close()


def test_atomic_input_sequence_builder_is_typed_bounded_and_correlated() -> None:
    sequence_id = "00000000-0000-0000-0000-000000000020"
    command = encode_input_sequence(
        [
            TextAtom("go", caller_intent="submit text"),
            KeyAtom("enter", control=True, caller_intent="submit key"),
            PasteAtom("value", caller_intent="paste value"),
            BytesAtom(b"\x00", caller_intent="raw byte"),
        ],
        pace_ms=5,
        verbose=True,
        sequence_id=sequence_id,
    )
    retry = encode_input_sequence(
        [
            TextAtom("go", caller_intent="submit text"),
            KeyAtom("enter", control=True, caller_intent="submit key"),
            PasteAtom("value", caller_intent="paste value"),
            BytesAtom(b"\x00", caller_intent="raw byte"),
        ],
        pace_ms=5,
        verbose=True,
        sequence_id=sequence_id,
    )

    assert command["command"] == "input_sequence"
    assert command["pace_ns"] == 5_000_000
    assert command["include_atom_outcomes"] is True
    atoms = command["atoms"]
    assert isinstance(atoms, list)
    assert len(atoms) == 4
    operation_ids = [
        str(uuid.UUID(str(atom["operation_id"]))) for atom in atoms if isinstance(atom, dict)
    ]
    assert len(operation_ids) == 4
    assert len(set(operation_ids)) == 4
    assert sequence_id not in operation_ids
    assert [atom["caller_intent"] for atom in atoms if isinstance(atom, dict)] == [
        "submit text",
        "submit key",
        "paste value",
        "raw byte",
    ]
    assert retry == command
    encode_driver_request(
        {
            "method": "control",
            "handle": "00000000-0000-0000-0000-000000000021",
            "command": command,
            "expected_presented_state_hash": None,
        },
        sequence_id,
    )

    with pytest.raises(ValueError, match="1 through 4096"):
        encode_input_sequence([], pace_ms=0, verbose=False)
    with pytest.raises(ValueError, match="total"):
        encode_input_sequence(
            [TextAtom("a"), TextAtom("b"), TextAtom("c")],
            pace_ms=40_000,
            verbose=False,
        )
