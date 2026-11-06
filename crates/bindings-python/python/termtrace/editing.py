"""Typed builders for presentation edits.

Builders only hold values and reject wrong Python types (``bool`` or ``float`` where an exact
integer is required, and selector values outside ``0..2**64-1``). Validation, subtitle path
resolution and the ``edit_plan_hash`` come from the one Rust normalizer.

Frame rounding: a clip's first source frame is the floor of its start and its end frame the
ceiling of its end. The retimed frame count is ``ceil(frames * denominator / numerator)``,
the hold is ``ceil(hold_ms * fps / 1000)`` frames, and output frames map back to source
frames with a floor. ``Select.native_frame(n)`` starts at ``ceil(n * 1e9 / fps)`` ns, so it
selects frame ``n`` exactly.
"""

from __future__ import annotations

import copy
import os
from dataclasses import dataclass
from fractions import Fraction
from typing import TYPE_CHECKING

from termtrace import _termtrace

if TYPE_CHECKING:
    from termtrace import types

_U64_MAX = 2**64 - 1

Speed = Fraction | tuple[int, int]


def _integer(value: object, name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise TypeError(f"{name} must be an int, not {type(value).__name__}")
    return value


def _u64(value: object, name: str) -> int:
    value = _integer(value, name)
    if not 0 <= value <= _U64_MAX:
        raise ValueError(f"{name} must be in 0..=2**64-1, got {value}")
    return value


@dataclass(frozen=True)
class Selector:
    """One source coordinate: exactly one marker, time, event, state or native frame."""

    wire: types.EditSelectorInput

    def to_wire(self) -> types.EditSelectorInput:
        return dict(self.wire)  # type: ignore[return-value]


class Select:
    """Selector constructors; integers are exact u64 values."""

    @staticmethod
    def marker(label: str) -> Selector:
        if not isinstance(label, str):
            raise TypeError(f"marker must be a str, not {type(label).__name__}")
        return Selector({"marker": label})

    @staticmethod
    def at_ns(offset: int) -> Selector:
        return Selector({"monotonicNs": _u64(offset, "monotonic_ns")})

    @staticmethod
    def event(seq: int) -> Selector:
        return Selector({"eventSeq": _u64(seq, "event_seq")})

    @staticmethod
    def state(state_id: int) -> Selector:
        return Selector({"stateId": _u64(state_id, "state_id")})

    @staticmethod
    def native_frame(frame: int) -> Selector:
        return Selector({"nativeFrame": _u64(frame, "native_frame")})


class Scope:
    """Annotation lifetimes on the output timeline."""

    @staticmethod
    def whole_video() -> types.AnnotationScopeInput:
        return {"scope": "whole_video"}

    @staticmethod
    def clip(index: int) -> types.AnnotationScopeInput:
        return {"scope": "clip", "clip_index": _integer(index, "clip_index")}

    @staticmethod
    def output_frame(frame: int) -> types.AnnotationScopeInput:
        return {"scope": "output_frame", "output_frame": _u64(frame, "output_frame")}

    @staticmethod
    def range(from_: Selector, to: Selector) -> types.AnnotationScopeInput:
        return {"scope": "range", "from": from_.to_wire(), "to": to.to_wire()}


def _speed(speed: Speed) -> types.RationalSpeed:
    if isinstance(speed, Fraction):
        return {"numerator": speed.numerator, "denominator": speed.denominator}
    if isinstance(speed, tuple) and len(speed) == 2:
        return {
            "numerator": _integer(speed[0], "speed numerator"),
            "denominator": _integer(speed[1], "speed denominator"),
        }
    raise TypeError(
        f"speed must be a Fraction or (numerator, denominator), not {type(speed).__name__}"
    )


class Edit:
    """Presentation edit builder. Methods append in call order and return the builder."""

    def __init__(self, *, resource_base: str | os.PathLike[str] | None = None) -> None:
        self.resource_base = None if resource_base is None else os.fspath(resource_base)
        self._plan: types.EditPlanInput = {"version": 2, "clips": []}

    def clip(
        self,
        from_: Selector,
        to: Selector,
        *,
        speed: Speed = (1, 1),
        hold_ms: int = 0,
        caption: str | None = None,
    ) -> Edit:
        self._plan["clips"].append(
            {
                "from": from_.to_wire(),
                "to": to.to_wire(),
                "speed": _speed(speed),
                "hold_ms": _u64(hold_ms, "hold_ms"),
                "caption": caption,
            }
        )
        return self

    def annotate(
        self,
        text: str | None = None,
        *,
        template: str | None = None,
        scope: types.AnnotationScopeInput,
        position: types.AnnotationPosition | None = None,
        style: types.AnnotationStyle | None = None,
        z_order: int = 0,
    ) -> Edit:
        annotation: types.TextAnnotationInput = {
            "text": text,
            "template": template,
            "scope": scope,
            "z_order": _integer(z_order, "z_order"),
        }
        if position is not None:
            annotation["position"] = position
        if style is not None:
            annotation["style"] = style
        self._plan.setdefault("annotations", []).append(annotation)
        return self

    def subtitles(self, ass: str | os.PathLike[str]) -> Edit:
        self._plan.setdefault("subtitles", []).append({"ass": os.fspath(ass)})
        return self

    def changes(
        self,
        source: types.ChangeSource,
        mode: types.ChangeMode,
        *,
        hold_frames: int = 0,
        pixel_threshold: int | None = None,
    ) -> Edit:
        self._plan["change_visualization"] = {
            "source": source,
            "mode": mode,
            "hold_frames": _integer(hold_frames, "hold_frames"),
            "pixel_threshold": None
            if pixel_threshold is None
            else _integer(pixel_threshold, "pixel_threshold"),
        }
        return self

    def caption_layout(self, layout: types.CaptionLayout) -> Edit:
        self._plan["layout"] = {**self._layout(), "caption": layout}
        return self

    def branding(self, label: str) -> Edit:
        self._plan["layout"] = {**self._layout(), "branding": label}
        return self

    def _layout(self) -> types.PresentationLayoutInput:
        default: types.PresentationLayoutInput = {"caption": "footer"}
        return self._plan.get("layout", default)

    def delivery(
        self, *, codec: str, pixel_format: str, crf: int, encoder_preset: str
    ) -> Edit:
        self._plan["delivery"] = {
            "codec": codec,
            "pixel_format": pixel_format,
            "crf": _integer(crf, "crf"),
            "encoder_preset": encoder_preset,
        }
        return self

    def cursor_mask(self) -> Edit:
        return self._transform({"transform": "cursor_mask"})

    def crop(self, x: int, y: int, width: int, height: int) -> Edit:
        return self._transform(
            {
                "transform": "crop",
                "x": _integer(x, "x"),
                "y": _integer(y, "y"),
                "width": _integer(width, "width"),
                "height": _integer(height, "height"),
            }
        )

    def zoom(self, numerator: int, denominator: int) -> Edit:
        return self._transform(
            {
                "transform": "zoom",
                "numerator": _integer(numerator, "numerator"),
                "denominator": _integer(denominator, "denominator"),
            }
        )

    def resize(self, width: int, height: int) -> Edit:
        return self._transform(
            {
                "transform": "resize",
                "width": _integer(width, "width"),
                "height": _integer(height, "height"),
            }
        )

    def callout(
        self, x: int, y: int, width: int, height: int, *, label: str | None = None
    ) -> Edit:
        return self._transform(
            {
                "transform": "callout",
                "x": _integer(x, "x"),
                "y": _integer(y, "y"),
                "width": _integer(width, "width"),
                "height": _integer(height, "height"),
                "label": label,
            }
        )

    def _transform(self, transform: types.PixelTransformInput) -> Edit:
        self._plan.setdefault("transforms", []).append(transform)
        return self

    def build(self) -> types.EditPlanInput:
        """Return a copy of the edit plan as built; sections never set keep Rust defaults."""
        return copy.deepcopy(self._plan)

    def to_source(self) -> types.EditSourceInput:
        return {
            "kind": "plan",
            "edit": self.build(),
            "resource_base": self.resource_base,
        }

    def normalize(self) -> types.NormalizedEdit:
        """Resolve, validate and hash the edit in Rust."""
        return _termtrace.normalize_edit(self)
