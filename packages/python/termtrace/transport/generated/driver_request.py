# Generated from schemas/driver-request.schema.json. DO NOT EDIT.

from __future__ import annotations

from enum import StrEnum
from typing import Annotated, TypeAlias

from msgspec import UNSET, Meta, Struct, UnsetType, convert, field


class ActionPostcondition1(Struct, forbid_unknown_fields=True, tag_field='condition', tag='none'):
    pass


class ActionPostcondition2(Struct, forbid_unknown_fields=True, tag_field='condition', tag='idle'):
    stable_for_ns: Annotated[
        int, Meta(description='Required unchanged presented-state interval.', ge=0)
    ]
    timeout_ns: Annotated[
        int, Meta(description='Server-side monotonic deadline interval.', ge=0)
    ]


Base64Bytes: TypeAlias = Annotated[str, Meta(pattern='^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$')]


class BundleProfile(StrEnum):
    screen = 'screen'
    debug = 'debug'
    forensic = 'forensic'


class CaptureTarget(Struct, forbid_unknown_fields=True):
    height: Annotated[
        int, Meta(description='Viewport height in pixels.', ge=0, le=65535)
    ]
    width: Annotated[int, Meta(description='Viewport width in pixels.', ge=0, le=65535)]


class ColorPolicy(StrEnum):
    auto = 'auto'
    always = 'always'
    never = 'never'


class ControlCommand1(Struct, forbid_unknown_fields=True, tag_field='command', tag='ping'):
    pass


class ControlCommand2(Struct, forbid_unknown_fields=True, tag_field='command', tag='status'):
    pass


class ControlCommand3(Struct, forbid_unknown_fields=True, tag_field='command', tag='snapshot'):
    pass


class ControlCommand11(Struct, forbid_unknown_fields=True, tag_field='command', tag='paste'):
    data: Annotated[
        Base64Bytes,
        Meta(
            description='Paste content before mode-dependent wrapping or newline conversion.'
        ),
    ]


class ControlCommand12(Struct, forbid_unknown_fields=True, tag_field='command', tag='focus'):
    focused: Annotated[
        bool,
        Meta(
            description='Whether the terminal gained (`true`) or lost (`false`) focus.'
        ),
    ]
    strict: Annotated[
        bool,
        Meta(
            description='Reject instead of returning a semantic no-op when reporting is disabled.'
        ),
    ]


class ControlCommand14(Struct, forbid_unknown_fields=True, tag_field='command', tag='activate'):
    strict: Annotated[
        bool,
        Meta(
            description='Reject instead of returning a semantic no-op when reporting is disabled.'
        ),
    ]
    text: Annotated[
        str, Meta(description='Exact row-contained visible text to resolve.')
    ]
    unique: Annotated[
        bool,
        Meta(description='Require exactly one match instead of choosing the first.'),
    ]


class ControlCommand15(Struct, forbid_unknown_fields=True, tag_field='command', tag='resize'):
    cols: Annotated[int, Meta(description='Terminal columns.', ge=0, le=65535)]
    pixel_height: Annotated[
        int, Meta(description='Total pixel height.', ge=0, le=65535)
    ]
    pixel_width: Annotated[int, Meta(description='Total pixel width.', ge=0, le=65535)]
    rows: Annotated[int, Meta(description='Terminal rows.', ge=0, le=65535)]


class ControlCommand17(Struct, forbid_unknown_fields=True, tag_field='command', tag='mark'):
    label: Annotated[str, Meta(description='Non-empty marker label.')]


class ControlCommand18(Struct, forbid_unknown_fields=True, tag_field='command', tag='stop'):
    pass


class CursorStyle(StrEnum):
    bar = 'bar'
    block = 'block'
    underline = 'underline'
    hollow_block = 'hollow_block'


class DiagnosticCheck(StrEnum):
    full_redraw = 'full_redraw'
    transient = 'transient'
    dirty = 'dirty'
    cursor = 'cursor'
    modes = 'modes'
    palette = 'palette'
    query_loop = 'query_loop'


class DriverMethod1(Struct, forbid_unknown_fields=True, tag_field='method', tag='hello'):
    pass


class DriverMethod3(Struct, forbid_unknown_fields=True, tag_field='method', tag='attach'):
    session: Annotated[
        str,
        Meta(
            description='Name, `name:` reference, `path:` reference, or canonical path.'
        ),
    ]


class DriverMethod5(Struct, forbid_unknown_fields=True, tag_field='method', tag='wait_output'):
    data: Annotated[Base64Bytes, Meta(description='Exact byte sequence to find.')]
    handle: Annotated[str, Meta(description='Process-local session handle.')]
    timeout_ms: Annotated[int, Meta(description='Bounded wait duration.', ge=0)]


class DriverMethod6(Struct, forbid_unknown_fields=True, tag_field='method', tag='completion'):
    handle: Annotated[str, Meta(description='Process-local session handle.')]


class DriverMethod7(Struct, forbid_unknown_fields=True, tag_field='method', tag='complete'):
    handle: Annotated[str, Meta(description='Process-local session handle.')]
    timeout_ms: Annotated[int, Meta(description='Bounded wait duration.', ge=0)]


class DriverMethod8(Struct, forbid_unknown_fields=True, tag_field='method', tag='discard_completed'):
    handle: Annotated[str, Meta(description='Process-local owned session handle.')]


class DriverMethod11(Struct, forbid_unknown_fields=True, tag_field='method', tag='timeline'):
    all_states: Annotated[
        bool, Meta(description='Include non-presented semantic states.')
    ]
    controls: Annotated[bool, Meta(description='Include exact lossless VT frames.')]
    handle: Annotated[str, Meta(description='Process-local session handle.')]
    limit: Annotated[
        int,
        Meta(
            description='Maximum returned entries, from 1 through 4096.', ge=0, le=65535
        ),
    ]
    offset: Annotated[int, Meta(description='Zero-based entry offset.', ge=0)]


class DriverMethod13(Struct, forbid_unknown_fields=True, tag_field='method', tag='diagnose'):
    broad_change_percent: Annotated[
        int,
        Meta(description='Broad repaint threshold from 1 through 100.', ge=0, le=255),
    ]
    checks: Annotated[
        list[DiagnosticCheck], Meta(description='Empty means every detector.')
    ]
    handle: Annotated[str, Meta(description='Process-local session handle.')]


class DriverMethod14(Struct, forbid_unknown_fields=True, tag_field='method', tag='events'):
    from_event_seq: Annotated[
        int, Meta(description='Inclusive first event sequence.', ge=0)
    ]
    handle: Annotated[str, Meta(description='Process-local session handle.')]
    limit: Annotated[
        int,
        Meta(
            description='Maximum returned events, bounded by the driver.',
            ge=0,
            le=65535,
        ),
    ]
    metadata_only: Annotated[bool, Meta(description='Elide exact byte payloads.')]


class DriverMethod17(Struct, forbid_unknown_fields=True, tag_field='method', tag='bundle'):
    destination: Annotated[str, Meta(description='New no-replace bundle directory.')]
    handle: Annotated[str, Meta(description='Process-local session handle.')]
    profile: Annotated[BundleProfile, Meta(description='Explicit sensitivity profile.')]


class DriverMethod18(Struct, forbid_unknown_fields=True, tag_field='method', tag='detach'):
    handle: Annotated[str, Meta(description='Process-local session handle.')]


class DriverScreenshotRenderer(StrEnum):
    semantic = 'semantic'
    native = 'native'


class EnvironmentBase(StrEnum):
    inherit = 'inherit'
    clear = 'clear'


class EnvironmentSpec(Struct, forbid_unknown_fields=True):
    base: (
        Annotated[
            EnvironmentBase,
            Meta(description="Whether resolution begins with the recorder's ambient environment."),
        ]
        | UnsetType
    ) = EnvironmentBase.inherit
    color: (
        Annotated[
            ColorPolicy,
            Meta(description='Deterministic color-variable behavior.'),
        ]
        | UnsetType
    ) = ColorPolicy.auto
    copy: (
        Annotated[
            list[str],
            Meta(
                description='Names copied from the recorder environment without placing values in arguments.'
            ),
        ]
        | UnsetType
    ) = field(default_factory=lambda: list[str]())
    set: (
        Annotated[
            dict[str, Base64Bytes],
            Meta(description='Exact UTF-8-named values supplied by the caller.'),
        ]
        | UnsetType
    ) = field(default_factory=lambda: dict[str, Base64Bytes]())
    unset: (
        Annotated[
            list[str], Meta(description='Names removed from an inherited environment.')
        ]
        | UnsetType
    ) = field(default_factory=lambda: list[str]())


class FrameDetail(StrEnum):
    text = 'text'
    spans = 'spans'
    cells = 'cells'


class InputKind(StrEnum):
    raw = 'raw'
    text = 'text'
    keys = 'keys'
    paste = 'paste'
    focus = 'focus'
    pointer = 'pointer'


class InputSequenceAtom1(Struct, forbid_unknown_fields=True, tag_field='input', tag='bytes'):
    data: Annotated[Base64Bytes, Meta(description='Bytes encoded as Base64 in JSON.')]
    input_kind: Annotated[
        InputKind, Meta(description='Semantic category retained in canonical evidence.')
    ]
    operation_id: Annotated[
        str,
        Meta(
            description='Caller-owned operation identifier retained in canonical events.'
        ),
    ]
    caller_intent: (
        Annotated[
            str,
            Meta(description='Optional caller-supplied reason retained separately from input semantics.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class InputSequenceAtom3(Struct, forbid_unknown_fields=True, tag_field='input', tag='paste'):
    data: Annotated[
        Base64Bytes, Meta(description='Paste content before mode-dependent encoding.')
    ]
    operation_id: Annotated[
        str,
        Meta(
            description='Caller-owned operation identifier retained in canonical events.'
        ),
    ]
    caller_intent: (
        Annotated[
            str,
            Meta(description='Optional caller-supplied reason retained separately from paste semantics.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class KeyModifiers(Struct, forbid_unknown_fields=True):
    alt: Annotated[bool, Meta(description='Alt modifier.')]
    control: Annotated[bool, Meta(description='Control modifier.')]
    shift: Annotated[bool, Meta(description='Shift modifier.')]
    super_key: Annotated[bool, Meta(description='Super modifier.')]


class MouseButton(StrEnum):
    left = 'left'
    middle = 'middle'
    right = 'right'
    wheel_up = 'wheel_up'
    wheel_down = 'wheel_down'


class NamedKey1(Struct, forbid_unknown_fields=True):
    function: Annotated[int, Meta(ge=0, le=255)]


class NamedKey2(Struct, forbid_unknown_fields=True):
    letter: Annotated[str, Meta(max_length=1, min_length=1)]


NamedKey: TypeAlias = str | dict[str, int | str]


class PointerOperation1(Struct, forbid_unknown_fields=True, tag_field='kind', tag='click'):
    button: Annotated[MouseButton, Meta(description='Button to click.')]


class PointerOperation2(Struct, forbid_unknown_fields=True, tag_field='kind', tag='multi_click'):
    button: Annotated[MouseButton, Meta(description='Button to click.')]
    count: Annotated[int, Meta(description='Number of complete clicks.', ge=0, le=255)]


class PointerOperation3(Struct, forbid_unknown_fields=True, tag_field='kind', tag='drag'):
    button: Annotated[MouseButton, Meta(description='Button held throughout the drag.')]
    steps: Annotated[
        int,
        Meta(
            description='Number of motion samples, including the destination.',
            ge=0,
            le=255,
        ),
    ]
    to_column: Annotated[
        int, Meta(description='Zero-based destination column.', ge=0, le=65535)
    ]
    to_row: Annotated[
        int, Meta(description='Zero-based destination row.', ge=0, le=65535)
    ]


class PointerOperation4(Struct, forbid_unknown_fields=True, tag_field='kind', tag='scroll'):
    lines: Annotated[
        int,
        Meta(
            description='Negative scrolls up and positive scrolls down; zero is invalid.',
            ge=-32768,
            le=32767,
        ),
    ]


PointerOperation: TypeAlias = Annotated[PointerOperation1 | PointerOperation2 | PointerOperation3 | PointerOperation4, Meta(description='One high-level cell-addressed pointer operation.')]


class ProcessSignal(StrEnum):
    interrupt = 'interrupt'
    terminate = 'terminate'
    hangup = 'hangup'
    user1 = 'user1'
    user2 = 'user2'


class ResourceLimits(Struct, forbid_unknown_fields=True):
    output_bytes: (
        Annotated[
            int,
            Meta(description='Soft output budget that triggers termination after all observed bytes are retained.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET


class ScreenMode1(Struct, forbid_unknown_fields=True, tag_field='mode', tag='alternate_screen'):
    pass


class ScreenMode2(Struct, forbid_unknown_fields=True, tag_field='mode', tag='bracketed_paste'):
    pass


class ScreenMode3(Struct, forbid_unknown_fields=True, tag_field='mode', tag='wraparound'):
    pass


class ScreenMode4(Struct, forbid_unknown_fields=True, tag_field='mode', tag='synchronized_output'):
    pass


class ScreenMode5(Struct, forbid_unknown_fields=True, tag_field='mode', tag='ansi'):
    number: Annotated[int, Meta(description='Mode number.', ge=0, le=65535)]


class ScreenMode6(Struct, forbid_unknown_fields=True, tag_field='mode', tag='dec'):
    number: Annotated[int, Meta(description='Mode number.', ge=0, le=65535)]


ScreenMode: TypeAlias = Annotated[ScreenMode1 | ScreenMode2 | ScreenMode3 | ScreenMode4 | ScreenMode5 | ScreenMode6, Meta(description='Stable terminal-mode selectors available to screen predicates.')]


class ScreenPredicate1(Struct, forbid_unknown_fields=True, tag_field='predicate', tag='contains'):
    text: Annotated[str, Meta(description='Exact UTF-8 substring.')]


class ScreenPredicate2(Struct, forbid_unknown_fields=True, tag_field='predicate', tag='absent'):
    text: Annotated[str, Meta(description='Exact UTF-8 substring.')]


class ScreenPredicate3(Struct, forbid_unknown_fields=True, tag_field='predicate', tag='matches_regex'):
    pattern: Annotated[
        str, Meta(description='Rust-regex pattern, limited by the control service.')
    ]


class ScreenPredicate4(Struct, forbid_unknown_fields=True, tag_field='predicate', tag='excludes_regex'):
    pattern: Annotated[
        str, Meta(description='Rust-regex pattern, limited by the control service.')
    ]


class ScreenPredicate5(Struct, forbid_unknown_fields=True, tag_field='predicate', tag='row_contains'):
    row: Annotated[int, Meta(description='Zero-based viewport row.', ge=0, le=65535)]
    text: Annotated[str, Meta(description='Exact UTF-8 substring.')]


class ScreenPredicate6(Struct, forbid_unknown_fields=True, tag_field='predicate', tag='row_absent'):
    row: Annotated[int, Meta(description='Zero-based viewport row.', ge=0, le=65535)]
    text: Annotated[str, Meta(description='Exact UTF-8 substring.')]


class ScreenPredicate9(Struct, forbid_unknown_fields=True, tag_field='predicate', tag='cursor'):
    column: (
        Annotated[
            int,
            Meta(description='Optional zero-based viewport column.', ge=0, le=65535),
        ]
        | None
        | UnsetType
    ) = UNSET
    row: (
        Annotated[
            int,
            Meta(description='Optional zero-based viewport row.', ge=0, le=65535),
        ]
        | None
        | UnsetType
    ) = UNSET
    shape: (
        Annotated[CursorStyle, Meta(description='Optional cursor shape requirement.')]
        | None
        | UnsetType
    ) = UNSET
    visible: (
        Annotated[bool, Meta(description='Optional cursor visibility requirement.')]
        | None
        | UnsetType
    ) = UNSET


class ScreenPredicate10(Struct, forbid_unknown_fields=True, tag_field='predicate', tag='mode'):
    enabled: Annotated[bool, Meta(description='Required set/reset value.')]
    mode: Annotated[ScreenMode, Meta(description='Stable mode selector.')]


class ScreenRegion(Struct, forbid_unknown_fields=True):
    bottom: Annotated[int, Meta(description='Exclusive bottom row.', ge=0, le=65535)]
    left: Annotated[int, Meta(description='Inclusive left column.', ge=0, le=65535)]
    right: Annotated[int, Meta(description='Exclusive right column.', ge=0, le=65535)]
    top: Annotated[int, Meta(description='Inclusive top row.', ge=0, le=65535)]


class StateHashComparison(StrEnum):
    unchanged = 'unchanged'
    changed = 'changed'


class StatePoint1(Struct, forbid_unknown_fields=True, tag_field='selector', tag='latest_presented'):
    pass


class StatePoint2(Struct, forbid_unknown_fields=True, tag_field='selector', tag='state_id'):
    state_id: Annotated[int, Meta(description='Exact semantic state identifier.', ge=0)]


class StatePoint3(Struct, forbid_unknown_fields=True, tag_field='selector', tag='state_hash'):
    state_hash: Annotated[
        str, Meta(description='BLAKE3 hash of the complete normalized semantic state.')
    ]


class StatePoint4(Struct, forbid_unknown_fields=True, tag_field='selector', tag='event_seq'):
    event_seq: Annotated[int, Meta(description='Canonical event sequence.', ge=0)]


class StatePoint5(Struct, forbid_unknown_fields=True, tag_field='selector', tag='monotonic_ns'):
    monotonic_ns: Annotated[
        int, Meta(description='Session-monotonic nanoseconds.', ge=0)
    ]


class StatePoint6(Struct, forbid_unknown_fields=True, tag_field='selector', tag='marker'):
    label: Annotated[str, Meta(description='Unique marker label.')]


StatePoint: TypeAlias = Annotated[StatePoint1 | StatePoint2 | StatePoint3 | StatePoint4 | StatePoint5 | StatePoint6, Meta(description='Shared state selection vocabulary.')]


class VerificationLevel(StrEnum):
    structure = 'structure'
    content = 'content'
    durability_record = 'durability_record'
    signature = 'signature'


class ControlCommand7(Struct, forbid_unknown_fields=True, tag_field='command', tag='input'):
    data: Annotated[
        Base64Bytes, Meta(description='Raw bytes encoded as Base64 in JSON.')
    ]
    input_kind: Annotated[
        InputKind,
        Meta(description='Semantic category retained in the canonical input intent.'),
    ]
    caller_intent: (
        Annotated[
            str,
            Meta(description='Optional caller-supplied reason for the operation.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class ControlCommand10(Struct, forbid_unknown_fields=True, tag_field='command', tag='key'):
    key: Annotated[NamedKey, Meta(description='Layout-independent key.')]
    modifiers: Annotated[KeyModifiers, Meta(description='Typed modifier state.')]


class ControlCommand13(Struct, forbid_unknown_fields=True, tag_field='command', tag='pointer'):
    column: Annotated[
        int, Meta(description='Zero-based viewport column.', ge=0, le=65535)
    ]
    operation: Annotated[
        PointerOperation, Meta(description='Click or vertical scrolling plan.')
    ]
    row: Annotated[int, Meta(description='Zero-based viewport row.', ge=0, le=65535)]
    strict: Annotated[
        bool,
        Meta(
            description='Reject instead of returning a semantic no-op when reporting is disabled.'
        ),
    ]


class ControlCommand16(Struct, forbid_unknown_fields=True, tag_field='command', tag='signal'):
    signal: Annotated[ProcessSignal, Meta(description='Signal to send.')]


class DriverMethod2(Struct, forbid_unknown_fields=True, tag_field='method', tag='launch'):
    cols: Annotated[int, Meta(description='Initial terminal columns.', ge=0, le=65535)]
    command: Annotated[
        list[Base64Bytes], Meta(description='Exact executable and argument bytes.')
    ]
    evidence_root: Annotated[
        str, Meta(description='Owner-only root receiving collision-free evidence.')
    ]
    rows: Annotated[int, Meta(description='Initial terminal rows.', ge=0, le=65535)]
    environment: (
        Annotated[
            EnvironmentSpec,
            Meta(
                description='Deterministic child-environment policy; values travel only in driver stdin.'
            ),
        ]
        | UnsetType
    ) = (
        field(default_factory=lambda: convert({'base': 'inherit', 'color': 'auto', 'copy': [], 'set': {}, 'unset': []},  type=EnvironmentSpec))
    )
    resource_limits: (
        Annotated[
            ResourceLimits, Meta(description='Non-truncating runtime resource limits.')
        ]
        | UnsetType
    ) = (
        field(default_factory=lambda: convert({'output_bytes': None},  type=ResourceLimits))
    )
    working_directory: (
        Annotated[str, Meta(description='Optional child working directory.')]
        | None
        | UnsetType
    ) = UNSET


class DriverMethod9(Struct, forbid_unknown_fields=True, tag_field='method', tag='observe'):
    handle: Annotated[str, Meta(description='Process-local session handle.')]
    include_hidden: Annotated[
        bool,
        Meta(
            description='Permit explicitly selected hidden synchronized-output state.'
        ),
    ]
    point: Annotated[StatePoint, Meta(description='Exact shared state selector.')]


class DriverMethod12(Struct, forbid_unknown_fields=True, tag_field='method', tag='diff'):
    from_: Annotated[StatePoint, Meta(description='Earlier state point.')] = field(
        name='from'
    )
    handle: Annotated[str, Meta(description='Process-local session handle.')]
    to: Annotated[StatePoint, Meta(description='Later state point.')]


class DriverMethod15(Struct, forbid_unknown_fields=True, tag_field='method', tag='screenshot'):
    destination: Annotated[str, Meta(description='No-replace artifact directory.')]
    handle: Annotated[str, Meta(description='Process-local session handle.')]
    point: Annotated[StatePoint, Meta(description='Exact selected state point.')]
    renderer: Annotated[
        DriverScreenshotRenderer, Meta(description='Explicit truth renderer.')
    ]
    viewport: (
        Annotated[
            CaptureTarget,
            Meta(description='Optional strict native viewport; absent runs the solver.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class DriverMethod16(Struct, forbid_unknown_fields=True, tag_field='method', tag='verify'):
    handle: Annotated[str, Meta(description='Process-local session handle.')]
    level: Annotated[VerificationLevel, Meta(description='Requested validation depth.')]


class InputSequenceAtom2(Struct, forbid_unknown_fields=True, tag_field='input', tag='key'):
    key: Annotated[NamedKey, Meta(description='Layout-independent key.')]
    modifiers: Annotated[KeyModifiers, Meta(description='Typed modifier state.')]
    operation_id: Annotated[
        str,
        Meta(
            description='Caller-owned operation identifier retained in canonical events.'
        ),
    ]
    caller_intent: (
        Annotated[
            str,
            Meta(description='Optional caller-supplied reason retained separately from the symbolic key.'),
        ]
        | None
        | UnsetType
    ) = UNSET


InputSequenceAtom: TypeAlias = Annotated[InputSequenceAtom1 | InputSequenceAtom2 | InputSequenceAtom3, Meta(description='One input atom accepted inside an admission-guarded sequence.')]


class ScreenPredicate7(Struct, forbid_unknown_fields=True, tag_field='predicate', tag='region_contains'):
    region: Annotated[
        ScreenRegion, Meta(description='Region bounds in viewport cells.')
    ]
    text: Annotated[str, Meta(description='Exact UTF-8 substring.')]


class ScreenPredicate8(Struct, forbid_unknown_fields=True, tag_field='predicate', tag='region_absent'):
    region: Annotated[
        ScreenRegion, Meta(description='Region bounds in viewport cells.')
    ]
    text: Annotated[str, Meta(description='Exact UTF-8 substring.')]


class ScreenPredicate11(Struct, forbid_unknown_fields=True, tag_field='predicate', tag='state_hash'):
    comparison: Annotated[
        StateHashComparison, Meta(description='Equality relationship to require.')
    ]
    hash: Annotated[str, Meta(description='Lowercase BLAKE3 state hash.')]


class ScreenProjectionRequest(Struct, forbid_unknown_fields=True):
    detail: Annotated[FrameDetail, Meta(description='Requested representation.')]
    region: (
        Annotated[
            ScreenRegion,
            Meta(description='Exact absolute region, or the complete viewport for text/spans.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class ControlCommand4(Struct, forbid_unknown_fields=True, tag_field='command', tag='screen_frame'):
    projection: Annotated[
        ScreenProjectionRequest,
        Meta(description='Requested rich detail and exact optional viewport region.'),
    ]


class ControlCommand6(Struct, forbid_unknown_fields=True, tag_field='command', tag='capture_stable'):
    projection: Annotated[
        ScreenProjectionRequest,
        Meta(description='Rich projection returned from the exact terminating state.'),
    ]
    stable_for_ns: Annotated[
        int, Meta(description='Required unchanged presented-state interval.', ge=0)
    ]
    timeout_ns: Annotated[
        int, Meta(description='Server-side monotonic deadline interval.', ge=0)
    ]


class ControlCommand8(Struct, forbid_unknown_fields=True, tag_field='command', tag='input_sequence'):
    atoms: Annotated[
        list[InputSequenceAtom],
        Meta(
            description='Ordered input atoms encoded against actor state immediately before each write.'
        ),
    ]
    include_atom_outcomes: Annotated[
        bool,
        Meta(
            description='Retain per-atom identifiers and canonical event positions in the response.'
        ),
    ]
    pace_ns: Annotated[
        int,
        Meta(
            description='Delay between atoms, bounded together with the atom count.',
            ge=0,
        ),
    ]


class DriverMethod10(Struct, forbid_unknown_fields=True, tag_field='method', tag='screen_frame'):
    handle: Annotated[str, Meta(description='Process-local session handle.')]
    include_hidden: Annotated[
        bool,
        Meta(
            description='Permit explicitly selected hidden synchronized-output state.'
        ),
    ]
    point: Annotated[StatePoint, Meta(description='Exact shared state selector.')]
    projection: Annotated[
        ScreenProjectionRequest,
        Meta(description='Requested rich detail and exact optional viewport region.'),
    ]


class DriverRequest(Struct, forbid_unknown_fields=True):
    method: Annotated[DriverMethod, Meta(description='Typed driver operation.')]
    request_id: Annotated[
        str, Meta(description='Caller-owned retry/correlation identifier.')
    ]
    version: Annotated[int, Meta(description='Driver protocol version.', ge=0)]


class ActionPostcondition3(Struct, forbid_unknown_fields=True, tag_field='condition', tag='screen'):
    predicate: Annotated[ScreenPredicate, Meta(description='Exact screen predicate.')]
    timeout_ns: Annotated[
        int, Meta(description='Server-side monotonic deadline interval.', ge=0)
    ]


ActionPostcondition: TypeAlias = Annotated[ActionPostcondition1 | ActionPostcondition2 | ActionPostcondition3, Meta(description='Presented-state condition retained under exclusive actor ordering after input.')]


class ControlCommand5(Struct, forbid_unknown_fields=True, tag_field='command', tag='wait_screen'):
    predicate: Annotated[ScreenPredicate, Meta(description='Exact screen predicate.')]
    projection: Annotated[
        ScreenProjectionRequest,
        Meta(
            description='Rich projection returned from the exact satisfying or terminating state.'
        ),
    ]
    stable_for_ns: Annotated[
        int, Meta(description='Required unchanged matching-state interval.', ge=0)
    ]
    timeout_ns: Annotated[
        int, Meta(description='Server-side monotonic deadline interval.', ge=0)
    ]


class ControlCommand9(Struct, forbid_unknown_fields=True, tag_field='command', tag='act'):
    atoms: Annotated[
        list[InputSequenceAtom],
        Meta(
            description='Ordered input atoms encoded against actor state immediately before each write.'
        ),
    ]
    include_atom_outcomes: Annotated[
        bool,
        Meta(
            description='Retain per-atom identifiers and canonical event positions in the response.'
        ),
    ]
    pace_ns: Annotated[
        int,
        Meta(
            description='Delay between atoms, bounded together with the atom count.',
            ge=0,
        ),
    ]
    postcondition: Annotated[
        ActionPostcondition,
        Meta(
            description='Presented-state condition evaluated without releasing actor ordering.'
        ),
    ]
    projection: Annotated[
        ScreenProjectionRequest,
        Meta(description='Rich projection returned from the exact terminating state.'),
    ]


ControlCommand: TypeAlias = Annotated[ControlCommand1 | ControlCommand2 | ControlCommand3 | ControlCommand4 | ControlCommand5 | ControlCommand6 | ControlCommand7 | ControlCommand8 | ControlCommand9 | ControlCommand10 | ControlCommand11 | ControlCommand12 | ControlCommand13 | ControlCommand14 | ControlCommand15 | ControlCommand16 | ControlCommand17 | ControlCommand18, Meta(description='One command sent to the live session actor.')]


class DriverMethod4(Struct, forbid_unknown_fields=True, tag_field='method', tag='control'):
    command: Annotated[ControlCommand, Meta(description='Typed actor command.')]
    handle: Annotated[str, Meta(description='Process-local session handle.')]
    expected_presented_state_hash: (
        Annotated[str, Meta(description='Optional optimistic presented-state guard.')]
        | None
        | UnsetType
    ) = UNSET


DriverMethod: TypeAlias = Annotated[DriverMethod1 | DriverMethod2 | DriverMethod3 | DriverMethod4 | DriverMethod5 | DriverMethod6 | DriverMethod7 | DriverMethod8 | DriverMethod9 | DriverMethod10 | DriverMethod11 | DriverMethod12 | DriverMethod13 | DriverMethod14 | DriverMethod15 | DriverMethod16 | DriverMethod17 | DriverMethod18, Meta(description='Supported initial high-level driver methods.')]


class ScreenPredicate12(Struct, forbid_unknown_fields=True, tag_field='predicate', tag='all'):
    predicates: Annotated[
        list[ScreenPredicate], Meta(description='Bounded non-empty predicate list.')
    ]


class ScreenPredicate13(Struct, forbid_unknown_fields=True, tag_field='predicate', tag='any'):
    predicates: Annotated[
        list[ScreenPredicate], Meta(description='Bounded non-empty predicate list.')
    ]


ScreenPredicate: TypeAlias = Annotated[ScreenPredicate1 | ScreenPredicate2 | ScreenPredicate3 | ScreenPredicate4 | ScreenPredicate5 | ScreenPredicate6 | ScreenPredicate7 | ScreenPredicate8 | ScreenPredicate9 | ScreenPredicate10 | ScreenPredicate11 | ScreenPredicate12 | ScreenPredicate13, Meta(description='Exact predicate evaluated against the latest presented viewport.')]
