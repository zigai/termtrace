"""Typed builders for admission-guarded keyboard input sequences."""

from __future__ import annotations

import base64
import uuid
from collections.abc import Sequence
from dataclasses import dataclass
from typing import Literal, TypeAlias

import msgspec

from termtrace.duration import Duration
from termtrace.errors import InvalidAtomError, InvalidDurationError
from termtrace.transport.generated import driver_request as request_wire
from termtrace.transport.protocol import JsonObject, JsonValue


@dataclass(frozen=True, slots=True)
class TextAtom:
    """One UTF-8 text write in an atomic input sequence."""

    text: str
    caller_intent: str | None = None


@dataclass(frozen=True, slots=True)
class BytesAtom:
    """One exact byte write in an atomic input sequence."""

    data: bytes
    input_kind: Literal["raw", "text"] = "raw"
    caller_intent: str | None = None


@dataclass(frozen=True, slots=True)
class KeyAtom:
    """One symbolic key resolved against actor state at sequence admission."""

    key: str
    shift: bool = False
    control: bool = False
    alt: bool = False
    super_key: bool = False
    caller_intent: str | None = None


@dataclass(frozen=True, slots=True)
class PasteAtom:
    """One paste resolved against bracketed-paste mode at sequence admission."""

    data: str | bytes
    caller_intent: str | None = None


InputAtom: TypeAlias = TextAtom | BytesAtom | KeyAtom | PasteAtom
KeyboardAtom: TypeAlias = InputAtom


def encode_input_sequence(
    atoms: Sequence[KeyboardAtom],
    *,
    pace_ms: int,
    verbose: bool,
    sequence_id: str | None = None,
) -> JsonObject:
    """Encode public atom types into the strict driver command contract."""
    pace = Duration.milliseconds(pace_ms)
    wire_atoms = _encode_wire_atoms(atoms, request_id=sequence_id)
    if pace.nanoseconds > Duration.seconds(60).nanoseconds:
        raise InvalidDurationError("pace must not exceed 60 seconds")
    if pace.nanoseconds * len(atoms) > Duration.seconds(60).nanoseconds:
        raise InvalidDurationError("total input sequence pacing must not exceed 60 seconds")
    command = request_wire.ControlCommand8(
        atoms=wire_atoms,
        pace_ns=pace.nanoseconds,
        include_atom_outcomes=verbose,
    )
    data = msgspec.to_builtins(command)
    if not isinstance(data, dict) or not all(isinstance(key, str) for key in data):
        raise AssertionError("input command did not serialize to an object")
    return data


def encode_atoms(
    atoms: Sequence[InputAtom],
    *,
    request_id: str | None = None,
    caller_intent: str | None = None,
) -> list[JsonValue]:
    """Validate and deterministically encode the shared action atom vocabulary."""
    return [
        msgspec.to_builtins(atom) for atom in _encode_wire_atoms(atoms, request_id, caller_intent)
    ]


def _encode_wire_atoms(
    atoms: Sequence[InputAtom],
    request_id: str | None = None,
    caller_intent: str | None = None,
) -> list[request_wire.InputSequenceAtom]:
    if not 1 <= len(atoms) <= 4096:
        raise InvalidAtomError("input sequence must contain 1 through 4096 atoms")
    namespace = uuid.UUID(request_id) if request_id is not None else uuid.uuid4()
    encoded: list[request_wire.InputSequenceAtom] = []
    for index, atom in enumerate(atoms):
        operation_id = str(uuid.UUID(int=namespace.int ^ (index + 1)))
        intent = atom.caller_intent if atom.caller_intent is not None else caller_intent
        if isinstance(atom, TextAtom):
            if not atom.text:
                raise InvalidAtomError(f"text atom {index} must not be empty")
            encoded.append(
                request_wire.InputSequenceAtom1(
                    data=base64.b64encode(atom.text.encode()).decode(),
                    input_kind=request_wire.InputKind.text,
                    operation_id=operation_id,
                    caller_intent=intent,
                )
            )
            continue
        if isinstance(atom, BytesAtom):
            if not atom.data:
                raise InvalidAtomError(f"bytes atom {index} must not be empty")
            encoded.append(
                request_wire.InputSequenceAtom1(
                    data=base64.b64encode(atom.data).decode(),
                    input_kind=request_wire.InputKind(atom.input_kind),
                    operation_id=operation_id,
                    caller_intent=intent,
                )
            )
            continue
        if isinstance(atom, KeyAtom):
            if not atom.key:
                raise InvalidAtomError(f"key atom {index} must not be empty")
            encoded.append(
                request_wire.InputSequenceAtom2(
                    key=atom.key,
                    modifiers=request_wire.KeyModifiers(
                        shift=atom.shift,
                        control=atom.control,
                        alt=atom.alt,
                        super_key=atom.super_key,
                    ),
                    operation_id=operation_id,
                    caller_intent=intent,
                )
            )
            continue
        if isinstance(atom, PasteAtom):
            raw = atom.data.encode() if isinstance(atom.data, str) else atom.data
            encoded.append(
                request_wire.InputSequenceAtom3(
                    data=base64.b64encode(raw).decode(),
                    operation_id=operation_id,
                    caller_intent=intent,
                )
            )
            continue
        raise InvalidAtomError(f"atom {index} has an unsupported type")
    return encoded


__all__ = [
    "BytesAtom",
    "InputAtom",
    "KeyAtom",
    "KeyboardAtom",
    "PasteAtom",
    "TextAtom",
    "encode_atoms",
    "encode_input_sequence",
]
