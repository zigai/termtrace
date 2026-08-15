"""Pure session request construction and response lifecycle decisions."""

from __future__ import annotations

import uuid
from collections.abc import Sequence

from termtrace.atoms import InputAtom, encode_atoms
from termtrace.duration import Duration
from termtrace.errors import PostconditionIncompleteError
from termtrace.screen.wire import encode_frame_request
from termtrace.session.actions import ActionOptions, ActionResult, is_incomplete_completion
from termtrace.session.wire import encode_postcondition
from termtrace.transport.protocol import JsonObject


def action_request(atoms: Sequence[InputAtom], options: ActionOptions) -> tuple[str, JsonObject]:
    request_id = options.request_id or str(uuid.uuid4())
    pace = options.pace
    if pace > Duration.seconds(60):
        raise ValueError("action pace must not exceed 60 seconds")
    if pace.nanoseconds * len(atoms) > Duration.seconds(60).nanoseconds:
        raise ValueError("total action pacing must not exceed 60 seconds")
    return request_id, {
        "command": "act",
        "atoms": encode_atoms(
            atoms,
            request_id=request_id,
            caller_intent=options.caller_intent,
        ),
        "pace_ns": pace.nanoseconds,
        "include_atom_outcomes": options.verbose_atom_outcomes,
        "postcondition": encode_postcondition(options.postcondition),
        "projection": encode_frame_request(options.frame),
    }


def require_complete_action(result: ActionResult, *, allow_incomplete: bool) -> ActionResult:
    if is_incomplete_completion(result.postcondition) and not allow_incomplete:
        raise PostconditionIncompleteError(result)
    return result


__all__ = ["action_request", "require_complete_action"]
