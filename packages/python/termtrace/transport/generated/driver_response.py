# Generated from schemas/driver-response.schema.json. DO NOT EDIT.

from __future__ import annotations

from enum import StrEnum
from typing import Annotated, Any, TypeAlias

from msgspec import UNSET, Meta, Struct, UnsetType, field


class ApplicationEventKind(StrEnum):
    event = 'event'
    span_start = 'span_start'
    span_end = 'span_end'


class ArtifactSensitivity(StrEnum):
    sensitive_terminal_data = 'sensitive_terminal_data'
    operational_metadata = 'operational_metadata'


Base64Bytes: TypeAlias = Annotated[str, Meta(pattern='^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$')]


class BundleProfile(StrEnum):
    screen = 'screen'
    debug = 'debug'
    forensic = 'forensic'


class BytePayloadSummary(Struct, forbid_unknown_fields=True):
    blake3: Annotated[str, Meta(description='BLAKE3 of the exact bytes.')]
    byte_count: Annotated[int, Meta(description='Exact byte count.', ge=0)]
    field: Annotated[
        str, Meta(description='Event field name (`data`, `query`, or `response`).')
    ]


class CanonicalBoundary(Struct, forbid_unknown_fields=True):
    event_hash: Annotated[
        str,
        Meta(
            description='Hash of the preceding event, or the event header at sequence zero.'
        ),
    ]
    next_event_seq: Annotated[
        int,
        Meta(
            description='Sequence number that will be assigned to the next event.', ge=0
        ),
    ]
    output_bytes: Annotated[
        int, Meta(description='PTY output bytes included through this boundary.', ge=0)
    ]


class CaptureQuality(StrEnum):
    live = 'live'
    stable = 'stable'
    completed = 'completed'
    incomplete = 'incomplete'


class ClockSample(Struct, forbid_unknown_fields=True):
    session_monotonic_ns: Annotated[
        int,
        Meta(
            description='Session-relative time sampled inside the system clock bracket.',
            ge=0,
        ),
    ]
    system_monotonic_ns: Annotated[
        int, Meta(description='Midpoint of the system CLOCK_MONOTONIC bracket.', ge=0)
    ]
    uncertainty_ns: Annotated[
        int,
        Meta(
            description='Maximum error of the midpoint mapping, in nanoseconds.', ge=0
        ),
    ]


class ColorSpec1(Struct, forbid_unknown_fields=True, tag_field='kind', tag='none'):
    pass


class ColorSpec2(Struct, forbid_unknown_fields=True, tag_field='kind', tag='palette'):
    value: Annotated[int, Meta(ge=0, le=255)]


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


StateId: TypeAlias = Annotated[int, Meta(ge=0)]


TokenId: TypeAlias = Annotated[int, Meta(ge=0)]


GhosttyDirtyRow: TypeAlias = Annotated[int, Meta(ge=0, le=65535)]


class DirtyState(StrEnum):
    clean = 'clean'
    partial = 'partial'
    full = 'full'


class DriverData1(Struct, forbid_unknown_fields=True, tag_field='kind', tag='hello'):
    control_protocol_version: Annotated[
        int, Meta(description='Live actor protocol version.', ge=0)
    ]
    driver_protocol_version: Annotated[
        int, Meta(description='Driver protocol version.', ge=0)
    ]
    event_log_version: Annotated[
        int, Meta(description='Canonical event format version.', ge=0)
    ]
    max_request_bytes: Annotated[
        int, Meta(description='Maximum newline-framed request bytes.', ge=0)
    ]
    max_response_bytes: Annotated[
        int, Meta(description='Maximum newline-framed response bytes.', ge=0)
    ]
    methods: Annotated[list[str], Meta(description='Stable supported methods.')]
    state_format_version: Annotated[
        int, Meta(description='Semantic state format version.', ge=0)
    ]


class DriverData2(Struct, forbid_unknown_fields=True, tag_field='kind', tag='attached'):
    canonical_session: Annotated[str, Meta(description='Canonical path.')]
    completed: Annotated[
        bool, Meta(description='Whether completion evidence already exists.')
    ]
    handle: Annotated[str, Meta(description='Process-local handle.')]
    session_id: Annotated[
        str, Meta(description='Immutable canonical session identity.')
    ]


class DriverData12(Struct, forbid_unknown_fields=True, tag_field='kind', tag='artifact'):
    artifact_type: Annotated[
        str, Meta(description='`semantic_screenshot` or `native_screenshot`.')
    ]
    directory: Annotated[
        str, Meta(description='Atomically committed artifact directory.')
    ]
    manifest: Annotated[str, Meta(description='Manifest path.')]
    primary: Annotated[str, Meta(description='Primary PNG member.')]
    truth_layer: Annotated[str, Meta(description='Exact truth-layer label.')]


class DriverData15(Struct, forbid_unknown_fields=True, tag_field='kind', tag='detached'):
    pass


class DriverData16(Struct, forbid_unknown_fields=True, tag_field='kind', tag='discarded'):
    evidence_removed: Annotated[
        bool, Meta(description='Confirms that the canonical directory was removed.')
    ]


class DurabilityMode(StrEnum):
    strict = 'strict'
    bounded = 'bounded'
    throughput = 'throughput'


class DurabilityPolicy(Struct, forbid_unknown_fields=True):
    mode: Annotated[DurabilityMode, Meta(description='Selected persistence behavior.')]
    maximum_interval_ms: (
        Annotated[
            int,
            Meta(description='Maximum uncommitted interval for bounded mode.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET
    maximum_uncommitted_bytes: (
        Annotated[
            int,
            Meta(description='Maximum uncommitted PTY bytes for bounded mode.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET


class DurabilityRecord(Struct, forbid_unknown_fields=True):
    final_sync_completed: Annotated[
        bool,
        Meta(
            description='Whether the final event boundary completed file synchronization.'
        ),
    ]
    maximum_observed_uncommitted_bytes: Annotated[
        int, Meta(description='Largest PTY byte count pending synchronization.', ge=0)
    ]
    maximum_observed_uncommitted_ns: Annotated[
        int,
        Meta(
            description='Largest measured interval with output pending synchronization.',
            ge=0,
        ),
    ]
    policy: Annotated[
        DurabilityPolicy, Meta(description='Chain-bound configured policy.')
    ]
    sync_count: Annotated[
        int,
        Meta(
            description='Number of successful event-log data synchronizations after header creation.',
            ge=0,
        ),
    ]


class Event1(Struct, forbid_unknown_fields=True, tag_field='type', tag='output'):
    data: Annotated[
        Base64Bytes,
        Meta(description='Exact bytes, including control sequences and invalid UTF-8.'),
    ]


class Event4(Struct, forbid_unknown_fields=True, tag_field='type', tag='resize_intent'):
    cols: Annotated[int, Meta(description='Columns after the resize.', ge=0, le=65535)]
    operation_id: Annotated[
        str, Meta(description='Identifier paired with the corresponding result.')
    ]
    pixel_height: Annotated[
        int,
        Meta(description='Total pixel height reported to the child.', ge=0, le=65535),
    ]
    pixel_width: Annotated[
        int,
        Meta(description='Total pixel width reported to the child.', ge=0, le=65535),
    ]
    rows: Annotated[int, Meta(description='Rows after the resize.', ge=0, le=65535)]


class Event6(Struct, forbid_unknown_fields=True, tag_field='type', tag='signal_intent'):
    name: Annotated[str, Meta(description='Stable display name such as `SIGINT`.')]
    number: Annotated[int, Meta(description='Platform signal number.')]
    operation_id: Annotated[
        str, Meta(description='Identifier paired with the corresponding result.')
    ]


class Event8(Struct, forbid_unknown_fields=True, tag_field='type', tag='marker'):
    label: Annotated[str, Meta(description='User-provided marker label.')]
    operation_id: Annotated[
        str,
        Meta(
            description='Caller-owned operation identifier used for durable deduplication.'
        ),
    ]


class Event10(Struct, forbid_unknown_fields=True, tag_field='type', tag='query_response'):
    query: Annotated[Base64Bytes, Meta(description='Query bytes emitted by the child.')]
    responder: Annotated[
        str, Meta(description='Component that generated the response.')
    ]
    response: Annotated[
        Base64Bytes, Meta(description='Response bytes sent back to the child.')
    ]
    source_byte_offset: (
        Annotated[
            int,
            Meta(description="Inclusive offset of the query's final byte in that output event.", ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET
    source_event_seq: (
        Annotated[
            int,
            Meta(description='Canonical output event containing the end of the query.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET


class Event12(Struct, forbid_unknown_fields=True, tag_field='type', tag='exit'):
    code: (
        Annotated[
            int,
            Meta(description='Exit status when the process exited normally.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    signal: (
        Annotated[
            int,
            Meta(description='Signal number when signal termination occurred.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    signal_name: (
        Annotated[str, Meta(description='Platform signal description when available.')]
        | None
        | UnsetType
    ) = UNSET


class Event13(Struct, forbid_unknown_fields=True, tag_field='type', tag='host_error'):
    code: Annotated[str, Meta(description='Stable diagnostic code.')]
    message: Annotated[str, Meta(description='Redacted human-readable message.')]


class FindingConfidence(StrEnum):
    high = 'high'
    medium = 'medium'


class FrameDetail(StrEnum):
    none = 'none'
    text = 'text'
    spans = 'spans'
    cells = 'cells'


class InputDescriptor1(Struct, forbid_unknown_fields=True, tag_field='kind', tag='raw'):
    pass


class InputDescriptor2(Struct, forbid_unknown_fields=True, tag_field='kind', tag='text'):
    pass


class InputDescriptor3(Struct, forbid_unknown_fields=True, tag_field='kind', tag='key'):
    alt: Annotated[bool, Meta(description='Alt modifier.')]
    control: Annotated[bool, Meta(description='Control modifier.')]
    encoder: Annotated[str, Meta(description='Stable encoder identity.')]
    key: Annotated[str, Meta(description='Stable symbolic key spelling.')]
    shift: Annotated[bool, Meta(description='Shift modifier.')]
    super_key: Annotated[bool, Meta(description='Super modifier.')]
    terminal_modes_hash: Annotated[
        str,
        Meta(description='BLAKE3 of canonical JSON for the terminal mode snapshot.'),
    ]


class InputDescriptor4(Struct, forbid_unknown_fields=True, tag_field='kind', tag='paste'):
    bracketed: Annotated[
        bool, Meta(description='Whether the actor applied bracketed-paste wrappers.')
    ]
    terminal_modes_hash: Annotated[
        str,
        Meta(description='BLAKE3 of canonical JSON for the terminal mode snapshot.'),
    ]


class InputDescriptor5(Struct, forbid_unknown_fields=True, tag_field='kind', tag='focus'):
    encoder: Annotated[str, Meta(description='Pinned encoder identity.')]
    focused: Annotated[
        bool,
        Meta(
            description='Whether the terminal gained (`true`) or lost (`false`) focus.'
        ),
    ]
    terminal_modes_hash: Annotated[
        str,
        Meta(description='BLAKE3 of canonical JSON for the terminal mode snapshot.'),
    ]


class InputDescriptor6(Struct, forbid_unknown_fields=True, tag_field='kind', tag='pointer'):
    button: Annotated[
        str, Meta(description='Button name for clicks, otherwise wheel direction.')
    ]
    click_count: Annotated[
        int,
        Meta(
            description='Complete click count, or zero for non-click operations.',
            ge=0,
            le=255,
        ),
    ]
    cols: Annotated[
        int, Meta(description='Terminal columns at encoding time.', ge=0, le=65535)
    ]
    column: Annotated[
        int, Meta(description='Zero-based viewport column.', ge=0, le=65535)
    ]
    encoder: Annotated[str, Meta(description='Pinned encoder identity.')]
    lines: Annotated[
        int,
        Meta(
            description='Signed requested wheel lines, or zero for clicks.',
            ge=-32768,
            le=32767,
        ),
    ]
    operation: Annotated[
        str, Meta(description='`click`, `multi_click`, `drag`, or `scroll`.')
    ]
    pixel_height: Annotated[
        int,
        Meta(description='Total terminal surface height in pixels.', ge=0, le=65535),
    ]
    pixel_width: Annotated[
        int, Meta(description='Total terminal surface width in pixels.', ge=0, le=65535)
    ]
    row: Annotated[int, Meta(description='Zero-based viewport row.', ge=0, le=65535)]
    rows: Annotated[
        int, Meta(description='Terminal rows at encoding time.', ge=0, le=65535)
    ]
    terminal_modes_hash: Annotated[
        str,
        Meta(description='BLAKE3 of canonical JSON for the terminal mode snapshot.'),
    ]
    end_column: (
        Annotated[
            int,
            Meta(description='Optional zero-based destination column for a drag.', ge=0, le=65535),
        ]
        | None
        | UnsetType
    ) = UNSET
    end_row: (
        Annotated[
            int,
            Meta(description='Optional zero-based destination row for a drag.', ge=0, le=65535),
        ]
        | None
        | UnsetType
    ) = UNSET


class InputDescriptor7(Struct, forbid_unknown_fields=True, tag_field='kind', tag='activate'):
    cols: Annotated[int, Meta(description='Current grid columns.', ge=0, le=65535)]
    encoder: Annotated[str, Meta(description='Pinned encoder identity.')]
    end_column: Annotated[
        int, Meta(description='Zero-based exclusive matched column.', ge=0, le=65535)
    ]
    presented_state_hash: Annotated[
        str, Meta(description='Presented-state hash used for resolution and encoding.')
    ]
    row: Annotated[int, Meta(description='Zero-based matched row.', ge=0, le=65535)]
    rows: Annotated[int, Meta(description='Current grid rows.', ge=0, le=65535)]
    start_column: Annotated[
        int, Meta(description='Zero-based first matched column.', ge=0, le=65535)
    ]
    target_column: Annotated[
        int, Meta(description='Cell chosen for the click.', ge=0, le=65535)
    ]
    terminal_modes_hash: Annotated[
        str,
        Meta(description='BLAKE3 of canonical JSON for the terminal mode snapshot.'),
    ]
    text: Annotated[str, Meta(description='Exact matched text.')]


InputDescriptor: TypeAlias = Annotated[InputDescriptor1 | InputDescriptor2 | InputDescriptor3 | InputDescriptor4 | InputDescriptor5 | InputDescriptor6 | InputDescriptor7, Meta(description='Structured semantic provenance for exact input bytes.')]


class InputKind(StrEnum):
    raw = 'raw'
    text = 'text'
    keys = 'keys'
    paste = 'paste'
    focus = 'focus'
    pointer = 'pointer'


class InputSequenceAtomOutcome(Struct, forbid_unknown_fields=True):
    bytes_written: Annotated[
        int, Meta(description='Exact number of PTY bytes written.', ge=0)
    ]
    input_kind: Annotated[
        InputKind, Meta(description='Semantic category retained in canonical evidence.')
    ]
    intent_event_seq: Annotated[
        int, Meta(description='Canonical input-intent event sequence.', ge=0)
    ]
    operation_id: Annotated[
        str, Meta(description='Caller-owned operation identifier for this atom.')
    ]
    result_event_seq: Annotated[
        int, Meta(description='Canonical input-result event sequence.', ge=0)
    ]


class InputSequenceFailureDetails(Struct, forbid_unknown_fields=True):
    atom_count: Annotated[
        int, Meta(description='Total number of requested atoms.', ge=0)
    ]
    atoms_applied: Annotated[
        int, Meta(description='Number of preceding atoms applied completely.', ge=0)
    ]
    bytes_written: Annotated[
        int, Meta(description='Total bytes written across all attempted atoms.', ge=0)
    ]
    failed_atom_bytes_written: Annotated[
        int,
        Meta(
            description='Bytes written for the failed atom; nonzero means application may be partial.',
            ge=0,
        ),
    ]
    failed_atom_index: Annotated[
        int, Meta(description='Zero-based atom whose write failed.', ge=0)
    ]
    failed_operation_id: Annotated[
        str, Meta(description='Caller-owned operation identifier for the failed atom.')
    ]
    first_intent_event_seq: Annotated[
        int,
        Meta(
            description='First canonical input-intent event in the attempted prefix.',
            ge=0,
        ),
    ]
    last_result_event_seq: Annotated[
        int, Meta(description='Canonical failed input-result event.', ge=0)
    ]
    resume_from_atom_index: Annotated[
        int,
        Meta(
            description='First atom safe to submit independently with a new request ID.',
            ge=0,
        ),
    ]
    sequence_id: Annotated[
        str, Meta(description='Caller-owned sequence request identifier.')
    ]


class InputSource(StrEnum):
    human = 'human'
    agent = 'agent'
    host = 'host'


class InvalidRegionReason(StrEnum):
    cells_require_explicit_region = 'cells_require_explicit_region'
    empty = 'empty'
    reversed = 'reversed'
    out_of_bounds = 'out_of_bounds'


class LifecycleKind(StrEnum):
    spawned = 'spawned'
    output_closed = 'output_closed'
    stopping = 'stopping'


class LiveEventCursor(Struct, forbid_unknown_fields=True):
    event_hash: Annotated[
        str, Meta(description='Hash of the preceding record or header.')
    ]
    file_offset: Annotated[
        int, Meta(description='Byte offset of the next record.', ge=0)
    ]
    monotonic_ns: Annotated[
        int,
        Meta(
            description='Timestamp of the preceding record, or zero at the header.',
            ge=0,
        ),
    ]
    next_event_seq: Annotated[
        int, Meta(description='Sequence of the next record.', ge=0)
    ]


class MutationJournalStatus(StrEnum):
    missing = 'missing'
    verified = 'verified'
    invalid = 'invalid'


class NativeEvidenceLevel(StrEnum):
    presentation_eligible = 'presentation_eligible'
    scheduled_for_frame = 'scheduled_for_frame'
    post_endpoint_stable_capture = 'post_endpoint_stable_capture'
    frame_observed = 'frame_observed'


class ObservationIntegrity(StrEnum):
    canonical_chained = 'canonical_chained'
    unchained = 'unchained'


class ObservationResolution(StrEnum):
    actor_snapshot = 'actor_snapshot'
    canonical_replay = 'canonical_replay'
    validated_checkpoint = 'validated_checkpoint'
    ansi_interpretation = 'ansi_interpretation'


class ObservationSourceKind(StrEnum):
    live_session = 'live_session'
    completed_session = 'completed_session'
    interrupted_session = 'interrupted_session'
    ansi_stream = 'ansi_stream'
    imported_ansi = 'imported_ansi'
    one_off_command = 'one_off_command'


class OperationStatus(StrEnum):
    applied = 'applied'
    failed = 'failed'


class OutputPage(Struct, forbid_unknown_fields=True):
    offset: Annotated[
        int,
        Meta(
            description='Effective zero-based offset, clamped to the collection length.',
            ge=0,
        ),
    ]
    returned: Annotated[
        int, Meta(description='Number of items included in this response.', ge=0)
    ]
    truncated: Annotated[
        bool, Meta(description='Whether more matching items remain after this page.')
    ]
    limit: (
        Annotated[
            int,
            Meta(description='Requested maximum, or none when every remaining item was requested.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET
    next_offset: (
        Annotated[
            int,
            Meta(description='Offset for the next page when more items remain.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET
    total: (
        Annotated[
            int,
            Meta(description='Complete matching collection length when pagination reaches closure.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET


class ResourceLimitKind(StrEnum):
    output_bytes = 'output_bytes'
    max_duration = 'max_duration'
    idle_timeout = 'idle_timeout'


class ResourceLimitOutcome(Struct, forbid_unknown_fields=True):
    kind: Annotated[ResourceLimitKind, Meta(description='Stable limit kind.')]
    limit: Annotated[int, Meta(description='Configured trigger threshold.', ge=0)]
    observed: Annotated[
        int,
        Meta(
            description='Exact observed count when the host triggered termination.',
            ge=0,
        ),
    ]


class Rgb(Struct, forbid_unknown_fields=True):
    b: Annotated[int, Meta(description='Blue channel.', ge=0, le=255)]
    g: Annotated[int, Meta(description='Green channel.', ge=0, le=255)]
    r: Annotated[int, Meta(description='Red channel.', ge=0, le=255)]


class ScreenFrameData1(Struct, forbid_unknown_fields=True, tag_field='detail', tag='none'):
    pass


class ScreenGeometry(Struct, forbid_unknown_fields=True):
    cols: Annotated[int, Meta(description='Viewport columns.', ge=0, le=65535)]
    rows: Annotated[int, Meta(description='Viewport rows.', ge=0, le=65535)]


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


class ScreenPostconditionQuality(StrEnum):
    matched = 'matched'
    matched_unsettled = 'matched_unsettled'
    child_completed = 'child_completed'
    output_closed = 'output_closed'
    deadline = 'deadline'


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


class ScreenTextRow(Struct, forbid_unknown_fields=True):
    row: Annotated[int, Meta(description='Absolute row.', ge=0, le=65535)]
    soft_wrapped: Annotated[
        bool, Meta(description='Whether this row ends in a native soft wrap.')
    ]
    text: Annotated[str, Meta(description='Deterministic convenience text.')]
    wrap_continuation: Annotated[
        bool, Meta(description='Whether this row continues a native soft wrap.')
    ]


class ScreenWaitQuality(StrEnum):
    matched = 'matched'
    matched_unsettled = 'matched_unsettled'
    child_completed = 'child_completed'
    output_closed = 'output_closed'
    deadline = 'deadline'


class SessionFailure(Struct, forbid_unknown_fields=True):
    code: Annotated[str, Meta(description='Stable failure category.')]
    message: Annotated[str, Meta(description='Human-readable failure context.')]


class StableCaptureQuality(StrEnum):
    stable = 'stable'
    child_completed = 'child_completed'
    output_closed = 'output_closed'
    deadline = 'deadline'


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


class TerminalMetadata(Struct, forbid_unknown_fields=True):
    title: Annotated[Base64Bytes, Meta(description='Raw title bytes.')]
    working_directory: Annotated[
        Base64Bytes, Meta(description='Raw working-directory bytes.')
    ]


class TerminalMode(Struct, forbid_unknown_fields=True):
    ansi: Annotated[
        bool, Meta(description='`true` for ANSI mode, `false` for DEC-private mode.')
    ]
    enabled: Annotated[bool, Meta(description='Current set/reset value.')]
    number: Annotated[int, Meta(description='Numeric mode identifier.', ge=0, le=65535)]


ChangedRow: TypeAlias = Annotated[int, Meta(ge=0, le=65535)]


class TimelineItem1(Struct, forbid_unknown_fields=True, tag_field='kind', tag='semantic_state'):
    changed_cells: Annotated[
        int, Meta(description='Number of complete semantic cell values changed.', ge=0)
    ]
    changed_rows: Annotated[
        list[ChangedRow], Meta(description='Rows whose semantic cells changed.')
    ]
    cursor_changed: Annotated[bool, Meta(description='Whether cursor state changed.')]
    ghostty_dirty: Annotated[
        DirtyState, Meta(description="Ghostty's dirty classification.")
    ]
    metadata_changed: Annotated[
        bool, Meta(description='Whether terminal metadata changed.')
    ]
    modes_changed: Annotated[bool, Meta(description='Whether terminal modes changed.')]
    presented: Annotated[
        bool, Meta(description='Whether the state was eligible for presentation.')
    ]
    state_hash: Annotated[
        str, Meta(description='Complete normalized semantic state hash.')
    ]
    state_id: Annotated[int, Meta(description='Dense semantic state identifier.', ge=0)]
    event_byte_offset: (
        Annotated[
            int,
            Meta(description='Byte offset within the responsible output event.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET


class TruthLayer(StrEnum):
    semantic = 'semantic'
    native = 'native'


AfterItem: TypeAlias = Annotated[int, Meta(ge=0, le=65535)]


BeforeItem: TypeAlias = Annotated[int, Meta(ge=0, le=65535)]


class ValueChange(Struct, forbid_unknown_fields=True):
    after: Annotated[
        tuple[AfterItem, AfterItem], Meta(description='Value at the later endpoint.')
    ]
    before: Annotated[
        tuple[BeforeItem, BeforeItem],
        Meta(description='Value at the earlier endpoint.'),
    ]


class ValueChange6(Struct, forbid_unknown_fields=True):
    after: Annotated[TerminalMetadata, Meta(description='Value at the later endpoint.')]
    before: Annotated[
        TerminalMetadata, Meta(description='Value at the earlier endpoint.')
    ]


class ValueChange7(Struct, forbid_unknown_fields=True):
    after: Annotated[bool, Meta(description='Value at the later endpoint.')]
    before: Annotated[bool, Meta(description='Value at the earlier endpoint.')]


class ValueChange8(Struct, forbid_unknown_fields=True):
    after: (
        Annotated[bool, Meta(description='Value at the later endpoint.')]
        | None
        | UnsetType
    ) = UNSET
    before: (
        Annotated[bool, Meta(description='Value at the earlier endpoint.')]
        | None
        | UnsetType
    ) = UNSET


class VerificationFinding(Struct, forbid_unknown_fields=True):
    code: Annotated[str, Meta(description='Stable machine-readable finding code.')]
    message: Annotated[
        str,
        Meta(
            description='Human-readable context that is not intended for programmatic matching.'
        ),
    ]


class VerificationLevel(StrEnum):
    structure = 'structure'
    content = 'content'
    durability_record = 'durability_record'
    signature = 'signature'


class VerificationReport(Struct, forbid_unknown_fields=True):
    artifact_authenticity_established: Annotated[
        bool,
        Meta(
            description='Whether artifact authenticity was established from a trusted checkpoint.'
        ),
    ]
    artifact_roots_verified: Annotated[
        int,
        Meta(
            description='Number of locally cataloged artifact roots verified at content level.',
            ge=0,
        ),
    ]
    findings: Annotated[
        list[VerificationFinding],
        Meta(description='Stable findings; empty when verification succeeds.'),
    ]
    format: Annotated[str, Meta(description='Stable report format identifier.')]
    level: Annotated[
        VerificationLevel, Meta(description='Requested verification depth.')
    ]
    mutation_journal_records_verified: Annotated[
        int,
        Meta(
            description='Number of mutation outcomes verified against canonical ranges.',
            ge=0,
        ),
    ]
    mutation_journal_status: Annotated[
        MutationJournalStatus,
        Meta(
            description='State of the disposable mutation deduplication acceleration journal.'
        ),
    ]
    session: Annotated[
        str, Meta(description='Canonical session directory checked by the verifier.')
    ]
    session_authenticity_established: Annotated[
        bool,
        Meta(
            description='Whether canonical session authenticity was established from a trusted receipt.'
        ),
    ]
    signature_cryptographic_valid: Annotated[
        bool,
        Meta(
            description='Whether the detached signature matches the exact receipt payload.'
        ),
    ]
    signature_receipt_present: Annotated[
        bool, Meta(description='Whether a detached evidence receipt was present.')
    ]
    signature_signer_trusted: Annotated[
        bool,
        Meta(
            description='Whether the configured trust policy accepts the signer identity/key.'
        ),
    ]
    valid: Annotated[
        bool, Meta(description='Whether every invariant at the requested level passed.')
    ]
    version: Annotated[int, Meta(description='Report schema version.', ge=0)]
    final_event_hash: (
        Annotated[str, Meta(description='Final canonical event hash, when readable.')]
        | None
        | UnsetType
    ) = UNSET
    mutation_journal_error: (
        Annotated[
            str,
            Meta(description='Journal-only diagnostic when acceleration is invalid.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    output_page: (
        Annotated[
            OutputPage,
            Meta(description='Page coordinates when CLI output intentionally contains a bounded subset.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    session_id: (
        Annotated[str, Meta(description='Immutable session identifier, when readable.')]
        | None
        | UnsetType
    ) = UNSET
    session_manifest_hash: (
        Annotated[str, Meta(description='Exact session-manifest hash, when readable.')]
        | None
        | UnsetType
    ) = UNSET
    signature_signer_key_id: (
        Annotated[
            str,
            Meta(description='Signer identity asserted by the detached receipt.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class VtSourceSegment(Struct, forbid_unknown_fields=True):
    end_byte_offset: Annotated[
        int, Meta(description='Exclusive byte offset within the event payload.', ge=0)
    ]
    event_seq: Annotated[
        int, Meta(description='Canonical output event sequence.', ge=0)
    ]
    monotonic_ns: Annotated[
        int, Meta(description='Session-monotonic event timestamp.', ge=0)
    ]
    start_byte_offset: Annotated[
        int, Meta(description='Inclusive byte offset within the event payload.', ge=0)
    ]


class VtTokenKind(StrEnum):
    printable = 'printable'
    c0 = 'c0'
    c1 = 'c1'
    esc = 'esc'
    csi = 'csi'
    osc = 'osc'
    dcs = 'dcs'
    apc = 'apc'
    unknown = 'unknown'
    incomplete = 'incomplete'


class WaitOutcome(Struct, forbid_unknown_fields=True):
    event_seq: Annotated[
        int,
        Meta(description='Event containing the end of the first observed match.', ge=0),
    ]
    monotonic_ns: Annotated[
        int, Meta(description='Monotonic timestamp of the containing event.', ge=0)
    ]
    output_end_offset: Annotated[
        int, Meta(description='End offset in the concatenated output stream.', ge=0)
    ]


class WideKind(StrEnum):
    narrow = 'narrow'
    wide = 'wide'
    spacer_tail = 'spacer_tail'
    spacer_head = 'spacer_head'


class ActionInputResult(Struct, forbid_unknown_fields=True):
    atom_count: Annotated[
        int, Meta(description='Number of atoms applied completely.', ge=0)
    ]
    bytes_written: Annotated[
        int, Meta(description='Total exact PTY bytes written across every atom.', ge=0)
    ]
    first_intent_event_seq: Annotated[
        int,
        Meta(description='First canonical input-intent event in the sequence.', ge=0),
    ]
    last_result_event_seq: Annotated[
        int,
        Meta(description='Last canonical input-result event in the sequence.', ge=0),
    ]
    pace_ns: Annotated[int, Meta(description='Requested delay between atoms.', ge=0)]
    sequence_id: Annotated[
        str, Meta(description='Canonical sequence identifier (the action request ID).')
    ]
    atoms: (
        Annotated[
            list[InputSequenceAtomOutcome],
            Meta(
                description='Optional per-atom outcomes requested by an explicit verbose caller.'
            ),
        ]
        | UnsetType
    ) = UNSET


class ActorBoundaries(Struct, forbid_unknown_fields=True):
    durable: Annotated[
        CanonicalBoundary,
        Meta(
            description='Latest boundary whose file synchronization completed successfully.'
        ),
    ]
    observed: Annotated[
        CanonicalBoundary,
        Meta(
            description='Latest boundary processed by the session actor, possibly volatile.'
        ),
    ]


class ApplicationEvent(Struct, forbid_unknown_fields=True):
    attributes: Annotated[
        dict[str, Any],
        Meta(
            description='Sensitive application data, excluded from metadata-only exports.'
        ),
    ]
    kind: Annotated[
        ApplicationEventKind, Meta(description='Event or span lifecycle transition.')
    ]
    monotonic_ns: Annotated[
        int,
        Meta(
            description='Caller-reported monotonic nanoseconds since instrumentation initialization.',
            ge=0,
        ),
    ]
    name: Annotated[
        str, Meta(description='Diagnostic name, from 1 to 256 UTF-8 bytes.')
    ]
    process_id: Annotated[
        int, Meta(description='Caller-reported operating-system PID.', ge=0)
    ]
    process_instance_id: Annotated[
        str, Meta(description='Stable UUID for the emitting process instance.')
    ]
    parent_span_id: (
        Annotated[
            str,
            Meta(description='Optional parent span in the same process instance.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    span_id: (
        Annotated[
            str,
            Meta(description='Span identity; required for span lifecycle transitions.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class ArtifactMember(Struct, forbid_unknown_fields=True):
    blake3: Annotated[str, Meta(description='BLAKE3 of the complete file.')]
    byte_length: Annotated[int, Meta(description='Exact byte length.', ge=0)]
    media_type: Annotated[str, Meta(description='Stable media type.')]
    path: Annotated[
        str, Meta(description='Normalized relative path below the artifact directory.')
    ]
    relationship: Annotated[str, Meta(description='Relationship to the derivation.')]
    sensitivity: Annotated[
        ArtifactSensitivity, Meta(description='Sensitivity classification.')
    ]
    schema_type: (
        Annotated[
            str,
            Meta(description='Public schema identifier, when the file is structured data.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class BundleManifest(Struct, forbid_unknown_fields=True):
    canonical_session: Annotated[
        Base64Bytes, Meta(description='Exact source session path bytes.')
    ]
    format: Annotated[str, Meta(description='Stable manifest format.')]
    includes_canonical_evidence: Annotated[
        bool, Meta(description='Whether canonical raw evidence is included.')
    ]
    members: Annotated[
        list[ArtifactMember],
        Meta(description='Whole-file inventory; unmanifested payloads are invalid.'),
    ]
    profile: Annotated[BundleProfile, Meta(description='Selected policy.')]
    session_id: Annotated[
        str, Meta(description='Immutable canonical session identity.')
    ]
    source_final_event_hash: Annotated[
        str, Meta(description='Final canonical event hash.')
    ]
    version: Annotated[int, Meta(description='Manifest schema version.', ge=0)]


class ColorSpec3(Struct, forbid_unknown_fields=True, tag_field='kind', tag='rgb'):
    value: Rgb


ColorSpec: TypeAlias = Annotated[ColorSpec1 | ColorSpec2 | ColorSpec3, Meta(description='Cell color source before default-color fallback.')]


class ColorState(Struct, forbid_unknown_fields=True):
    background: Annotated[Rgb, Meta(description='Default/current background.')]
    foreground: Annotated[Rgb, Meta(description='Default/current foreground.')]
    palette: Annotated[list[Rgb], Meta(description='Active 256-color palette.')]
    cursor: (
        Annotated[Rgb, Meta(description='Explicit cursor color.')] | None | UnsetType
    ) = UNSET


class ControlData1(Struct, forbid_unknown_fields=True, tag_field='kind', tag='event_cursor'):
    device: Annotated[
        int, Meta(description="Device of the writer's open file descriptor.", ge=0)
    ]
    durable: Annotated[
        LiveEventCursor, Meta(description='Latest data-synchronized position.')
    ]
    header_hash: Annotated[
        str, Meta(description='Canonical header hash retained by the writer.')
    ]
    inode: Annotated[
        int, Meta(description="Inode of the writer's open file descriptor.", ge=0)
    ]
    start: (
        Annotated[
            LiveEventCursor,
            Meta(description='Requested position, if still retained and durable.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class ControlData2(Struct, forbid_unknown_fields=True, tag_field='kind', tag='acknowledged'):
    boundaries: Annotated[
        ActorBoundaries,
        Meta(
            description='Durable and observed canonical positions after the operation.'
        ),
    ]


class ControlData3(Struct, forbid_unknown_fields=True, tag_field='kind', tag='input_applied'):
    boundaries: Annotated[
        ActorBoundaries,
        Meta(description='Durable and observed positions after application.'),
    ]
    bytes_written: Annotated[
        int, Meta(description='Number of bytes written completely.', ge=0)
    ]
    data: Annotated[Base64Bytes, Meta(description='Exact bytes emitted by the actor.')]
    input_kind: Annotated[InputKind, Meta(description='Semantic input category.')]
    intent_event_seq: Annotated[
        int, Meta(description='Canonical intent event sequence.', ge=0)
    ]
    operation_id: Annotated[
        str, Meta(description='Canonical operation identifier (the caller request ID).')
    ]
    result_event_seq: Annotated[
        int, Meta(description='Canonical result event sequence.', ge=0)
    ]


class ControlData4(Struct, forbid_unknown_fields=True, tag_field='kind', tag='input_sequence_applied'):
    atom_count: Annotated[
        int, Meta(description='Number of atoms applied completely.', ge=0)
    ]
    boundaries: Annotated[
        ActorBoundaries,
        Meta(description='Durable and observed positions after complete application.'),
    ]
    bytes_written: Annotated[
        int, Meta(description='Total exact PTY bytes written across every atom.', ge=0)
    ]
    first_intent_event_seq: Annotated[
        int,
        Meta(description='First canonical input-intent event in the sequence.', ge=0),
    ]
    last_result_event_seq: Annotated[
        int,
        Meta(description='Last canonical input-result event in the sequence.', ge=0),
    ]
    pace_ns: Annotated[int, Meta(description='Requested delay between atoms.', ge=0)]
    sequence_id: Annotated[
        str, Meta(description='Canonical sequence identifier (the caller request ID).')
    ]
    atoms: (
        Annotated[
            list[InputSequenceAtomOutcome],
            Meta(
                description='Optional per-atom outcomes requested by an explicit verbose caller.'
            ),
        ]
        | UnsetType
    ) = UNSET


class ControlData6(Struct, forbid_unknown_fields=True, tag_field='kind', tag='input_noop'):
    boundaries: Annotated[
        ActorBoundaries,
        Meta(description='Durable and observed positions at evaluation.'),
    ]
    input_kind: Annotated[InputKind, Meta(description='Semantic input category.')]
    reason: Annotated[str, Meta(description='Stable explanation for the no-op.')]
    terminal_modes_hash: Annotated[
        str,
        Meta(description='BLAKE3 of canonical JSON for the terminal mode snapshot.'),
    ]


class ControlData7(Struct, forbid_unknown_fields=True, tag_field='kind', tag='activated'):
    boundaries: Annotated[
        ActorBoundaries,
        Meta(description='Durable and observed positions after application.'),
    ]
    bytes_written: Annotated[
        int, Meta(description='Number of bytes written completely.', ge=0)
    ]
    data: Annotated[Base64Bytes, Meta(description='Exact bytes emitted by the actor.')]
    end_column: Annotated[
        int, Meta(description='Zero-based exclusive matched column.', ge=0, le=65535)
    ]
    intent_event_seq: Annotated[
        int, Meta(description='Canonical intent event sequence.', ge=0)
    ]
    operation_id: Annotated[str, Meta(description='Canonical operation identifier.')]
    presented_state_hash: Annotated[
        str, Meta(description='Presented-state hash used for resolution and encoding.')
    ]
    result_event_seq: Annotated[
        int, Meta(description='Canonical result event sequence.', ge=0)
    ]
    row: Annotated[int, Meta(description='Zero-based matched row.', ge=0, le=65535)]
    start_column: Annotated[
        int, Meta(description='Zero-based first matched column.', ge=0, le=65535)
    ]
    target_column: Annotated[
        int, Meta(description='Cell chosen for the click.', ge=0, le=65535)
    ]
    text: Annotated[str, Meta(description='Exact matched text.')]


class ControlData8(Struct, forbid_unknown_fields=True, tag_field='kind', tag='status'):
    boundaries: Annotated[
        ActorBoundaries,
        Meta(
            description='Durable and observed canonical positions at status evaluation.'
        ),
    ]
    clock_sample: Annotated[
        ClockSample,
        Meta(
            description='Bounded mapping between session-relative and system monotonic time.'
        ),
    ]
    pid: (
        Annotated[
            int,
            Meta(description='Child process identifier when available.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET


class ControlData13(Struct, forbid_unknown_fields=True, tag_field='kind', tag='marked'):
    boundaries: Annotated[
        ActorBoundaries,
        Meta(description='Durable and observed canonical positions after the marker.'),
    ]
    event_seq: Annotated[
        int, Meta(description='Canonical event sequence containing the marker.', ge=0)
    ]
    label: Annotated[str, Meta(description='Normalized marker label.')]


class CursorState(Struct, forbid_unknown_fields=True):
    blinking: Annotated[bool, Meta(description='Blink mode.')]
    in_viewport: Annotated[
        bool, Meta(description='Whether the cursor lies in the viewport.')
    ]
    style: Annotated[CursorStyle, Meta(description='Visual shape.')]
    visible: Annotated[bool, Meta(description='Visibility mode.')]
    column: (
        Annotated[
            int,
            Meta(description='Viewport column when available.', ge=0, le=65535),
        ]
        | None
        | UnsetType
    ) = UNSET
    row: (
        Annotated[int, Meta(description='Viewport row when available.', ge=0, le=65535)]
        | None
        | UnsetType
    ) = UNSET
    wide_tail: (
        Annotated[
            bool,
            Meta(description='Whether the cursor is on a wide-character tail.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class DiffEndpoint(Struct, forbid_unknown_fields=True):
    ghostty_dirty: Annotated[
        DirtyState, Meta(description='Ghostty dirty classification at this endpoint.')
    ]
    ghostty_dirty_rows: Annotated[
        list[GhosttyDirtyRow],
        Meta(description='Rows Ghostty marked dirty at this endpoint.'),
    ]
    monotonic_ns: Annotated[int, Meta(description='Session-monotonic timestamp.', ge=0)]
    point: Annotated[StatePoint, Meta(description='Requested endpoint selector.')]
    presented: Annotated[
        bool, Meta(description='Whether the state was presentation eligible.')
    ]
    state_hash: Annotated[str, Meta(description='Complete semantic state hash.')]
    state_id: Annotated[int, Meta(description='Dense semantic state identifier.', ge=0)]
    event_seq: (
        Annotated[int, Meta(description='Responsible event sequence.', ge=0)]
        | None
        | UnsetType
    ) = UNSET


class DriverData4(Struct, forbid_unknown_fields=True, tag_field='kind', tag='output_wait'):
    outcome: Annotated[
        WaitOutcome, Meta(description='First match and its canonical position.')
    ]


class DriverData13(Struct, forbid_unknown_fields=True, tag_field='kind', tag='verification'):
    report: Annotated[
        VerificationReport,
        Meta(description='Stable complete report, including invalid findings.'),
    ]


class DriverData14(Struct, forbid_unknown_fields=True, tag_field='kind', tag='bundle'):
    directory: Annotated[str, Meta(description='Atomically committed directory.')]
    manifest: Annotated[BundleManifest, Meta(description='Complete bundle manifest.')]


class Event2(Struct, forbid_unknown_fields=True, tag_field='type', tag='input_intent'):
    data: Annotated[Base64Bytes, Meta(description='Exact emitted bytes.')]
    descriptor: Annotated[
        InputDescriptor,
        Meta(description='Structured semantic source of the exact emitted bytes.'),
    ]
    input_kind: Annotated[InputKind, Meta(description='High-level input category.')]
    operation_id: Annotated[
        str, Meta(description='Identifier paired with the corresponding result.')
    ]
    source: Annotated[InputSource, Meta(description='Input origin.')]
    caller_intent: (
        Annotated[
            str,
            Meta(description='Optional caller-supplied reason for applying the input.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    semantic_input: (
        Annotated[
            str,
            Meta(description='Canonical symbolic input, such as `ctrl+u` or `paste`.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    terminal_modes_hash: (
        Annotated[
            str,
            Meta(description='BLAKE3 of canonical terminal modes used to encode this atom, when mode-aware.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class Event3(Struct, forbid_unknown_fields=True, tag_field='type', tag='input_result'):
    bytes_written: Annotated[
        int, Meta(description='Bytes written before success or failure.', ge=0)
    ]
    operation_id: Annotated[
        str, Meta(description='Identifier from the corresponding intent.')
    ]
    status: Annotated[
        OperationStatus,
        Meta(description='Whether the entire byte sequence was applied.'),
    ]
    error_code: (
        Annotated[str, Meta(description='Stable diagnostic code on failure.')]
        | None
        | UnsetType
    ) = UNSET


class Event5(Struct, forbid_unknown_fields=True, tag_field='type', tag='resize_result'):
    operation_id: Annotated[
        str, Meta(description='Identifier from the corresponding intent.')
    ]
    status: Annotated[
        OperationStatus, Meta(description='Whether the new geometry was applied.')
    ]
    error_code: (
        Annotated[str, Meta(description='Stable diagnostic code on failure.')]
        | None
        | UnsetType
    ) = UNSET


class Event7(Struct, forbid_unknown_fields=True, tag_field='type', tag='signal_result'):
    operation_id: Annotated[
        str, Meta(description='Identifier from the corresponding intent.')
    ]
    status: Annotated[
        OperationStatus, Meta(description='Whether the signal syscall succeeded.')
    ]
    error_code: (
        Annotated[str, Meta(description='Stable diagnostic code on failure.')]
        | None
        | UnsetType
    ) = UNSET


class Event9(Struct, forbid_unknown_fields=True, tag_field='type', tag='application'):
    data: Annotated[
        ApplicationEvent, Meta(description='Caller-reported diagnostic payload.')
    ]
    operation_id: Annotated[
        str,
        Meta(description='Caller-owned request identity for durable deduplication.'),
    ]


class Event11(Struct, forbid_unknown_fields=True, tag_field='type', tag='lifecycle'):
    lifecycle_kind: Annotated[LifecycleKind, Meta(description='Transition kind.')]
    pid: (
        Annotated[int, Meta(description='Child process ID when available.', ge=0)]
        | None
        | UnsetType
    ) = UNSET


Event: TypeAlias = Annotated[Event1 | Event2 | Event3 | Event4 | Event5 | Event6 | Event7 | Event8 | Event9 | Event10 | Event11 | Event12 | Event13, Meta(description='One ordered session event.')]


class EventProjection(Struct, forbid_unknown_fields=True):
    byte_payloads: Annotated[
        list[BytePayloadSummary],
        Meta(description='Hash-and-length summaries when byte fields were elided.'),
    ]
    event_type: Annotated[str, Meta(description='Stable event variant name.')]
    hash: Annotated[str, Meta(description='Canonical event hash.')]
    metadata_only: Annotated[
        bool, Meta(description='Whether exact byte fields were elided.')
    ]
    monotonic_ns: Annotated[
        int, Meta(description='Session-monotonic event timestamp.', ge=0)
    ]
    seq: Annotated[int, Meta(description='Canonical event sequence.', ge=0)]
    event: (
        Annotated[
            Event,
            Meta(description='Exact typed event when byte payloads were retained.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    metadata: (
        Annotated[
            Any,
            Meta(description='Non-byte event fields when exact payloads were elided.'),
        ]
        | UnsetType
    ) = UNSET


class ModeState(Struct, forbid_unknown_fields=True):
    alternate_screen: Annotated[bool, Meta(description='Any alternate-screen mode.')]
    bracketed_paste: Annotated[bool, Meta(description='DEC bracketed-paste mode 2004.')]
    synchronized_output: Annotated[
        bool, Meta(description='DEC synchronized-output mode 2026.')
    ]
    values: Annotated[
        list[TerminalMode],
        Meta(description='Complete stable inventory of known ANSI and DEC modes.'),
    ]
    wraparound: Annotated[bool, Meta(description='DEC auto-wrap mode 7.')]


class NativeCorrelation(Struct, forbid_unknown_fields=True):
    level: Annotated[NativeEvidenceLevel, Meta(description='Honest evidence level.')]
    limitation: Annotated[
        str, Meta(description='Exact limitation on the relationship.')
    ]
    manifest: Annotated[str, Meta(description='Relative native manifest path.')]
    state_id: Annotated[int, Meta(description='Related semantic state.', ge=0)]
    frame: (
        Annotated[
            int,
            Meta(description='Nominal encoded frame when clock mapping supplied one.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET


class ObservationFrame(Struct, forbid_unknown_fields=True):
    cols: Annotated[int, Meta(description='Terminal columns.', ge=0, le=65535)]
    cursor_visible: Annotated[bool, Meta(description='Whether the cursor is visible.')]
    modes: Annotated[ModeState, Meta(description='Stable terminal-mode facts.')]
    rows: Annotated[int, Meta(description='Terminal rows.', ge=0, le=65535)]
    viewport_text: Annotated[
        str, Meta(description='Plain text for the complete viewport.')
    ]
    cursor_column: (
        Annotated[
            int,
            Meta(description='Cursor column when it lies in the viewport.', ge=0, le=65535),
        ]
        | None
        | UnsetType
    ) = UNSET
    cursor_row: (
        Annotated[
            int,
            Meta(description='Cursor row when it lies in the viewport.', ge=0, le=65535),
        ]
        | None
        | UnsetType
    ) = UNSET


class ScreenFrameData2(Struct, forbid_unknown_fields=True, tag_field='detail', tag='text'):
    rows: Annotated[list[ScreenTextRow], Meta(description='Ordered projected rows.')]


class ScreenFrameFailureDetails(Struct, forbid_unknown_fields=True):
    cols: (
        Annotated[
            int,
            Meta(description='Full terminal columns when geometry was available.', ge=0, le=65535),
        ]
        | None
        | UnsetType
    ) = UNSET
    detail: (
        Annotated[
            FrameDetail,
            Meta(description='Requested projection detail when known.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    limit_kind: (
        Annotated[
            str,
            Meta(description='`cell_count`, `serialized_bytes`, or `response_bytes` for limit failures.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    maximum: (
        Annotated[
            int,
            Meta(description='Maximum permitted amount for a limit failure.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET
    region: (
        Annotated[ScreenRegion, Meta(description='Requested region when supplied.')]
        | None
        | UnsetType
    ) = UNSET
    region_reason: (
        Annotated[
            InvalidRegionReason,
            Meta(description='Typed invalid-region reason when validation failed.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    requested: (
        Annotated[
            int,
            Meta(description='Requested measured amount for a limit failure.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET
    rows: (
        Annotated[
            int,
            Meta(description='Full terminal rows when geometry was available.', ge=0, le=65535),
        ]
        | None
        | UnsetType
    ) = UNSET


class ScreenFrameSource1(Struct, forbid_unknown_fields=True, tag_field='source', tag='actor'):
    boundaries: Annotated[
        ActorBoundaries,
        Meta(description='Durable and actor-observed canonical boundaries.'),
    ]
    observed_monotonic_ns: Annotated[
        int,
        Meta(
            description='Actor observation time in session-monotonic nanoseconds.', ge=0
        ),
    ]
    session_id: Annotated[str, Meta(description='Canonical session identity.')]


class ScreenFrameSource2(Struct, forbid_unknown_fields=True, tag_field='source', tag='resolved'):
    boundary: Annotated[
        CanonicalBoundary, Meta(description='Exact validated source boundary.')
    ]
    integrity: Annotated[
        ObservationIntegrity,
        Meta(description='Integrity relationship to canonical evidence.'),
    ]
    resolution: Annotated[
        ObservationResolution,
        Meta(description='Resolution path used to select the state.'),
    ]
    source_completed: Annotated[
        bool, Meta(description='Whether canonical evidence completed durably.')
    ]
    source_kind: Annotated[ObservationSourceKind, Meta(description='Source category.')]
    state_point: Annotated[StatePoint, Meta(description='Requested state selector.')]
    event_seq: (
        Annotated[
            int,
            Meta(description='Responsible canonical event sequence when known.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET
    monotonic_ns: (
        Annotated[
            int,
            Meta(description='Session-monotonic timestamp when known.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET
    session_id: (
        Annotated[str, Meta(description='Canonical session identity when available.')]
        | None
        | UnsetType
    ) = UNSET
    state_id: (
        Annotated[
            int,
            Meta(description='Dense semantic state identity when assigned.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET


ScreenFrameSource: TypeAlias = Annotated[ScreenFrameSource1 | ScreenFrameSource2, Meta(description='Exact provenance of the projected semantic state.')]


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


class SessionCompletion(Struct, forbid_unknown_fields=True):
    durability: Annotated[
        DurabilityRecord,
        Meta(
            description='Recorder-observed persistence behavior and final sync result.'
        ),
    ]
    event_count: Annotated[int, Meta(description='Number of canonical events.', ge=0)]
    event_header_hash: Annotated[
        str, Meta(description='BLAKE3 of the exact event-log header bytes.')
    ]
    final_event_hash: Annotated[
        str, Meta(description='Hash of the final canonical event.')
    ]
    format: Annotated[str, Meta(description='Stable format identifier.')]
    output_bytes: Annotated[int, Meta(description='Total raw output bytes.', ge=0)]
    session_id: Annotated[str, Meta(description='Session identifier.')]
    session_manifest_hash: Annotated[
        str, Meta(description='BLAKE3 of the exact immutable session manifest bytes.')
    ]
    terminal_event_seq: Annotated[
        int,
        Meta(
            description='Sequence number of the required terminal `Exit` event.', ge=0
        ),
    ]
    validation_version: Annotated[
        int, Meta(description='Validation contract applied before publication.', ge=0)
    ]
    version: Annotated[int, Meta(description='Completion schema version.', ge=0)]
    exit_code: (
        Annotated[int, Meta(description='Normal exit code when available.')]
        | None
        | UnsetType
    ) = UNSET
    failure: (
        Annotated[
            SessionFailure,
            Meta(description='Fatal recorder failure, absent for normal child completion.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    resource_limit: (
        Annotated[
            ResourceLimitOutcome,
            Meta(description='Resource limit that caused host termination, when one was reached.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    signal_name: (
        Annotated[str, Meta(description='Signal name when available.')]
        | None
        | UnsetType
    ) = UNSET


class SpanCell(Struct, forbid_unknown_fields=True):
    grapheme: Annotated[str, Meta(description='Complete grapheme cluster.')]
    wide_kind: Annotated[WideKind, Meta(description='Native Ghostty width role.')]


class TimelineItem2(Struct, forbid_unknown_fields=True, tag_field='kind', tag='canonical_event'):
    event: Annotated[Event, Meta(description='Exact typed canonical event.')]


class ValueChange2(Struct, forbid_unknown_fields=True):
    after: Annotated[CursorState, Meta(description='Value at the later endpoint.')]
    before: Annotated[CursorState, Meta(description='Value at the earlier endpoint.')]


class ValueChange3(Struct, forbid_unknown_fields=True):
    after: Annotated[ModeState, Meta(description='Value at the later endpoint.')]
    before: Annotated[ModeState, Meta(description='Value at the earlier endpoint.')]


class ValueChange4(Struct, forbid_unknown_fields=True):
    after: Annotated[ColorState, Meta(description='Value at the later endpoint.')]
    before: Annotated[ColorState, Meta(description='Value at the earlier endpoint.')]


class VtToken(Struct, forbid_unknown_fields=True):
    description: Annotated[
        str, Meta(description='Stable neutral description for common controls.')
    ]
    kind: Annotated[VtTokenKind, Meta(description='Framed sequence family.')]
    raw: Annotated[
        Base64Bytes,
        Meta(description='Exact bytes, including introducer and terminator.'),
    ]
    source: Annotated[
        list[VtSourceSegment],
        Meta(
            description='Ordered exact source segments; concatenating them reproduces `raw`.'
        ),
    ]
    timing_resolution: Annotated[
        str,
        Meta(
            description='Timing resolution; current recordings timestamp complete PTY reads.'
        ),
    ]
    token_id: Annotated[int, Meta(description='Dense token identifier.', ge=0)]


class CellStyle(Struct, forbid_unknown_fields=True):
    background: Annotated[ColorSpec, Meta(description='Background color source.')]
    blink: Annotated[bool, Meta(description='Blink attribute.')]
    bold: Annotated[bool, Meta(description='Bold attribute.')]
    faint: Annotated[bool, Meta(description='Faint attribute.')]
    foreground: Annotated[ColorSpec, Meta(description='Foreground color source.')]
    inverse: Annotated[bool, Meta(description='Inverse-video attribute.')]
    invisible: Annotated[bool, Meta(description='Invisible attribute.')]
    italic: Annotated[bool, Meta(description='Italic attribute.')]
    overline: Annotated[bool, Meta(description='Overline attribute.')]
    strikethrough: Annotated[bool, Meta(description='Strikethrough attribute.')]
    underline: Annotated[
        int, Meta(description='Upstream SGR underline value (`0` through `5`).')
    ]
    underline_color: Annotated[ColorSpec, Meta(description='Underline color source.')]


class ControlData9(Struct, forbid_unknown_fields=True, tag_field='kind', tag='snapshot'):
    boundaries: Annotated[
        ActorBoundaries,
        Meta(description='Durable and observed source boundaries at observation time.'),
    ]
    cols: Annotated[int, Meta(description='Terminal columns.', ge=0, le=65535)]
    cursor_visible: Annotated[bool, Meta(description='Whether the cursor is visible.')]
    modes: Annotated[
        ModeState, Meta(description='Presentation-eligible terminal modes.')
    ]
    observed_monotonic_ns: Annotated[
        int, Meta(description='Session-monotonic observation timestamp.', ge=0)
    ]
    presented: Annotated[
        bool, Meta(description='Always true for this presentation-filtered endpoint.')
    ]
    rows: Annotated[int, Meta(description='Terminal rows.', ge=0, le=65535)]
    semantic_state_zstd: Annotated[
        Base64Bytes,
        Meta(
            description='Zstandard-compressed canonical JSON for the complete semantic state.'
        ),
    ]
    state_hash: Annotated[
        str,
        Meta(description='Normalized semantic hash assigned by deterministic replay.'),
    ]
    viewport_text: Annotated[
        str, Meta(description='Plain text of the presentation-eligible viewport.')
    ]
    cursor_column: (
        Annotated[
            int,
            Meta(description='Cursor column when it lies in the viewport.', ge=0, le=65535),
        ]
        | None
        | UnsetType
    ) = UNSET
    cursor_row: (
        Annotated[
            int,
            Meta(description='Cursor row when it lies in the viewport.', ge=0, le=65535),
        ]
        | None
        | UnsetType
    ) = UNSET


class ControlErrorDetails(Struct, forbid_unknown_fields=True):
    boundaries: Annotated[
        ActorBoundaries,
        Meta(
            description='Durable and observed source positions at the failure boundary.'
        ),
    ]
    current_presented_state_hash: Annotated[
        str,
        Meta(description='Latest presented-state hash at the actor ordering point.'),
    ]
    scope: Annotated[
        str,
        Meta(description='Exact output or mutation scope covered by these details.'),
    ]
    input_sequence: (
        Annotated[
            InputSequenceFailureDetails,
            Meta(description='Exact attempted prefix when an admitted input sequence stopped early.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    screen_frame: (
        Annotated[
            ScreenFrameFailureDetails,
            Meta(description='Bounded rich-projection validation or resource-limit facts.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class DiagnosticFinding(Struct, forbid_unknown_fields=True):
    code: Annotated[str, Meta(description='Stable machine-readable finding code.')]
    confidence: Annotated[
        FindingConfidence, Meta(description='Conservative confidence label.')
    ]
    facts: Annotated[
        list[str], Meta(description='Exact neutral facts and limitations.')
    ]
    native_correlations: Annotated[
        list[NativeCorrelation],
        Meta(description='Native relationships, if independently available.'),
    ]
    state_ids: Annotated[
        list[StateId], Meta(description='Supporting semantic state IDs.')
    ]
    token_ids: Annotated[list[TokenId], Meta(description='Supporting VT token IDs.')]


class DriverData5(Struct, forbid_unknown_fields=True, tag_field='kind', tag='completion'):
    completion: Annotated[
        SessionCompletion, Meta(description='Validated completion envelope.')
    ]


class DriverData11(Struct, forbid_unknown_fields=True, tag_field='kind', tag='events'):
    events: Annotated[
        list[EventProjection], Meta(description='Ordered event projections.')
    ]
    next_event_seq: Annotated[
        int, Meta(description='Next event sequence when more source may exist.', ge=0)
    ]


class DriverResult2(Struct, forbid_unknown_fields=True, tag_field='status', tag='error'):
    code: Annotated[str, Meta(description='Machine-readable code.')]
    message: Annotated[str, Meta(description='Human-readable diagnostic.')]
    retry_safety: Annotated[
        str,
        Meta(description='`safe_same_request_id`, `safe_new_request`, or `unknown`.'),
    ]
    uncertain: Annotated[
        bool, Meta(description='Whether mutation application may be uncertain.')
    ]
    details: (
        Annotated[
            ControlErrorDetails,
            Meta(description='Actor-provided ordering or attempted-prefix details when available.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    evidence: (
        Annotated[str, Meta(description='Canonical evidence path when known.')]
        | None
        | UnsetType
    ) = UNSET


class Observation(Struct, forbid_unknown_fields=True):
    boundary: Annotated[
        CanonicalBoundary, Meta(description='Source boundary observed or validated.')
    ]
    capture_quality: Annotated[CaptureQuality, Meta(description='Settlement quality.')]
    frame: Annotated[ObservationFrame, Meta(description='Compact stable frame.')]
    integrity: Annotated[
        ObservationIntegrity,
        Meta(description='Integrity relationship to canonical execution evidence.'),
    ]
    payload_hash: Annotated[
        str, Meta(description='BLAKE3 of canonical JSON for `frame`.')
    ]
    presented: Annotated[
        bool, Meta(description='Whether this state was presentation eligible.')
    ]
    resolution: Annotated[
        ObservationResolution,
        Meta(description='Service path used to resolve this state.'),
    ]
    source_completed: Annotated[
        bool,
        Meta(description='Whether canonical execution evidence was durably completed.'),
    ]
    source_kind: Annotated[ObservationSourceKind, Meta(description='Source category.')]
    state_hash: Annotated[
        str, Meta(description='BLAKE3 of the complete semantic state.')
    ]
    state_point: Annotated[StatePoint, Meta(description='Requested state point.')]
    truth_layer: Annotated[TruthLayer, Meta(description='Rendering truth layer.')]
    canonical_session: (
        Annotated[
            Base64Bytes,
            Meta(description='Exact canonical session path bytes when evidence exists.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    event_seq: (
        Annotated[
            int,
            Meta(description='Responsible canonical event sequence when known.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET
    monotonic_ns: (
        Annotated[
            int,
            Meta(description='Session-monotonic timestamp when known.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET
    session_id: (
        Annotated[str, Meta(description='Canonical session ID when evidence exists.')]
        | None
        | UnsetType
    ) = UNSET
    state_id: (
        Annotated[
            int,
            Meta(description='Semantic state ID when replay assigned one.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET


class ScreenAppearance(Struct, forbid_unknown_fields=True):
    selected: Annotated[bool, Meta(description='Selection membership.')]
    style: Annotated[
        CellStyle, Meta(description='Authoritative source style and color selectors.')
    ]
    resolved_background: (
        Annotated[
            Rgb,
            Meta(description='Resolved explicit background before default fallback.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    resolved_foreground: (
        Annotated[
            Rgb,
            Meta(description='Resolved explicit foreground before default fallback.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class ScreenCell(Struct, forbid_unknown_fields=True):
    appearance: Annotated[
        ScreenAppearance, Meta(description='Complete semantic appearance.')
    ]
    column: Annotated[
        int, Meta(description='Absolute viewport column.', ge=0, le=65535)
    ]
    grapheme: Annotated[
        str, Meta(description='Complete grapheme cluster, empty for a blank/spacer.')
    ]
    row: Annotated[int, Meta(description='Absolute viewport row.', ge=0, le=65535)]
    wide_kind: Annotated[WideKind, Meta(description='Native Ghostty width role.')]


class ScreenCellRow(Struct, forbid_unknown_fields=True):
    cells: Annotated[list[ScreenCell], Meta(description='Exact ordered cells.')]
    row: Annotated[int, Meta(description='Absolute row.', ge=0, le=65535)]
    soft_wrapped: Annotated[
        bool, Meta(description='Whether this row ends in a native soft wrap.')
    ]
    text: Annotated[str, Meta(description='Deterministic convenience text.')]
    wrap_continuation: Annotated[
        bool, Meta(description='Whether this row continues a native soft wrap.')
    ]


class ScreenFrameData4(Struct, forbid_unknown_fields=True, tag_field='detail', tag='cells'):
    rows: Annotated[list[ScreenCellRow], Meta(description='Ordered projected rows.')]


class ScreenSpan(Struct, forbid_unknown_fields=True):
    appearance: Annotated[
        ScreenAppearance, Meta(description='Appearance common to every covered cell.')
    ]
    cell_count: Annotated[
        int, Meta(description='Number of terminal columns covered.', ge=0, le=65535)
    ]
    cells: Annotated[
        list[SpanCell], Meta(description='Exact grapheme/width map in column order.')
    ]
    end_column: Annotated[
        int, Meta(description='Exclusive absolute end column.', ge=0, le=65535)
    ]
    start_column: Annotated[
        int, Meta(description='Inclusive absolute start column.', ge=0, le=65535)
    ]
    text: Annotated[
        str, Meta(description='Deterministic convenience text for this run.')
    ]


class ScreenSpanRow(Struct, forbid_unknown_fields=True):
    row: Annotated[int, Meta(description='Absolute row.', ge=0, le=65535)]
    soft_wrapped: Annotated[
        bool, Meta(description='Whether this row ends in a native soft wrap.')
    ]
    spans: Annotated[
        list[ScreenSpan], Meta(description='Lossless ordered appearance spans.')
    ]
    text: Annotated[str, Meta(description='Deterministic convenience text.')]
    wrap_continuation: Annotated[
        bool, Meta(description='Whether this row continues a native soft wrap.')
    ]


class TimelineItem3(Struct, forbid_unknown_fields=True, tag_field='kind', tag='vt_token'):
    token: Annotated[
        VtToken, Meta(description='Exact token and canonical source segments.')
    ]


TimelineItem: TypeAlias = Annotated[TimelineItem1 | TimelineItem2 | TimelineItem3, Meta(description='Concise canonical or semantic item in a diagnostic timeline.')]


class ValueChange5(Struct, forbid_unknown_fields=True):
    after: Annotated[CellStyle, Meta(description='Value at the later endpoint.')]
    before: Annotated[CellStyle, Meta(description='Value at the earlier endpoint.')]


class CellState(Struct, forbid_unknown_fields=True):
    column: Annotated[int, Meta(description='Zero-based column.', ge=0, le=65535)]
    grapheme: Annotated[
        str, Meta(description='Full UTF-8 grapheme cluster; empty for a blank cell.')
    ]
    selected: Annotated[bool, Meta(description='Whether the cell is selected.')]
    style: Annotated[CellStyle, Meta(description='Visual style.')]
    wide_kind: Annotated[
        WideKind,
        Meta(description='Native width role; never inferred from the grapheme.'),
    ]
    resolved_background: (
        Annotated[
            Rgb,
            Meta(description='Resolved explicit background, before terminal-default fallback.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    resolved_foreground: (
        Annotated[
            Rgb,
            Meta(description='Resolved explicit foreground, before terminal-default fallback.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class DiagnoseReport(Struct, forbid_unknown_fields=True):
    broad_change_fraction: Annotated[
        str,
        Meta(
            description='Minimum changed-cell fraction used for broad redraw findings.'
        ),
    ]
    checks: Annotated[
        list[DiagnosticCheck], Meta(description='Ordered detector selection.')
    ]
    findings: Annotated[
        list[DiagnosticFinding], Meta(description='Deterministic findings.')
    ]
    format: Annotated[str, Meta(description='Stable report format.')]
    session_id: Annotated[str, Meta(description='Canonical session identity.')]
    source_event_count: Annotated[
        int, Meta(description='Canonical event count anchoring the report.', ge=0)
    ]
    source_final_event_hash: Annotated[
        str, Meta(description='Final canonical event hash anchoring the report.')
    ]
    version: Annotated[int, Meta(description='Report schema version.', ge=0)]
    output_page: (
        Annotated[
            OutputPage,
            Meta(description='Page coordinates when CLI output intentionally contains a bounded subset.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class DriverData6(Struct, forbid_unknown_fields=True, tag_field='kind', tag='observation'):
    observation: Annotated[
        Observation, Meta(description='Shared observation envelope.')
    ]


class DriverData10(Struct, forbid_unknown_fields=True, tag_field='kind', tag='diagnose'):
    report: Annotated[DiagnoseReport, Meta(description='Source-bound findings.')]


class ScreenFrameData3(Struct, forbid_unknown_fields=True, tag_field='detail', tag='spans'):
    rows: Annotated[list[ScreenSpanRow], Meta(description='Ordered projected rows.')]


ScreenFrameData: TypeAlias = Annotated[ScreenFrameData1 | ScreenFrameData2 | ScreenFrameData3 | ScreenFrameData4, Meta(description='Structurally discriminated rich row payload.')]


class TimelineEntry(Struct, forbid_unknown_fields=True):
    item: Annotated[TimelineItem, Meta(description='Timeline item payload.')]
    monotonic_ns: Annotated[int, Meta(description='Canonical session offset.', ge=0)]
    event_seq: (
        Annotated[
            int,
            Meta(description='Responsible canonical event, absent for the initial semantic state.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET


class CellChange(Struct, forbid_unknown_fields=True):
    column: Annotated[int, Meta(description='Zero-based column.', ge=0, le=65535)]
    after: (
        Annotated[
            CellState,
            Meta(description='Complete later cell, absent outside later geometry.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    before: (
        Annotated[
            CellState,
            Meta(description='Complete earlier cell, absent outside earlier geometry.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class DriverData8(Struct, forbid_unknown_fields=True, tag_field='kind', tag='timeline'):
    entries: Annotated[
        list[TimelineEntry], Meta(description='Requested entries in canonical order.')
    ]
    next_offset: (
        Annotated[
            int,
            Meta(description='Next zero-based entry offset, absent at the end.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET


class RowChange(Struct, forbid_unknown_fields=True):
    cells: Annotated[
        list[CellChange], Meta(description='Complete semantic cell changes.')
    ]
    row: Annotated[int, Meta(description='Zero-based row.', ge=0, le=65535)]
    soft_wrapped: (
        Annotated[
            ValueChange8,
            Meta(description='Earlier/later soft-wrap fact, including row absence at either endpoint.'),
        ]
        | None
        | UnsetType
    ) = UNSET
    wrap_continuation: (
        Annotated[
            ValueChange8,
            Meta(description='Earlier/later wrap-continuation fact, including row absence at either endpoint.'),
        ]
        | None
        | UnsetType
    ) = UNSET


class ScreenFrame(Struct, forbid_unknown_fields=True):
    capture_quality: Annotated[
        CaptureQuality, Meta(description='Settlement/provenance label.')
    ]
    colors: Annotated[
        ColorState, Meta(description='Current/default colors and complete palette.')
    ]
    cursor: Annotated[CursorState, Meta(description='Complete cursor state.')]
    data: Annotated[
        ScreenFrameData, Meta(description='Structurally matched detail payload.')
    ]
    frame_hash: Annotated[
        str,
        Meta(
            description='Projection identity in the `termtrace-screen-frame-v1` hash domain.'
        ),
    ]
    geometry: Annotated[ScreenGeometry, Meta(description='Full viewport geometry.')]
    modes: Annotated[ModeState, Meta(description='Complete relevant terminal modes.')]
    presented: Annotated[
        bool, Meta(description='Whether this state was presentation eligible.')
    ]
    requested_region: Annotated[
        ScreenRegion, Meta(description='Validated requested region.')
    ]
    returned_region: Annotated[
        ScreenRegion,
        Meta(description='Actually returned region; equal to requested in version 1.'),
    ]
    source: Annotated[ScreenFrameSource, Meta(description='Exact source occurrence.')]
    state_hash: Annotated[str, Meta(description='Complete semantic v5 state identity.')]
    version: Annotated[int, Meta(description='Projection format version.', ge=0)]


class SemanticDiff(Struct, forbid_unknown_fields=True):
    changed: Annotated[
        bool,
        Meta(
            description='Whether any normalized semantic or presentation value changed.'
        ),
    ]
    from_: Annotated[DiffEndpoint, Meta(description='Earlier endpoint identity and provenance.')] = field(
        name='from'
    )
    rows: Annotated[
        list[RowChange],
        Meta(description='Minimal changed semantic cells grouped by row.'),
    ]
    to: Annotated[
        DiffEndpoint, Meta(description='Later endpoint identity and provenance.')
    ]
    active_style: (
        Annotated[ValueChange5, Meta(description='Active SGR style change.')]
        | None
        | UnsetType
    ) = UNSET
    colors: (
        Annotated[ValueChange4, Meta(description='Palette/default-color change.')]
        | None
        | UnsetType
    ) = UNSET
    cursor: (
        Annotated[ValueChange2, Meta(description='Cursor change.')] | None | UnsetType
    ) = UNSET
    geometry: (
        Annotated[ValueChange, Meta(description='Geometry change.')] | None | UnsetType
    ) = UNSET
    metadata: (
        Annotated[ValueChange6, Meta(description='OSC metadata change.')]
        | None
        | UnsetType
    ) = UNSET
    modes: (
        Annotated[ValueChange3, Meta(description='Terminal mode change.')]
        | None
        | UnsetType
    ) = UNSET
    presented: (
        Annotated[ValueChange7, Meta(description='Presentation-eligibility change.')]
        | None
        | UnsetType
    ) = UNSET


class ActionPostconditionResult1(Struct, forbid_unknown_fields=True, tag_field='kind', tag='none'):
    matched: Annotated[
        bool, Meta(description='Always true after complete input application.')
    ]
    frame: (
        Annotated[ScreenFrame, Meta(description='Exact immediate post-input frame.')]
        | None
        | UnsetType
    ) = UNSET
    state_hash: (
        Annotated[str, Meta(description='Exact terminating state hash when known.')]
        | None
        | UnsetType
    ) = UNSET


class ActionPostconditionResult2(Struct, forbid_unknown_fields=True, tag_field='kind', tag='idle'):
    matched: Annotated[
        bool,
        Meta(
            description='Whether the state remained unchanged for the requested interval.'
        ),
    ]
    quality: Annotated[
        StableCaptureQuality, Meta(description='Typed stability quality.')
    ]
    frame: (
        Annotated[ScreenFrame, Meta(description='Exact terminating frame.')]
        | None
        | UnsetType
    ) = UNSET
    state_hash: (
        Annotated[str, Meta(description='Exact terminating state hash when known.')]
        | None
        | UnsetType
    ) = UNSET


class ActionPostconditionResult3(Struct, forbid_unknown_fields=True, tag_field='kind', tag='screen'):
    matched: Annotated[
        bool, Meta(description='Whether the predicate matched before termination.')
    ]
    quality: Annotated[
        ScreenPostconditionQuality,
        Meta(description='Closed completion classification.'),
    ]
    frame: (
        Annotated[ScreenFrame, Meta(description='Exact terminating frame.')]
        | None
        | UnsetType
    ) = UNSET
    state_hash: (
        Annotated[str, Meta(description='Exact terminating state hash when known.')]
        | None
        | UnsetType
    ) = UNSET


ActionPostconditionResult: TypeAlias = Annotated[ActionPostconditionResult1 | ActionPostconditionResult2 | ActionPostconditionResult3, Meta(description='Exact postcondition result returned without releasing actor ordering.')]


class ControlData5(Struct, forbid_unknown_fields=True, tag_field='kind', tag='action_applied'):
    boundaries: Annotated[
        ActorBoundaries,
        Meta(
            description='Durable and observed positions after postcondition evaluation.'
        ),
    ]
    input: Annotated[
        ActionInputResult,
        Meta(description='Exact result of the completely applied input prefix.'),
    ]
    postcondition: Annotated[
        ActionPostconditionResult,
        Meta(
            description='Exact presented state that satisfied or ended the postcondition.'
        ),
    ]


class ControlData10(Struct, forbid_unknown_fields=True, tag_field='kind', tag='screen_frame'):
    frame: Annotated[
        ScreenFrame,
        Meta(
            description='Exact owned projection produced before actor ordering was released.'
        ),
    ]


class ControlData11(Struct, forbid_unknown_fields=True, tag_field='kind', tag='stable_snapshot'):
    first_observed_monotonic_ns: Annotated[
        int,
        Meta(
            description='First actor timestamp at which this exact state was observed.',
            ge=0,
        ),
    ]
    frame: Annotated[
        ScreenFrame,
        Meta(description='Exact frame projected before waiter ordering was released.'),
    ]
    observed_monotonic_ns: Annotated[
        int, Meta(description='Actor timestamp of the returned snapshot.', ge=0)
    ]
    quality: Annotated[
        StableCaptureQuality, Meta(description='Typed reason the operation returned.')
    ]
    semantic_state_zstd: Annotated[
        Base64Bytes,
        Meta(
            description='Zstandard-compressed canonical JSON for this exact semantic state.'
        ),
    ]
    stable_for_ns: Annotated[
        int, Meta(description='Requested unchanged interval.', ge=0)
    ]


class DriverData7(Struct, forbid_unknown_fields=True, tag_field='kind', tag='screen_frame'):
    frame: Annotated[ScreenFrame, Meta(description='Exact owned rich projection.')]


class DriverData9(Struct, forbid_unknown_fields=True, tag_field='kind', tag='diff'):
    difference: Annotated[
        SemanticDiff, Meta(description='Source-bound semantic difference.')
    ]


class DriverResponse(Struct, forbid_unknown_fields=True):
    request_id: Annotated[str, Meta(description='Identifier copied from the request.')]
    result: Annotated[DriverResult, Meta(description='Typed success or stable error.')]
    version: Annotated[int, Meta(description='Driver protocol version.', ge=0)]


class ControlData12(Struct, forbid_unknown_fields=True, tag_field='kind', tag='screen_wait'):
    frame: Annotated[
        ScreenFrame, Meta(description='Exact satisfying or terminating frame.')
    ]
    matched: Annotated[
        bool, Meta(description='Whether the predicate matched before termination.')
    ]
    observed_monotonic_ns: Annotated[
        int,
        Meta(
            description='Last actor timestamp at which the returned state was evaluated.',
            ge=0,
        ),
    ]
    predicate: Annotated[
        ScreenPredicate, Meta(description='Predicate that was evaluated.')
    ]
    quality: Annotated[
        ScreenWaitQuality, Meta(description='Closed completion classification.')
    ]
    stable_for_ns: Annotated[
        int, Meta(description='Requested unchanged interval.', ge=0)
    ]
    first_match_monotonic_ns: (
        Annotated[
            int,
            Meta(description='First matching timestamp for this unchanged state, when matched.', ge=0),
        ]
        | None
        | UnsetType
    ) = UNSET


ControlData: TypeAlias = Annotated[ControlData1 | ControlData2 | ControlData3 | ControlData4 | ControlData5 | ControlData6 | ControlData7 | ControlData8 | ControlData9 | ControlData10 | ControlData11 | ControlData12 | ControlData13, Meta(description='Successful response payload.')]


class DriverData3(Struct, forbid_unknown_fields=True, tag_field='kind', tag='control'):
    data: Annotated[ControlData, Meta(description='Exact actor result data.')]


DriverData: TypeAlias = Annotated[DriverData1 | DriverData2 | DriverData3 | DriverData4 | DriverData5 | DriverData6 | DriverData7 | DriverData8 | DriverData9 | DriverData10 | DriverData11 | DriverData12 | DriverData13 | DriverData14 | DriverData15 | DriverData16, Meta(description='Typed driver result payload.')]


class DriverResult1(Struct, forbid_unknown_fields=True, tag_field='status', tag='ok'):
    data: Annotated[DriverData, Meta(description='Method-specific data.')]


DriverResult: TypeAlias = Annotated[DriverResult1 | DriverResult2, Meta(description='Driver success or failure envelope.')]


class ScreenPredicate12(Struct, forbid_unknown_fields=True, tag_field='predicate', tag='all'):
    predicates: Annotated[
        list[ScreenPredicate], Meta(description='Bounded non-empty predicate list.')
    ]


class ScreenPredicate13(Struct, forbid_unknown_fields=True, tag_field='predicate', tag='any'):
    predicates: Annotated[
        list[ScreenPredicate], Meta(description='Bounded non-empty predicate list.')
    ]


ScreenPredicate: TypeAlias = Annotated[ScreenPredicate1 | ScreenPredicate2 | ScreenPredicate3 | ScreenPredicate4 | ScreenPredicate5 | ScreenPredicate6 | ScreenPredicate7 | ScreenPredicate8 | ScreenPredicate9 | ScreenPredicate10 | ScreenPredicate11 | ScreenPredicate12 | ScreenPredicate13, Meta(description='Exact predicate evaluated against the latest presented viewport.')]

# Stable names for anonymous tagged-union variants.
DriverHelloData: TypeAlias = DriverData1
DriverAttachedData: TypeAlias = DriverData2
DriverArtifactData: TypeAlias = DriverData12
DriverDetachedData: TypeAlias = DriverData15
DriverDiscardedData: TypeAlias = DriverData16
DriverOutputWaitData: TypeAlias = DriverData4
DriverVerificationData: TypeAlias = DriverData13
DriverBundleData: TypeAlias = DriverData14
DriverCompletionData: TypeAlias = DriverData5
DriverEventsData: TypeAlias = DriverData11
DriverObservationData: TypeAlias = DriverData6
DriverDiagnoseData: TypeAlias = DriverData10
DriverTimelineData: TypeAlias = DriverData8
DriverScreenFrameData: TypeAlias = DriverData7
DriverDiffData: TypeAlias = DriverData9
DriverControlData: TypeAlias = DriverData3
ControlEventCursorData: TypeAlias = ControlData1
ControlAcknowledgedData: TypeAlias = ControlData2
ControlInputAppliedData: TypeAlias = ControlData3
ControlInputSequenceAppliedData: TypeAlias = ControlData4
ControlInputNoopData: TypeAlias = ControlData6
ControlActivatedData: TypeAlias = ControlData7
ControlStatusData: TypeAlias = ControlData8
ControlMarkedData: TypeAlias = ControlData13
ControlSnapshotData: TypeAlias = ControlData9
ControlActionAppliedData: TypeAlias = ControlData5
ControlScreenFrameData: TypeAlias = ControlData10
ControlStableSnapshotData: TypeAlias = ControlData11
ControlScreenWaitData: TypeAlias = ControlData12
DriverErrorResult: TypeAlias = DriverResult2
DriverOkResult: TypeAlias = DriverResult1
