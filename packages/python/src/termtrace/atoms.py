"""Typed builders for admission-guarded keyboard input sequences."""

from __future__ import annotations

import base64
import uuid
from collections.abc import Sequence
from dataclasses import dataclass
from typing import Literal, TypeAlias

from termtrace.models import JsonObject, JsonValue


@dataclass(frozen=True, slots=True)
class TextAtom:
    """One UTF-8 text write in an atomic input sequence."""

    text: str
    intent: str | None = None


@dataclass(frozen=True, slots=True)
class BytesAtom:
    """One exact byte write in an atomic input sequence."""

    data: bytes
    input_kind: Literal["raw", "text"] = "raw"
    intent: str | None = None


@dataclass(frozen=True, slots=True)
class KeyAtom:
    """One symbolic key resolved against actor state at sequence admission."""

    key: str
    shift: bool = False
    control: bool = False
    alt: bool = False
    super_key: bool = False


@dataclass(frozen=True, slots=True)
class PasteAtom:
    """One paste resolved against bracketed-paste mode at sequence admission."""

    data: str | bytes


KeyboardAtom: TypeAlias = TextAtom | BytesAtom | KeyAtom | PasteAtom


def encode_input_sequence(
    atoms: Sequence[KeyboardAtom],
    *,
    pace_ms: int,
    verbose: bool,
    sequence_id: str | None = None,
) -> JsonObject:
    """Encode public atom types into the strict driver command contract."""

    if not 1 <= len(atoms) <= 4096:
        raise ValueError("input sequence must contain 1 through 4096 atoms")
    if not 0 <= pace_ms <= 60_000:
        raise ValueError("pace_ms must be between 0 and 60000")
    if pace_ms * max(len(atoms) - 1, 0) > 60_000:
        raise ValueError("total input sequence pacing must not exceed 60000 ms")
    namespace = uuid.UUID(sequence_id) if sequence_id is not None else uuid.uuid4()
    encoded: list[JsonValue] = []
    for index, atom in enumerate(atoms):
        operation_id = str(uuid.uuid5(namespace, f"termtrace-input-atom:{index}"))
        if isinstance(atom, TextAtom):
            encoded.append(
                {
                    "input": "bytes",
                    "operation_id": operation_id,
                    "data": base64.b64encode(atom.text.encode()).decode(),
                    "input_kind": "text",
                    "intent": atom.intent,
                }
            )
        elif isinstance(atom, BytesAtom):
            encoded.append(
                {
                    "input": "bytes",
                    "operation_id": operation_id,
                    "data": base64.b64encode(atom.data).decode(),
                    "input_kind": atom.input_kind,
                    "intent": atom.intent,
                }
            )
        elif isinstance(atom, KeyAtom):
            encoded.append(
                {
                    "input": "key",
                    "operation_id": operation_id,
                    "key": atom.key,
                    "modifiers": {
                        "shift": atom.shift,
                        "control": atom.control,
                        "alt": atom.alt,
                        "super_key": atom.super_key,
                    },
                }
            )
        else:
            raw = atom.data.encode() if isinstance(atom.data, str) else atom.data
            encoded.append(
                {
                    "input": "paste",
                    "operation_id": operation_id,
                    "data": base64.b64encode(raw).decode(),
                }
            )
    return {
        "command": "input_sequence",
        "atoms": encoded,
        "pace_ns": pace_ms * 1_000_000,
        "include_atom_outcomes": verbose,
    }


__all__ = ["BytesAtom", "KeyAtom", "KeyboardAtom", "PasteAtom", "TextAtom"]
