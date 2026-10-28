"""SDK model types generated from the Rust type catalog. Do not edit; run scripts/sdk-types.py."""

from __future__ import annotations

from typing import TYPE_CHECKING, Literal, TypeAlias, TypedDict

if TYPE_CHECKING:
    from typing_extensions import NotRequired

    from termtrace._termtrace import ScreenFrame as FrameHandle

JsonValue: TypeAlias = (
    "None | bool | int | float | str | list[JsonValue] | dict[str, JsonValue]"
)


class ActOutcome(TypedDict):
    "Complete result of one actor-ordered action."

    boundaries: ActorBoundaries
    input: ActionInputResult
    postcondition: ActionPostconditionResult
    request_id: str


class ActionInputResult(TypedDict):
    "Exact applied-input component of an actor-ordered action."

    atom_count: int
    atoms: NotRequired[list[InputSequenceAtomOutcome]]
    bytes_written: int
    first_intent_event_seq: int
    last_result_event_seq: int
    pace_ns: int
    sequence_id: str


class ActionModeChange(TypedDict):
    "One changed ANSI or DEC terminal mode."

    after: bool
    ansi: bool
    before: bool
    number: int


class ActionOutputSummary(TypedDict):
    "Aggregate child output attributable to the last action boundary."

    byte_count: int
    event_count: int


ActionPostconditionResult: TypeAlias = "ActionPostconditionResultNone | ActionPostconditionResultIdle | ActionPostconditionResultScreen"


class ActionPostconditionResultIdle(TypedDict):
    "Final state of an idle postcondition."

    frame: NotRequired[ScreenFrame | None]
    kind: Literal["idle"]
    matched: bool
    quality: StableCaptureQuality
    state_hash: NotRequired[str | None]


class ActionPostconditionResultNone(TypedDict):
    "No presented-state postcondition was requested."

    frame: NotRequired[ScreenFrame | None]
    kind: Literal["none"]
    matched: bool
    state_hash: NotRequired[str | None]


class ActionPostconditionResultScreen(TypedDict):
    "Final state of a screen postcondition."

    frame: NotRequired[ScreenFrame | None]
    kind: Literal["screen"]
    matched: bool
    quality: ScreenPostconditionQuality
    state_hash: NotRequired[str | None]


class ActionWarning(TypedDict):
    "One bounded warning derived from canonical action evidence."

    code: str
    confidence: FindingConfidence
    facts: list[str]


class ActorBoundaries(TypedDict):
    "Durable and latest actor-processed positions returned by live operations."

    durable: CanonicalBoundary
    observed: CanonicalBoundary


AnnotationAnchor: TypeAlias = (
    "Literal['top_left', 'top_right', 'bottom_left', 'bottom_right', 'center']"
)


class AnnotationPosition(TypedDict):
    "Named anchor plus signed pixel offset."

    anchor: AnnotationAnchor
    x: int
    y: int


AnnotationScope: TypeAlias = "AnnotationScopeWholeVideo | AnnotationScopeClip | AnnotationScopeOutputFrame | AnnotationScopeRange"


class AnnotationScopeClip(TypedDict):
    "One normalized clip."

    clip_index: int
    scope: Literal["clip"]


AnnotationScopeInput: TypeAlias = "AnnotationScopeInputWholeVideo | AnnotationScopeInputClip | AnnotationScopeInputOutputFrame | AnnotationScopeInputRange"


class AnnotationScopeInputClip(TypedDict):
    "One normalized clip."

    clip_index: int
    scope: Literal["clip"]


class AnnotationScopeInputOutputFrame(TypedDict):
    "One exact output frame."

    output_frame: int
    scope: Literal["output_frame"]


AnnotationScopeInputRange = TypedDict(
    "AnnotationScopeInputRange",
    {
        "from": "EditSelectorInput",
        "scope": "Literal['range']",
        "to": "EditSelectorInput",
    },
)


class AnnotationScopeInputWholeVideo(TypedDict):
    "Every output frame."

    scope: Literal["whole_video"]


class AnnotationScopeOutputFrame(TypedDict):
    "One exact output frame."

    output_frame: int
    scope: Literal["output_frame"]


AnnotationScopeRange = TypedDict(
    "AnnotationScopeRange",
    {"from": "EditSelector", "scope": "Literal['range']", "to": "EditSelector"},
)


class AnnotationScopeWholeVideo(TypedDict):
    "Every output frame."

    scope: Literal["whole_video"]


class AnnotationStyle(TypedDict):
    "Deterministic visible text style compiled to ASS."

    background: str
    background_opacity: int
    border: int
    font_size: int
    foreground: str


AnyValue: TypeAlias = "JsonValue"


class ApplicationAcknowledgement(TypedDict):
    "Successful acknowledged SDK instrumentation emission. This is not a persisted event envelope."

    acknowledged: bool
    boundaries: ActorBoundaries
    process_instance_id: str
    span_id: str | None


class ApplicationEvent(TypedDict):
    "Caller-reported process and clock information, not independently verified identity.\n\nThe enclosing canonical event supplies recorder time and session identity. A process\ninstance UUID distinguishes PID reuse. Span identifiers are scoped to that instance."

    attributes: dict[str, JsonValue]
    kind: ApplicationEventKind
    monotonic_ns: int
    name: str
    parent_span_id: str | None
    process_id: int
    process_instance_id: str
    span_id: str | None


ApplicationEventKind: TypeAlias = "Literal['event', 'span_start', 'span_end']"


class ArtifactManifestRef(TypedDict):
    "Exact manifest reference, relative to the artifact directory."

    blake3: str
    path: ArtifactRelativePath


class ArtifactMember(TypedDict):
    "Whole-file integrity metadata embedded in a derived manifest."

    blake3: str
    byte_length: int
    media_type: str
    path: str
    relationship: str
    schema_type: str | None
    sensitivity: ArtifactSensitivity


ArtifactProductKind: TypeAlias = (
    "Literal['snapshot', 'video', 'playback', 'presentation', 'preview']"
)


ArtifactProvenance: TypeAlias = "ArtifactProvenanceSemanticRendering | ArtifactProvenanceNativeReplay | ArtifactProvenanceOriginalExecution | ArtifactProvenancePresentationOutput"


class ArtifactProvenanceNativeReplay(TypedDict):
    "Native replay pixels, not original execution."

    kind: Literal["native_replay"]


class ArtifactProvenanceOriginalExecution(TypedDict):
    "Original execution capture."

    kind: Literal["original_execution"]


class ArtifactProvenancePresentationOutput(TypedDict):
    "Derived presentation; authoritative mapping remains in its manifest."

    kind: Literal["presentation_output"]
    source_kind: ArtifactSourceKind


class ArtifactProvenanceSemanticRendering(TypedDict):
    "Semantic reconstruction, not original pixels."

    kind: Literal["semantic_rendering"]


ArtifactRelativePath: TypeAlias = "str"


ArtifactSensitivity: TypeAlias = (
    "Literal['sensitive_terminal_data', 'operational_metadata']"
)


ArtifactSourceKind: TypeAlias = (
    "Literal['semantic_rendering', 'native_replay', 'original_execution']"
)


class ArtifactSourceRef(TypedDict):
    "Binding to canonical source; no frame-exact original-capture correlation implied."

    boundary: SdkCanonicalBoundary
    session_id: str
    session_manifest_hash: str


class AssertionFrameIdentity(TypedDict):
    "Stable frame identity included in bounded assertion diagnostics."

    presented: bool
    projection_hash: str
    state_hash: str


AtomSpec: TypeAlias = "AtomSpecText | AtomSpecBytes | AtomSpecKey | AtomSpecPaste"


class AtomSpecBytes(TypedDict):
    "Exact raw bytes."

    bytes: Base64Bytes


class AtomSpecKey(TypedDict):
    "One key chord in the shared grammar."

    key: str


class AtomSpecPaste(TypedDict):
    "Bracketed-paste text."

    paste: str


class AtomSpecText(TypedDict):
    "UTF-8 text."

    text: str


Base64Bytes: TypeAlias = "bytes"


class BundleManifest(TypedDict):
    "Top-level whole-file bundle manifest."

    canonical_session: Base64Bytes
    format: str
    includes_canonical_evidence: bool
    members: list[ArtifactMember]
    profile: BundleProfile
    session_id: str
    source_final_event_hash: str
    version: int


BundleProfile: TypeAlias = "Literal['screen', 'debug', 'forensic']"


class ByteOutputPage(TypedDict):
    "Bounded exact PTY bytes; this never promises no future output will arrive."

    bytes: Base64Bytes
    cursor: OutputPageCursor
    has_more: bool
    incomplete_tail: bool


class BytePayloadSummary(TypedDict):
    "Hash-and-length summary for one elided canonical byte field."

    blake3: str
    byte_count: int
    field: str


class CanonicalBoundary(TypedDict):
    "Actor-consistent position in the canonical event stream."

    event_hash: str
    next_event_seq: int
    output_bytes: int


class Capability(TypedDict):
    "Result of one bounded capability probe."

    code: str
    detail: str
    id: str
    remediation: str | None
    status: CapabilityStatus


class CapabilityReport(TypedDict):
    "Complete local capability report."

    build_revision: str
    capabilities: list[Capability]
    ghostty_vt_revision: str
    target_arch: str
    target_os: str
    termtrace_version: str


CapabilityStatus: TypeAlias = "Literal['ready', 'blocked']"


CaptionLayout: TypeAlias = "Literal['footer', 'overlay', 'none']"


CaptureQuality: TypeAlias = "Literal['live', 'stable', 'completed', 'incomplete']"


class CaptureTarget(TypedDict):
    "Pixel viewport intended for native visual capture."

    height: int
    padding: NotRequired[WindowPadding]
    width: int


class CellChange(TypedDict):
    "One changed viewport cell."

    after: CellState | None
    before: CellState | None
    column: int


class CellGrid(TypedDict):
    "Terminal grid in cells."

    cols: int
    rows: int


class CellState(TypedDict):
    "One visible cell in a semantic snapshot."

    column: int
    grapheme: str
    resolved_background: Rgb | None
    resolved_foreground: Rgb | None
    selected: bool
    style: CellStyle
    wide_kind: WideKind


class CellStyle(TypedDict):
    "Complete style attached to one terminal cell."

    background: ColorSpec
    blink: bool
    bold: bool
    faint: bool
    foreground: ColorSpec
    inverse: bool
    invisible: bool
    italic: bool
    overline: bool
    strikethrough: bool
    underline: int
    underline_color: ColorSpec


ChangeMode: TypeAlias = (
    "Literal['outline', 'dirty_rows', 'bounding_box', 'heatmap', 'trail', 'side_diff']"
)


class ChangeRegion(TypedDict):
    "One nominal semantic-change region in source pixels."

    height: int
    width: int
    x: int
    y: int


ChangeSource: TypeAlias = "Literal['semantic_cells', 'encoded_pixels']"


class ChangeVisualization(TypedDict):
    "Explicit source and shape for per-frame change overlays."

    hold_frames: int
    mode: ChangeMode
    pixel_threshold: int | None
    source: ChangeSource


class ChangeVisualizationInput(TypedDict):
    "Explicit source and shape for per-frame change overlays."

    hold_frames: NotRequired[int]
    mode: ChangeMode
    pixel_threshold: NotRequired[int | None]
    source: ChangeSource


class ClockSample(TypedDict):
    "A system monotonic clock bracket around a session-relative clock sample."

    session_monotonic_ns: int
    system_monotonic_ns: int
    uncertainty_ns: int


ColorPolicy: TypeAlias = "Literal['auto', 'always', 'never']"


ColorSpec: TypeAlias = "ColorSpecNone | ColorSpecPalette | ColorSpecRgb"


class ColorSpecNone(TypedDict):
    "No explicit color."

    kind: Literal["none"]


class ColorSpecPalette(TypedDict):
    "Index into the active 256-color palette."

    kind: Literal["palette"]
    value: int


class ColorSpecRgb(TypedDict):
    "Explicit true color."

    kind: Literal["rgb"]
    value: Rgb


class ColorState(TypedDict):
    "Active terminal colors."

    background: Rgb
    cursor: Rgb | None
    foreground: Rgb
    palette: list[Rgb]


CommandOutcome: TypeAlias = "ControlOutcome | PointerOutcome"


ConditionSpec: TypeAlias = "ConditionSpecNone | ConditionSpecIdle | ConditionSpecScreen"


class ConditionSpecIdle(TypedDict):
    "Require a reaction, then an unchanged presented state."

    condition: Literal["idle"]
    reaction: ReactionSpec | None
    stable_for_ms: int | None
    timeout_ms: int | None


ConditionSpecInput: TypeAlias = (
    "ConditionSpecInputNone | ConditionSpecInputIdle | ConditionSpecInputScreen"
)


class ConditionSpecInputIdle(TypedDict):
    "Require a reaction, then an unchanged presented state."

    condition: Literal["idle"]
    reaction: NotRequired[ReactionSpecInput | None]
    stable_for_ms: NotRequired[int | None]
    timeout_ms: NotRequired[int | None]


class ConditionSpecInputNone(TypedDict):
    "Return once input is applied."

    condition: Literal["none"]


class ConditionSpecInputScreen(TypedDict):
    "Wait for one presented-screen predicate."

    condition: Literal["screen"]
    predicate: ScreenPredicateInput
    timeout_ms: NotRequired[int | None]


class ConditionSpecNone(TypedDict):
    "Return once input is applied."

    condition: Literal["none"]


class ConditionSpecScreen(TypedDict):
    "Wait for one presented-screen predicate."

    condition: Literal["screen"]
    predicate: ScreenPredicate
    timeout_ms: int | None


class ControlAcknowledgement(TypedDict):
    "The actor applied a control mutation that produces no further data."

    boundaries: ActorBoundaries
    request_id: str


class ControlErrorDetails(TypedDict):
    "Structured actor context attached to a rejected or incomplete request."

    boundaries: ActorBoundaries
    current_presented_state_hash: str
    input_sequence: NotRequired[InputSequenceFailureDetails | None]
    scope: str
    screen_frame: NotRequired[ScreenFrameFailureDetails | None]


ControlOutcome: TypeAlias = "ControlOutcomeAcknowledged | ControlOutcomeMarked"


class ControlOutcomeAcknowledged(TypedDict):
    "The actor applied the command."

    kind: Literal["acknowledged"]


class ControlOutcomeMarked(TypedDict):
    "The actor recorded a marker."

    kind: Literal["marked"]


class CursorMaskFrame(TypedDict):
    "One semantic cursor region masked on a mapped presentation frame."

    evidence_level: str
    output_frame: int
    region: ChangeRegion
    source_frame: int
    state_hash: str
    state_id: int


class CursorState(TypedDict):
    "Cursor state at one semantic boundary."

    blinking: bool
    column: int | None
    in_viewport: bool
    row: int | None
    style: CursorStyle
    visible: bool
    wide_tail: bool | None


class CursorStateInput(TypedDict):
    "Cursor state at one semantic boundary."

    blinking: bool
    column: NotRequired[int | None]
    in_viewport: bool
    row: NotRequired[int | None]
    style: CursorStyle
    visible: bool
    wide_tail: NotRequired[bool | None]


CursorStyle: TypeAlias = "Literal['bar', 'block', 'underline', 'hollow_block']"


DecimalU64: TypeAlias = "int"


class DiagnoseReport(TypedDict):
    "Deterministic report from conservative session-wide detectors."

    broad_change_fraction: str
    checks: list[DiagnosticCheck]
    findings: list[DiagnosticFinding]
    format: str
    output_page: NotRequired[OutputPage | None]
    session_id: str
    source_event_count: int
    source_final_event_hash: str
    version: int


DiagnosticCheck: TypeAlias = "Literal['full_redraw', 'transient', 'dirty', 'cursor', 'modes', 'palette', 'query_loop']"


class DiagnosticFinding(TypedDict):
    "One conservative session-wide diagnostic finding."

    code: str
    confidence: FindingConfidence
    facts: list[str]
    native_correlations: list[NativeCorrelation]
    state_ids: list[int]
    token_ids: list[int]


class DiffEndpoint(TypedDict):
    "Stable identity for one semantic diff endpoint."

    event_seq: int | None
    ghostty_dirty: DirtyState
    ghostty_dirty_rows: list[int]
    monotonic_ns: int
    point: StatePoint
    presented: bool
    state_hash: str
    state_id: int


DirtyState: TypeAlias = "Literal['clean', 'partial', 'full']"


DurabilityMode: TypeAlias = "Literal['strict', 'bounded', 'throughput']"


class DurabilityPolicy(TypedDict):
    "Chain-bound configuration for canonical output persistence."

    maximum_interval_ms: int | None
    maximum_uncommitted_bytes: int | None
    mode: DurabilityMode


class DurabilityRecord(TypedDict):
    "Recorder-observed durability outcomes bound into completion evidence."

    final_sync_completed: bool
    maximum_observed_uncommitted_bytes: int
    maximum_observed_uncommitted_ns: int
    policy: DurabilityPolicy
    sync_count: int


EditClip = TypedDict(
    "EditClip",
    {
        "caption": "str | None",
        "from": "EditSelector",
        "hold_ms": "int",
        "speed": "RationalSpeed",
        "to": "EditSelector",
    },
)


EditClipInput = TypedDict(
    "EditClipInput",
    {
        "caption": "NotRequired[str | None]",
        "from": "EditSelectorInput",
        "hold_ms": "NotRequired[int]",
        "speed": "NotRequired[RationalSpeed]",
        "to": "EditSelectorInput",
    },
)


class EditFailure(TypedDict):
    "Typed edit-plan failure with the location it was found at."

    clip_index: int | None
    code: EditFailureCode
    field: str
    marker: str | None
    reason: str


EditFailureCode: TypeAlias = "Literal['invalid_edit_plan', 'invalid_selector', 'marker_not_found', 'marker_not_unique', 'selector_out_of_range', 'invalid_speed', 'invalid_caption', 'overlapping_clips', 'clip_out_of_range', 'invalid_annotation', 'invalid_subtitle', 'relative_resource_without_base', 'invalid_transform', 'invalid_change_visualization']"


class EditPlan(TypedDict):
    "One immutable presentation edit plan."

    annotations: list[TextAnnotation]
    change_visualization: ChangeVisualization | None
    clips: list[EditClip]
    delivery: PresentationDelivery
    layout: PresentationLayout
    render_config: VideoRenderConfig | None
    subtitles: list[SubtitleTrack]
    transforms: list[PixelTransform]
    version: int


class EditPlanInput(TypedDict):
    "One immutable presentation edit plan."

    annotations: NotRequired[list[TextAnnotationInput]]
    change_visualization: NotRequired[ChangeVisualizationInput | None]
    clips: list[EditClipInput]
    delivery: NotRequired[PresentationDeliveryInput]
    layout: NotRequired[PresentationLayoutInput]
    render_config: NotRequired[VideoRenderConfigInput | None]
    subtitles: NotRequired[list[SubtitleTrack]]
    transforms: NotRequired[list[PixelTransformInput]]
    version: int


class EditSelector(TypedDict):
    "Unique marker or exact canonical monotonic timestamp."

    eventSeq: int | None
    marker: str | None
    monotonicNs: int | None
    nativeFrame: int | None
    stateId: int | None


class EditSelectorInput(TypedDict):
    "Unique marker or exact canonical monotonic timestamp."

    eventSeq: NotRequired[int | None]
    marker: NotRequired[str | None]
    monotonicNs: NotRequired[int | None]
    nativeFrame: NotRequired[int | None]
    stateId: NotRequired[int | None]


EditSource: TypeAlias = "EditSourceFile | EditSourcePlan"


class EditSourceFile(TypedDict):
    "JSON edit file; relative subtitle paths resolve beside it."

    kind: Literal["file"]
    path: ExactPath


EditSourceInput: TypeAlias = "EditSourceInputFile | EditSourceInputPlan"


class EditSourceInputFile(TypedDict):
    "JSON edit file; relative subtitle paths resolve beside it."

    kind: Literal["file"]
    path: ExactPath


class EditSourceInputPlan(TypedDict):
    "In-memory edit; relative subtitle paths need `resource_base`."

    edit: EditPlanInput
    kind: Literal["plan"]
    resource_base: NotRequired[ExactPath | None]


class EditSourcePlan(TypedDict):
    "In-memory edit; relative subtitle paths need `resource_base`."

    edit: EditPlan
    kind: Literal["plan"]
    resource_base: ExactPath | None


EnvironmentBase: TypeAlias = "Literal['inherit', 'clear']"


class EnvironmentSpec(TypedDict):
    "Caller-owned child-environment configuration."

    base: EnvironmentBase
    color: ColorPolicy
    copy: list[str]
    set: dict[str, Base64Bytes]
    unset: list[str]


class EnvironmentSpecInput(TypedDict):
    "Caller-owned child-environment configuration."

    base: NotRequired[EnvironmentBase]
    color: NotRequired[ColorPolicy]
    copy: NotRequired[list[str]]
    set: NotRequired[dict[str, Base64Bytes]]
    unset: NotRequired[list[str]]


class ErrorReport(TypedDict):
    "Classified failure of one step that did not abort its operation."

    code: str
    effect: MutationEffect
    kind: str
    message: str
    retry: Retry


Event: TypeAlias = "EventOutput | EventInputIntent | EventInputResult | EventResizeIntent | EventResizeResult | EventSignalIntent | EventSignalResult | EventMarker | EventApplication | EventQueryResponse | EventLifecycle | EventExit | EventHostError"


class EventApplication(TypedDict):
    "Structured application instrumentation received over the control socket."

    data: ApplicationEvent
    operation_id: str
    type: Literal["application"]


class EventExit(TypedDict):
    "Child process termination."

    code: int | None
    signal: int | None
    signal_name: str | None
    type: Literal["exit"]


EventFollowPosition: TypeAlias = (
    "EventFollowPositionLive | EventFollowPositionCompleted"
)


class EventFollowPositionCompleted(TypedDict):
    "Validated terminal completion, including its source-bound final event hash."

    completion: SessionCompletion
    kind: Literal["completed"]


class EventFollowPositionLive(TypedDict):
    "Position authenticated by the live actor, not imported file metadata."

    device: int
    durable: LiveEventCursor
    header_hash: str
    inode: int
    kind: Literal["live"]
    start: LiveEventCursor | None


class EventHostError(TypedDict):
    "A host failure retained in the canonical timeline."

    code: str
    message: str
    type: Literal["host_error"]


class EventInputIntent(TypedDict):
    "Durable intent to send bytes to the child PTY."

    caller_intent: str | None
    data: Base64Bytes
    descriptor: InputDescriptor
    input_kind: InputKind
    operation_id: str
    semantic_input: str | None
    source: InputSource
    terminal_modes_hash: str | None
    type: Literal["input_intent"]


class EventInputResult(TypedDict):
    "Result of one input intent."

    bytes_written: int
    error_code: str | None
    operation_id: str
    status: OperationStatus
    type: Literal["input_result"]


class EventLifecycle(TypedDict):
    "A process or host lifecycle transition."

    lifecycle_kind: LifecycleKind
    pid: int | None
    type: Literal["lifecycle"]


class EventMarker(TypedDict):
    "A named point in the session timeline."

    label: str
    operation_id: str
    type: Literal["marker"]


EventMetadata: TypeAlias = "EventMetadataOutput | EventMetadataInputIntent | EventMetadataInputResult | EventMetadataResizeIntent | EventMetadataResizeResult | EventMetadataSignalIntent | EventMetadataSignalResult | EventMetadataMarker | EventMetadataApplication | EventMetadataQueryResponse | EventMetadataLifecycle | EventMetadataExit | EventMetadataHostError"


class EventMetadataApplication(TypedDict):
    "Metadata for the canonical Application variant."

    kind: ApplicationEventKind
    operation_id: str
    parent_span_id: str | None
    process_id: int
    process_instance_id: str
    span_id: str | None
    type: Literal["application"]


class EventMetadataExit(TypedDict):
    "Metadata for the canonical Exit variant."

    code: int | None
    signal: int | None
    signal_name: str | None
    type: Literal["exit"]


class EventMetadataHostError(TypedDict):
    "Metadata for the canonical `HostError` variant."

    code: str
    message: str
    type: Literal["host_error"]


class EventMetadataInputIntent(TypedDict):
    "Metadata for the canonical `InputIntent` variant."

    caller_intent: str | None
    descriptor: InputDescriptor
    input_kind: InputKind
    operation_id: str
    semantic_input: str | None
    source: InputSource
    terminal_modes_hash: str | None
    type: Literal["input_intent"]


class EventMetadataInputResult(TypedDict):
    "Metadata for the canonical `InputResult` variant."

    bytes_written: int
    error_code: str | None
    operation_id: str
    status: OperationStatus
    type: Literal["input_result"]


class EventMetadataLifecycle(TypedDict):
    "Metadata for the canonical Lifecycle variant."

    lifecycle_kind: LifecycleKind
    pid: int | None
    type: Literal["lifecycle"]


class EventMetadataMarker(TypedDict):
    "Metadata for the canonical Marker variant."

    label: str
    operation_id: str
    type: Literal["marker"]


class EventMetadataOutput(TypedDict):
    "Output payload is represented by the projection byte summary."

    type: Literal["output"]


class EventMetadataQueryResponse(TypedDict):
    "Metadata for the canonical `QueryResponse` variant."

    responder: str
    type: Literal["query_response"]


class EventMetadataResizeIntent(TypedDict):
    "Metadata for the canonical `ResizeIntent` variant."

    cols: int
    operation_id: str
    pixel_height: int
    pixel_width: int
    rows: int
    type: Literal["resize_intent"]


class EventMetadataResizeResult(TypedDict):
    "Metadata for the canonical `ResizeResult` variant."

    error_code: str | None
    operation_id: str
    status: OperationStatus
    type: Literal["resize_result"]


class EventMetadataSignalIntent(TypedDict):
    "Metadata for the canonical `SignalIntent` variant."

    name: str
    number: int
    operation_id: str
    type: Literal["signal_intent"]


class EventMetadataSignalResult(TypedDict):
    "Metadata for the canonical `SignalResult` variant."

    error_code: str | None
    operation_id: str
    status: OperationStatus
    type: Literal["signal_result"]


class EventOutput(TypedDict):
    "Bytes read from the child PTY."

    data: Base64Bytes
    type: Literal["output"]


class EventPage(TypedDict):
    "A bounded page of lifecycle history."

    events: list[TmuxEventRecord]
    next_sequence: int | None


class EventPageCursor(TypedDict):
    "A checked chain endpoint, not permission to skip validation of an imported cursor."

    event_hash: str
    next_event_seq: int
    source: PageSource


class EventProjection(TypedDict):
    "Stable JSONL projection of one canonical event record."

    byte_payloads: list[BytePayloadSummary]
    event: Event | None
    event_type: str
    hash: str
    metadata: EventMetadata | None
    metadata_only: bool
    monotonic_ns: int
    observed_monotonic_ns: NotRequired[int | None]
    prev_hash: str
    seq: int


class EventQueryResponse(TypedDict):
    "A terminal capability query and corresponding response."

    query: Base64Bytes
    responder: str
    response: Base64Bytes
    source_byte_offset: NotRequired[int | None]
    source_event_seq: NotRequired[int | None]
    type: Literal["query_response"]


class EventResizeIntent(TypedDict):
    "Durable intent to change terminal geometry."

    cols: int
    operation_id: str
    pixel_height: int
    pixel_width: int
    rows: int
    type: Literal["resize_intent"]


class EventResizeResult(TypedDict):
    "Result of one resize intent."

    error_code: str | None
    operation_id: str
    status: OperationStatus
    type: Literal["resize_result"]


class EventSignalIntent(TypedDict):
    "Durable intent to signal the child process group."

    name: str
    number: int
    operation_id: str
    type: Literal["signal_intent"]


class EventSignalResult(TypedDict):
    "Result of one signal intent."

    error_code: str | None
    operation_id: str
    status: OperationStatus
    type: Literal["signal_result"]


EvidencePolicy: TypeAlias = "Literal['on_failure', 'always', 'manual']"


ExactPath: TypeAlias = "str"


class ExpectationMismatchError(TypedDict):
    "A valid expectation that did not match.\n\nOnly this type is suitable for conversion into a runner's negatable mismatch value."

    actual: str
    assertion: str
    excerpt: str
    expected: str
    frame: AssertionFrameIdentity | None
    truncated: bool


class ExplainFinding(TypedDict):
    "One deterministic evidence-backed diagnostic finding."

    code: str
    confidence: FindingConfidence
    facts: list[str]
    token_ids: list[int]


class ExplainOutputPage(TypedDict):
    "Independent page coordinates for each ordered explain collection."

    findings: OutputPage
    states: OutputPage
    tokens: OutputPage


class ExplainReport(TypedDict):
    "Causal evidence around one canonical state point."

    context_events: int
    findings: list[ExplainFinding]
    output_page: NotRequired[ExplainOutputPage | None]
    session_id: str
    source_event_count: int
    source_final_event_hash: str
    states: list[StateRecord]
    target: StatePoint
    target_state: StateRecord
    tokens: list[VtToken]


class FinalizationReport(TypedDict):
    "Result of exactly-once test finalization."

    evidence_directory: str | None
    outcome: TestOutcome
    retained: bool
    secondary_failures: list[SecondaryFailure]
    verified_artifacts: list[str]


FindingConfidence: TypeAlias = "Literal['high', 'medium']"


FrameDetail: TypeAlias = "Literal['none', 'text', 'spans', 'cells']"


class GarbageCollectionCandidate(TypedDict):
    "One completed, unregistered state-root recording eligible for explicit deletion."

    bytes: int
    completed_unix_ns: int
    session: ExactPath
    session_id: str


class GarbageCollectionReport(TypedDict):
    "Safe state-root retention plan or applied deletion report."

    candidates: list[GarbageCollectionCandidate]
    changes_applied: bool
    completed_bytes_after_plan: int
    completed_bytes_before: int
    keep_latest: int
    max_completed_bytes: int | None
    older_than_days: int
    protected_registered: int
    quota_excess_bytes: int
    reclaimed_bytes: int
    removed_sessions: int
    removed_stale_aliases: int
    removed_supervisor_files: int
    skipped_corrupt_registry_entries: int
    skipped_non_completed: int
    state_root: ExactPath


class GeometryProbeAttempt(TypedDict):
    "One viewport and measured grid considered by the exact-grid solver."

    report: TerminalGeometryReport
    target: CaptureTarget


class GeometrySolveReport(TypedDict):
    "Bounded capture-time search proving the selected viewport fits the canonical grid."

    attempts: list[GeometryProbeAttempt]
    required_cols: int
    required_rows: int
    selected: CaptureTarget


GifLoop: TypeAlias = "Literal['infinite', 'once']"


InputDescriptor: TypeAlias = "InputDescriptorRaw | InputDescriptorText | InputDescriptorKey | InputDescriptorPaste | InputDescriptorFocus | InputDescriptorPointer | InputDescriptorActivate"


class InputDescriptorActivate(TypedDict):
    "Semantic visible-text resolution followed by a mode-aware click."

    cols: int
    encoder: str
    end_column: int
    kind: Literal["activate"]
    presented_state_hash: str
    row: int
    rows: int
    start_column: int
    target_column: int
    terminal_modes_hash: str
    text: str


class InputDescriptorFocus(TypedDict):
    "Focus transition encoded while DEC focus reporting was enabled."

    encoder: str
    focused: bool
    kind: Literal["focus"]
    terminal_modes_hash: str


class InputDescriptorKey(TypedDict):
    "One semantic key encoded against a live terminal-mode snapshot."

    alt: bool
    control: bool
    encoder: str
    key: str
    kind: Literal["key"]
    shift: bool
    super_key: bool
    terminal_modes_hash: str


class InputDescriptorPaste(TypedDict):
    "Paste content encoded against live bracketed-paste mode."

    bracketed: bool
    kind: Literal["paste"]
    terminal_modes_hash: str


class InputDescriptorPointer(TypedDict):
    "Pointer operation encoded against live mouse modes and geometry."

    button: str
    click_count: int
    cols: int
    column: int
    encoder: str
    end_column: int | None
    end_row: int | None
    kind: Literal["pointer"]
    lines: int
    operation: str
    pixel_height: int
    pixel_width: int
    row: int
    rows: int
    terminal_modes_hash: str


class InputDescriptorRaw(TypedDict):
    "Uninterpreted caller or host bytes."

    kind: Literal["raw"]


class InputDescriptorText(TypedDict):
    "UTF-8 text intentionally entered as textual content."

    kind: Literal["text"]


InputKind: TypeAlias = "Literal['raw', 'text', 'keys', 'paste', 'focus', 'pointer']"


class InputSequenceAtomOutcome(TypedDict):
    "Canonical result coordinates for one atom in a successful input sequence."

    bytes_written: int
    input_kind: InputKind
    intent_event_seq: int
    operation_id: str
    result_event_seq: int


class InputSequenceFailureDetails(TypedDict):
    "Recoverable accounting for an input sequence that stopped during PTY delivery."

    atom_count: int
    atoms_applied: int
    bytes_written: int
    failed_atom_bytes_written: int
    failed_atom_index: int
    failed_operation_id: str
    first_intent_event_seq: int
    last_result_event_seq: int
    resume_from_atom_index: int
    sequence_id: str


InputSource: TypeAlias = "Literal['human', 'agent', 'host']"


InvalidRegionReason: TypeAlias = (
    "Literal['cells_require_explicit_region', 'empty', 'reversed', 'out_of_bounds']"
)


class LastActionAnalysis(TypedDict):
    "Both task-level reports derived in one canonical replay/diagnostic pass."

    diagnostics: LastActionDiagnosticsReport
    inspection: LastActionReport


class LastActionBoundary(TypedDict):
    "Canonical request and intent/result boundaries defining one input action."

    atom_count: int
    bytes_written: int
    error_code: str | None
    intended_bytes: int
    intent_event_seq: int
    operation_ids: list[str]
    request_event_end_seq: int
    request_event_start_seq: int
    request_id: str | None
    result_event_seq: int
    status: OperationStatus


class LastActionDiagnosticsReport(TypedDict):
    "Diagnostics restricted to the semantic and token range after the last input action."

    action: LastActionBoundary
    findings: list[DiagnosticFinding]
    findings_truncated: bool
    format: str
    from_state_id: int
    session_id: str
    source_final_event_hash: str
    to_state_id: int
    version: int


class LastActionReport(TypedDict):
    "Compact semantic result of the most recent canonical input action."

    action: LastActionBoundary
    cells_changed: int
    cursor_movement: ValueChange | None
    format: str
    from_state_id: int
    mode_changes: list[ActionModeChange]
    output_generated: ActionOutputSummary
    presentation_eligible: bool
    presentation_eligible_states: int
    rows_changed: list[int]
    session_id: str
    source_final_event_hash: str
    to_state_id: int
    version: int
    warnings: list[ActionWarning]
    warnings_truncated: bool


class LaunchOptions(TypedDict):
    "Launch configuration in host form."

    cols: int | None
    cwd: str | None
    environment: EnvironmentSpec | None
    idle_timeout_ms: int | None
    max_duration_ms: int | None
    output_byte_budget: int | None
    rows: int | None
    startup_timeout_ms: int | None
    visual_profile: VisualProfile | None


class LaunchOptionsInput(TypedDict):
    "Launch configuration in host form."

    cols: NotRequired[int | None]
    cwd: NotRequired[str | None]
    environment: NotRequired[EnvironmentSpecInput | None]
    idle_timeout_ms: NotRequired[int | None]
    max_duration_ms: NotRequired[int | None]
    output_byte_budget: NotRequired[int | None]
    rows: NotRequired[int | None]
    startup_timeout_ms: NotRequired[int | None]
    visual_profile: NotRequired[VisualProfile | None]


LifecycleKind: TypeAlias = "Literal['spawned', 'output_closed', 'stopping']"


class LiveEventCursor(TypedDict):
    "An actor-owned position immediately after a canonical record (or the header)."

    event_hash: str
    file_offset: int
    monotonic_ns: int
    next_event_seq: int


LocatorCondition: TypeAlias = "Literal['exists', 'absent'] | LocatorConditionCount"


class LocatorConditionCount(TypedDict):
    "Exactly this many matches."

    count: int


LocatorFailure: TypeAlias = "QueryError | LocatorWaitFailure"


LocatorWaitFailure: TypeAlias = (
    "LocatorWaitFailureLocatorTimeout | LocatorWaitFailureCancelled"
)


class LocatorWaitFailureCancelled(TypedDict):
    "The wait was cancelled."

    frame_hash: str | None
    kind: Literal["cancelled"]
    query: QueryDescription
    state_hash: str | None


class LocatorWaitFailureLocatorTimeout(TypedDict):
    "The wait timed out."

    frame_hash: str | None
    kind: Literal["locator_timeout"]
    query: QueryDescription
    state_hash: str | None


ManagementFailure: TypeAlias = "ManagementFailureRegistryRestart | ManagementFailureTmux | ManagementFailureOriginalCapture"


class ManagementFailureOriginalCapture(TypedDict):
    "Original-execution capture lifecycle facts; the application may have run."

    facts: OriginalCaptureFacts
    operation: Literal["original_capture"]


class ManagementFailureRegistryRestart(TypedDict):
    "Registry restart workflow facts."

    facts: RestartFailureFacts
    operation: Literal["registry_restart"]


class ManagementFailureTmux(TypedDict):
    "Tmux management workflow facts."

    facts: TmuxFailureFacts
    operation: Literal["tmux"]


class MarkOutcome(TypedDict):
    "Canonical position of an appended marker."

    boundaries: ActorBoundaries
    event_seq: int
    label: str
    request_id: str


class MarkerRecord(TypedDict):
    "Canonical marker with duplicate and nominal-frame context."

    event_seq: int
    label: str
    monotonic_ns: int
    native_evidence_level: str
    nominal_frame: int
    occurrence_count: int
    occurrence_index: int
    presentation_eligible: bool
    semantic_state_hash: str | None
    semantic_state_id: int | None


class MarkerRecordInput(TypedDict):
    "Canonical marker with duplicate and nominal-frame context."

    event_seq: int
    label: str
    monotonic_ns: int
    native_evidence_level: str
    nominal_frame: int
    occurrence_count: int
    occurrence_index: int
    presentation_eligible: bool
    semantic_state_hash: NotRequired[str | None]
    semantic_state_id: NotRequired[int | None]


class MaskedSnapshotOptions(TypedDict):
    "Options for the separate version-one cells comparison format.\n\nMasks exclude unstable comparison cells. They do not redact canonical evidence."

    include_cursor: bool
    include_modes: bool
    mask_regions: list[ScreenRegion]
    profile: SnapshotProfile


class MaskedSnapshotOptionsInput(TypedDict):
    "Options for the separate version-one cells comparison format.\n\nMasks exclude unstable comparison cells. They do not redact canonical evidence."

    include_cursor: NotRequired[bool]
    include_modes: NotRequired[bool]
    mask_regions: NotRequired[list[ScreenRegion]]
    profile: NotRequired[SnapshotProfile]


class MatchRange(TypedDict):
    "One absolute half-open range covered by a match."

    end_column: int
    row: int
    start_column: int


class MatchedCell(TypedDict):
    "Bounded cell evidence retained by a match."

    appearance: ScreenAppearance
    column: int
    grapheme: str
    row: int
    wide_kind: WideKind
    width: int


class MediaArtifactResult(TypedDict):
    "Versioned SDK envelope. Data only; does not promise runtime capture/export support."

    directory_bytes: Base64Bytes
    manifest: ArtifactManifestRef
    members: list[SdkArtifactMember]
    product_kind: ArtifactProductKind
    provenance: ArtifactProvenance
    source: ArtifactSourceRef
    version: int


class ModeState(TypedDict):
    "Terminal modes relevant to presentation and input."

    alternate_screen: bool
    bracketed_paste: bool
    synchronized_output: bool
    values: list[TerminalMode]
    wraparound: bool


MouseButton: TypeAlias = "Literal['left', 'middle', 'right', 'wheel_up', 'wheel_down']"


MutationEffect: TypeAlias = "MutationEffectNotApplied | MutationEffectPartial | MutationEffectApplied | MutationEffectUnknown"


class MutationEffectApplied(TypedDict):
    "Every requested mutation was applied."

    kind: Literal["applied"]


class MutationEffectNotApplied(TypedDict):
    "Nothing was applied."

    kind: Literal["not_applied"]


class MutationEffectPartial(TypedDict):
    "A known prefix of input atoms was applied completely, and nothing after it."

    completed_atoms: int
    kind: Literal["partial"]


class MutationEffectUnknown(TypedDict):
    "The request may or may not have been applied."

    kind: Literal["unknown"]


MutationJournalStatus: TypeAlias = "Literal['missing', 'verified', 'invalid']"


class NativeCorrelation(TypedDict):
    "One source-bound relationship to a native artifact."

    frame: int | None
    level: NativeEvidenceLevel
    limitation: str
    manifest: ExactPath
    state_id: int


NativeEvidenceLevel: TypeAlias = "Literal['presentation_eligible', 'scheduled_for_frame', 'post_endpoint_stable_capture', 'frame_observed']"


class NativeProfileReport(TypedDict):
    "Native facts observed while resolving a native profile."

    discovered_font_families: list[str]
    dpi: int
    effective_config_hash: str
    geometry: GeometrySolveReport
    ghostty_executable_blake3: str
    ghostty_version: str
    replay_profile_hash: str
    requested_font_families: list[str]


class NativeVisualProfile(TypedDict):
    "Native Ghostty settings. Absent fields keep the base configuration's value."

    background: NotRequired[str | None]
    config: NotRequired[str | None]
    dpi: NotRequired[int | None]
    font_family: NotRequired[str | None]
    font_size_points: NotRequired[str | None]
    foreground: NotRequired[str | None]
    padding_points: NotRequired[PaddingPoints | None]


class NormalizedAnnotation(TypedDict):
    "One annotation resolved to a half-open output-frame interval."

    annotation_index: int
    output_end_frame: int
    output_start_frame: int
    position: AnnotationPosition
    style: AnnotationStyle
    template: str | None
    text: str | None
    z_order: int


class NormalizedAnnotationInput(TypedDict):
    "One annotation resolved to a half-open output-frame interval."

    annotation_index: int
    output_end_frame: int
    output_start_frame: int
    position: AnnotationPosition
    style: AnnotationStyle
    template: NotRequired[str | None]
    text: NotRequired[str | None]
    z_order: int


class NormalizedChangeFrame(TypedDict):
    "Semantic delta nominally scheduled onto presentation frames."

    changed_regions: list[ChangeRegion]
    dirty_rows: list[int]
    evidence_level: str
    output_frames: list[int]
    source_frame: int
    state_hash: str
    state_id: int


class NormalizedClip(TypedDict):
    "Resolved clip and exact source/output frame decisions."

    caption: str | None
    hold_frame_count: int
    output_end_frame: int
    output_start_frame: int
    retimed_frame_count: int
    source_end_frame: int
    source_range: SourceRange
    source_start_frame: int
    speed: RationalSpeed


class NormalizedClipInput(TypedDict):
    "Resolved clip and exact source/output frame decisions."

    caption: NotRequired[str | None]
    hold_frame_count: int
    output_end_frame: int
    output_start_frame: int
    retimed_frame_count: int
    source_end_frame: int
    source_range: SourceRange
    source_start_frame: int
    speed: RationalSpeed


class NormalizedEdit(TypedDict):
    "Edit plan with resolved resource paths and its path-independent identity."

    edit: EditPlan
    edit_plan_hash: str


class NormalizedSubtitle(TypedDict):
    "Validated subtitle input with stable content identity."

    resource_hash: str
    source_path: ExactPath


NullableString: TypeAlias = "str | None"


class Observation(TypedDict):
    "Truth-labeled observation of one terminal state."

    boundary: CanonicalBoundary
    canonical_session: Base64Bytes | None
    capture_quality: CaptureQuality
    event_seq: int | None
    frame: ObservationFrame
    integrity: ObservationIntegrity
    monotonic_ns: int | None
    payload_hash: str
    presented: bool
    resolution: ObservationResolution
    session_id: str | None
    source_completed: bool
    source_kind: ObservationSourceKind
    state_hash: str
    state_id: int | None
    state_point: StatePoint
    truth_layer: TruthLayer


class ObservationFrame(TypedDict):
    "Compact stable frame shared by CLI and clients."

    cols: int
    cursor_column: int | None
    cursor_row: int | None
    cursor_visible: bool
    modes: ModeState
    rows: int
    viewport_text: str


ObservationIntegrity: TypeAlias = "Literal['canonical_chained', 'unchained']"


ObservationResolution: TypeAlias = "Literal['actor_snapshot', 'canonical_replay', 'validated_checkpoint', 'ansi_interpretation']"


ObservationSourceKind: TypeAlias = "Literal['live_session', 'completed_session', 'interrupted_session', 'ansi_stream', 'imported_ansi', 'one_off_command']"


class Observers(TypedDict):
    "PIDs of live observers."

    supervisor: int | None
    watchdog: int | None


OperationStatus: TypeAlias = "Literal['applied', 'failed']"


class OriginalCaptureFacts(TypedDict):
    "What a caller learns when an original capture did not publish, or did not finish in time.\n\nThe captured application may have run and completed; `completion` is its canonical outcome\nwhen one validates. This is a lifecycle failure, not an actor mutation, so it carries no\nrequest effect and no automatic retry."

    cleanup: str | None
    code: OriginalCaptureFailure | None
    completion: SessionCompletion | None
    reason: str
    session: Base64Bytes
    state: OriginalCaptureState
    surviving: list[SurvivingPathRecord]


OriginalCaptureFailure: TypeAlias = "Literal['cancelled', 'acquisition_failed', 'grid_mismatch', 'supervisor_abandoned', 'recording_failed', 'publication_failed', 'startup_failed']"


class OriginalCaptureFailureRecord(TypedDict):
    "Terminal failure facts written by the supervisor."

    cleanup: str | None
    code: OriginalCaptureFailure
    reason: str
    surviving: list[SurvivingPathRecord]


OriginalCaptureState: TypeAlias = "Literal['acquiring', 'live', 'finalizing', 'published', 'failed', 'cancelled', 'abandoned']"


class OriginalCaptureStatus(TypedDict):
    "Owner-only supervisor status, published beside the session as\n`<session>.original-capture.json`. Not canonical evidence."

    backend: str
    canvas: PixelCanvas
    failure: OriginalCaptureFailureRecord | None
    format: str
    fps: int
    grid: CellGrid | None
    result: MediaArtifactResult | None
    session_id: str | None
    state: OriginalCaptureState
    supervisor_pid: DecimalU64
    supervisor_start_ticks: DecimalU64
    version: int


class OriginalLaunchOptions(TypedDict):
    "Original-capture launch configuration in host form. `grid` and `viewport` are exclusive,\nand the canonical `rows`/`cols` options do not exist here: the captured terminal owns its grid."

    alias: str | None
    canvas: PixelCanvas | None
    cwd: str | None
    environment: EnvironmentSpec | None
    fps: int | None
    ghostty_config: str | None
    grid: CellGrid | None
    idle_timeout_ms: int | None
    max_duration_ms: int | None
    output_byte_budget: int | None
    startup_timeout_ms: int | None
    state_root: str | None
    viewport: PixelCanvas | None
    visual_profile: VisualProfile | None


class OriginalLaunchOptionsInput(TypedDict):
    "Original-capture launch configuration in host form. `grid` and `viewport` are exclusive,\nand the canonical `rows`/`cols` options do not exist here: the captured terminal owns its grid."

    alias: NotRequired[str | None]
    canvas: NotRequired[PixelCanvas | None]
    cwd: NotRequired[str | None]
    environment: NotRequired[EnvironmentSpecInput | None]
    fps: NotRequired[int | None]
    ghostty_config: NotRequired[str | None]
    grid: NotRequired[CellGrid | None]
    idle_timeout_ms: NotRequired[int | None]
    max_duration_ms: NotRequired[int | None]
    output_byte_budget: NotRequired[int | None]
    startup_timeout_ms: NotRequired[int | None]
    state_root: NotRequired[str | None]
    viewport: NotRequired[PixelCanvas | None]
    visual_profile: NotRequired[VisualProfile | None]


class OriginalRecording(TypedDict):
    "A finished original recording whose capture was published."

    canonical_success: bool
    capture: MediaArtifactResult
    completion: SessionCompletion


class OutputFrameSource(TypedDict):
    "Source of one presentation output frame."

    clip_index: int
    operation: str
    source_frame: int


class OutputPage(TypedDict):
    "Deterministic page coordinates for one ordered collection."

    limit: int | None
    next_offset: int | None
    offset: int
    returned: int
    total: int | None
    truncated: bool


class OutputPageCursor(TypedDict):
    "A byte position anchored to the chain endpoint enclosing that output prefix."

    byte_offset: int
    events: EventPageCursor


class PaddingPoints(TypedDict):
    "Symmetric window padding in points per side."

    x: int
    y: int


class PageSource(TypedDict):
    "Identity of the canonical file opened for a page. Display paths are not authoritative."

    device: int
    header_hash: str
    inode: int
    session_id: str
    session_path: Base64Bytes
    session_path_display: str


class PendingOperationReport(TypedDict):
    "Serializable uncertain-operation summary."

    expected_bytes: int | None
    intent_seq: int
    kind: str
    operation_id: str


class PixelCanvas(TypedDict):
    "Captured display size in pixels; constant for the whole video."

    height: int
    width: int


PixelTransform: TypeAlias = "PixelTransformCursorMask | PixelTransformCrop | PixelTransformZoom | PixelTransformResize | PixelTransformCallout"


class PixelTransformCallout(TypedDict):
    "Draw a visibly labeled rectangular callout."

    height: int
    label: str | None
    transform: Literal["callout"]
    width: int
    x: int
    y: int


class PixelTransformCrop(TypedDict):
    "Crop to an exact source rectangle."

    height: int
    transform: Literal["crop"]
    width: int
    x: int
    y: int


class PixelTransformCursorMask(TypedDict):
    "Replace cursor pixels during presentation composition."

    transform: Literal["cursor_mask"]


PixelTransformInput: TypeAlias = "PixelTransformInputCursorMask | PixelTransformInputCrop | PixelTransformInputZoom | PixelTransformInputResize | PixelTransformInputCallout"


class PixelTransformInputCallout(TypedDict):
    "Draw a visibly labeled rectangular callout."

    height: int
    label: NotRequired[str | None]
    transform: Literal["callout"]
    width: int
    x: int
    y: int


class PixelTransformInputCrop(TypedDict):
    "Crop to an exact source rectangle."

    height: int
    transform: Literal["crop"]
    width: int
    x: int
    y: int


class PixelTransformInputCursorMask(TypedDict):
    "Replace cursor pixels during presentation composition."

    transform: Literal["cursor_mask"]


class PixelTransformInputResize(TypedDict):
    "Resize to an exact presentation canvas."

    height: int
    transform: Literal["resize"]
    width: int


class PixelTransformInputZoom(TypedDict):
    "Scale by one exact rational factor."

    denominator: int
    numerator: int
    transform: Literal["zoom"]


class PixelTransformResize(TypedDict):
    "Resize to an exact presentation canvas."

    height: int
    transform: Literal["resize"]
    width: int


class PixelTransformZoom(TypedDict):
    "Scale by one exact rational factor."

    denominator: int
    numerator: int
    transform: Literal["zoom"]


PointerOperation: TypeAlias = "PointerOperationClick | PointerOperationMultiClick | PointerOperationDrag | PointerOperationScroll"


class PointerOperationClick(TypedDict):
    "Press and release one button at a cell."

    button: MouseButton
    kind: Literal["click"]


class PointerOperationDrag(TypedDict):
    "Press at the command cell, emit motion samples, and release at an end cell."

    button: MouseButton
    kind: Literal["drag"]
    steps: int
    to_column: int
    to_row: int


class PointerOperationMultiClick(TypedDict):
    "Repeat a complete press/release click a bounded number of times."

    button: MouseButton
    count: int
    kind: Literal["multi_click"]


class PointerOperationScroll(TypedDict):
    "Emit signed vertical wheel ticks at a cell."

    kind: Literal["scroll"]
    lines: int


PointerOutcome: TypeAlias = (
    "PointerOutcomeApplied | PointerOutcomeNoop | PointerOutcomeActivated"
)


class PointerOutcomeActivated(TypedDict):
    "Visible text was resolved and its center clicked."

    boundaries: ActorBoundaries
    bytes: Base64Bytes
    bytes_written: int
    end_column: int
    intent_event_seq: int
    kind: Literal["activated"]
    presented_state_hash: str
    request_id: str
    result_event_seq: int
    row: int
    start_column: int
    target_column: int
    text: str


class PointerOutcomeApplied(TypedDict):
    "The actor encoded the request against live modes and wrote these exact bytes."

    boundaries: ActorBoundaries
    bytes: Base64Bytes
    bytes_written: int
    input_kind: InputKind
    intent_event_seq: int
    kind: Literal["applied"]
    request_id: str
    result_event_seq: int


class PointerOutcomeNoop(TypedDict):
    "The application does not report this input, so the actor correctly wrote nothing."

    boundaries: ActorBoundaries
    input_kind: InputKind
    kind: Literal["noop"]
    reason: str
    request_id: str
    terminal_modes_hash: str


PostconditionSummary: TypeAlias = (
    "PostconditionSummaryNone | PostconditionSummaryIdle | PostconditionSummaryScreen"
)


class PostconditionSummaryIdle(TypedDict):
    "An idle postcondition was requested."

    kind: Literal["idle"]
    matched: bool
    quality: StableCaptureQuality
    state_hash: str | None


class PostconditionSummaryNone(TypedDict):
    "No postcondition was requested."

    kind: Literal["none"]
    matched: bool
    state_hash: str | None


class PostconditionSummaryScreen(TypedDict):
    "A screen predicate postcondition was requested."

    kind: Literal["screen"]
    matched: bool
    quality: ScreenPostconditionQuality
    state_hash: str | None


class PresentationDelivery(TypedDict):
    "Delivery codec and pixel-format contract."

    codec: str
    crf: int
    encoder_preset: str
    pixel_format: str


class PresentationDeliveryInput(TypedDict):
    "Delivery codec and pixel-format contract."

    codec: str
    crf: NotRequired[int]
    encoder_preset: NotRequired[str]
    pixel_format: str


class PresentationLayout(TypedDict):
    "Canvas layout for caption presets."

    branding: str | None
    caption: CaptionLayout


class PresentationLayoutInput(TypedDict):
    "Canvas layout for caption presets."

    branding: NotRequired[str | None]
    caption: CaptionLayout


class PresentationPlan(TypedDict):
    "Dry-run output binding a normalized edit to immutable visual evidence."

    annotations: list[NormalizedAnnotation]
    change_algorithm: str | None
    change_frames: list[NormalizedChangeFrame]
    change_visualization: ChangeVisualization | None
    clips: list[NormalizedClip]
    cursor_masks: list[CursorMaskFrame]
    delivery: PresentationDelivery
    edit_plan_hash: str
    fps: int
    layout: PresentationLayout
    markers: list[MarkerRecord]
    output_frame_count: int
    output_height: int
    output_width: int
    render_config: VideoRenderConfig | None
    source: PresentationSource
    source_geometry: TerminalGeometryReport
    source_height: int
    source_width: int
    subtitles: list[NormalizedSubtitle]
    transforms: list[PixelTransform]
    version: int


class PresentationPlanInput(TypedDict):
    "Dry-run output binding a normalized edit to immutable visual evidence."

    annotations: list[NormalizedAnnotationInput]
    change_algorithm: NotRequired[str | None]
    change_frames: list[NormalizedChangeFrame]
    change_visualization: NotRequired[ChangeVisualizationInput | None]
    clips: list[NormalizedClipInput]
    cursor_masks: list[CursorMaskFrame]
    delivery: PresentationDeliveryInput
    edit_plan_hash: str
    fps: int
    layout: PresentationLayoutInput
    markers: list[MarkerRecordInput]
    output_frame_count: int
    output_height: int
    output_width: int
    render_config: NotRequired[VideoRenderConfigInput | None]
    source: PresentationSource
    source_geometry: TerminalGeometryReport
    source_height: int
    source_width: int
    subtitles: list[NormalizedSubtitle]
    transforms: list[PixelTransformInput]
    version: int


PresentationSource: TypeAlias = "PresentationSourceNative | PresentationSourceSemantic"


class PresentationSourceNative(TypedDict):
    "Actual pixels from a verified native Ghostty capture."

    kind: Literal["native"]
    video: str
    video_hash: str


class PresentationSourceSemantic(TypedDict):
    "Deterministic generated pixels from canonical semantic replay."

    kind: Literal["semantic"]
    plan: SemanticVideoSourcePlan


class PreviewOptions(TypedDict):
    "Bounded storyboard and low-resolution preview policy."

    change_only: bool
    columns: int
    max_frames: int
    preview_width: int
    tile_width: int


ProcessSignal: TypeAlias = (
    "Literal['interrupt', 'terminate', 'hangup', 'user1', 'user2']"
)


class ProjectedEventPage(TypedDict):
    "Bounded projected canonical events. `has_more` concerns this finite snapshot only."

    cursor: EventPageCursor
    events: list[EventProjection]
    has_more: bool
    incomplete_tail: bool


class ProjectionSpec(TypedDict):
    "Requested terminating-frame projection in host form."

    detail: FrameDetail | None
    region: ScreenRegion | None


ProvenanceCapture: TypeAlias = "Literal['value', 'hash', 'name', 'not_captured']"


class QueryDescription(TypedDict):
    "Bounded normalized query evidence."

    text: str
    truncated: bool


QueryError: TypeAlias = "QueryErrorInvalidQuery | QueryErrorNoMatch | QueryErrorMultipleMatches | QueryErrorIndexOutOfBounds | QueryErrorQueryResourceLimit | QueryErrorInsufficientFrameDetail"


class QueryErrorIndexOutOfBounds(TypedDict):
    "A zero-based explicit index was absent."

    count: int
    frame_hash: str
    index: int
    kind: Literal["index_out_of_bounds"]
    query: QueryDescription
    state_hash: str


class QueryErrorInsufficientFrameDetail(TypedDict):
    "The frame detail could not be losslessly evaluated."

    kind: Literal["insufficient_frame_detail"]
    reason: str


class QueryErrorInvalidQuery(TypedDict):
    "Empty, zero-width, conflicting, or otherwise ambiguous query input."

    kind: Literal["invalid_query"]
    reason: str


class QueryErrorMultipleMatches(TypedDict):
    "A strict operation found multiple matches."

    count: int
    frame_hash: str
    kind: Literal["multiple_matches"]
    query: QueryDescription
    ranges: list[MatchRange]
    state_hash: str
    truncated: bool


class QueryErrorNoMatch(TypedDict):
    "A strict operation found no match."

    frame_hash: str
    kind: Literal["no_match"]
    query: QueryDescription
    state_hash: str


class QueryErrorQueryResourceLimit(TypedDict):
    "A bounded query result exceeded its public resource ceiling."

    kind: Literal["query_resource_limit"]
    maximum: int
    requested: int


class RationalSpeed(TypedDict):
    "Exact positive playback speed numerator/denominator."

    denominator: int
    numerator: int


class ReactionSpec(TypedDict):
    "Reaction required before an idle interval, in host millisecond units."

    changed_from: str | None
    timeout_ms: int | None


class ReactionSpecInput(TypedDict):
    "Reaction required before an idle interval, in host millisecond units."

    changed_from: NotRequired[str | None]
    timeout_ms: NotRequired[int | None]


class RecoveryIssue(TypedDict):
    "First invalid tail encountered during prefix recovery."

    line: int
    reason: str


class RecoveryReport(TypedDict):
    "Evidence produced by non-destructive event-log prefix recovery."

    discarded_tail: RecoveryIssue | None
    pending_operations: list[PendingOperationReport]
    recovered: ExactPath
    source: ExactPath
    valid_events: int


class RegistryEntry(TypedDict):
    "One friendly name bound to an immutable canonical session identity."

    alias: str
    canonical_session: Base64Bytes
    control_protocol_version: int
    format: str
    recorder_pid: int | None
    recorder_process_start_ticks: int | None
    registered_unix_ns: int
    session_id: str
    version: int


class RegistryListEntry(TypedDict):
    "One registry listing row with evidence-derived session summary."

    alias: str
    canonical_session: Base64Bytes
    command: list[Base64Bytes] | None
    diagnostic: str | None
    geometry: SessionGeometry | None
    lifecycle: SessionPhase
    registered_unix_ns: int
    session_id: str
    started_unix_ns: int | None
    working_directory: Base64Bytes | None


class RegistrySessionResolution(TypedDict):
    "A named session path paired with the registry's evidence-derived lifecycle."

    diagnostic: str | None
    lifecycle: SessionPhase
    session: ExactPath


class ReplayCachePruneReport(TypedDict):
    "Result of explicitly deleting disposable replay checkpoints."

    directory: ExactPath
    remaining: ReplayCacheStatus
    removed_bytes: int
    removed_entries: int


class ReplayCacheStatus(TypedDict):
    "Bounded disposable replay-cache inventory for one canonical session."

    compressed_bytes: int
    directory: ExactPath
    entries: int
    maximum_compressed_bytes: int
    maximum_entries: int


ResourceLimitKind: TypeAlias = "Literal['output_bytes', 'max_duration', 'idle_timeout']"


class ResourceLimitOutcome(TypedDict):
    "Resource-limit trigger retained in immutable completion evidence."

    kind: ResourceLimitKind
    limit: int
    observed: int


class RestartAttempt(TypedDict):
    "Exact allocated paths and proven successor launch/readiness facts, not rollback claims."

    launched: bool
    readiness: SuccessorReadiness | None
    session: Base64Bytes
    stderr_log: Base64Bytes
    stdout_log: Base64Bytes


class RestartConfiguration(TypedDict):
    "Typed successor configuration. Absent fields keep predecessor values; clear flags explicitly\nremove optional limits. This is a recorder-launch DTO, not a persisted evidence format."

    allow_environment_change: bool
    cell_height: int | None
    cell_width: int | None
    clear_idle_timeout: bool
    clear_max_duration: bool
    clear_output_byte_budget: bool
    cols: int | None
    command: list[Base64Bytes] | None
    cwd: Base64Bytes | None
    durability: RestartDurability | None
    durability_max_interval_ms: int | None
    durability_max_uncommitted_bytes: int | None
    idle_timeout_ms: int | None
    max_duration_ms: int | None
    output_byte_budget: int | None
    provenance: RestartProvenance | None
    provenance_environment: list[tuple[str, ProvenanceCapture]]
    rows: int | None
    term: str | None


class RestartConfigurationInput(TypedDict):
    "Typed successor configuration. Absent fields keep predecessor values; clear flags explicitly\nremove optional limits. This is a recorder-launch DTO, not a persisted evidence format."

    allow_environment_change: NotRequired[bool]
    cell_height: NotRequired[int | None]
    cell_width: NotRequired[int | None]
    clear_idle_timeout: NotRequired[bool]
    clear_max_duration: NotRequired[bool]
    clear_output_byte_budget: NotRequired[bool]
    cols: NotRequired[int | None]
    command: NotRequired[list[Base64Bytes] | None]
    cwd: NotRequired[Base64Bytes | None]
    durability: NotRequired[RestartDurability | None]
    durability_max_interval_ms: NotRequired[int | None]
    durability_max_uncommitted_bytes: NotRequired[int | None]
    idle_timeout_ms: NotRequired[int | None]
    max_duration_ms: NotRequired[int | None]
    output_byte_budget: NotRequired[int | None]
    provenance: NotRequired[RestartProvenance | None]
    provenance_environment: NotRequired[list[tuple[str, ProvenanceCapture]]]
    rows: NotRequired[int | None]
    term: NotRequired[str | None]


RestartDurability: TypeAlias = "Literal['Strict', 'Bounded', 'Throughput']"


class RestartFailureFacts(TypedDict):
    "Proven restart progress. No alias rollback or deduplicated replay is promised."

    alias: str
    phase: RestartFailurePhase
    predecessor_completion: SessionCompletion | None
    predecessor_path: Base64Bytes | None
    successor: RestartAttempt | None


RestartFailurePhase: TypeAlias = "Literal['resolve_predecessor', 'stopping_predecessor', 'predecessor_completed', 'successor_allocated', 'successor_launched', 'successor_ready', 'lineage_validated', 'alias_retarget']"


RestartProvenance: TypeAlias = "Literal['Minimal', 'Reproducible', 'Custom']"


class RestartReport(TypedDict):
    "Shared restart result, retaining the predecessor and compare-and-retargeted successor."

    alias: str
    predecessor: RegistryEntry
    ready: bool
    stderr_log: Base64Bytes
    stdout_log: Base64Bytes
    successor: RegistryEntry


Retry: TypeAlias = (
    "Literal['new_request', 'same_request_id', 'inspect_first', 'do_not_retry']"
)


class ReviewExportManifest(TypedDict):
    "Manifest for one explicit reviewer state export."

    canonical_session: ExactPath
    screen_text_blake3: str
    source_final_event_hash: str
    state_hash: str
    state_id: int
    state_json_blake3: str
    version: int


class ReviewSearchMatch(TypedDict):
    "One semantic viewport text match with a copyable canonical selector."

    event_seq: int | None
    presented: bool
    selector: str
    state_hash: str
    state_id: int


class ReviewSearchResult(TypedDict):
    "Stable noninteractive reviewer text-search result."

    canonical_session: ExactPath
    matches: list[ReviewSearchMatch]
    query: str


class ReviewSummary(TypedDict):
    "Noninteractive reviewer source summary."

    canonical_session: ExactPath
    final_state_hash: str
    marker_count: int
    native_correlation_count: int
    presented_state_count: int
    rendering_policy: str
    state_count: int


class Rgb(TypedDict):
    "RGB color independent of libghostty's unstable ABI."

    b: int
    g: int
    r: int


class RowChange(TypedDict):
    "Changed cells and native wrap facts for one viewport row."

    cells: list[CellChange]
    row: int
    soft_wrapped: ValueChange8 | None
    wrap_continuation: ValueChange8 | None


class RowDiff(TypedDict):
    "Fine-grained changes for one viewport row."

    cells: list[CellState]
    row: int
    soft_wrapped: bool
    wrap_continuation: bool


class RowMove(TypedDict):
    "Contiguous block of complete rows found at a different position in the later state."

    count: int
    from_row: int
    to_row: int


SaveFormat: TypeAlias = (
    "Literal['txt', 'json', 'deep-json', 'vt', 'html', 'svg', 'png']"
)


class SaveManifest(TypedDict):
    "Manifest binding every saved representation to one source state."

    canonical_session: Base64Bytes
    files: list[SavedFile]
    format: str
    semantic_image_resources: list[SemanticImageResource]
    sensitivity: str
    session_id: str
    source_final_event_hash: str
    state_hash: str
    state_id: int
    state_point: StatePoint
    version: int


class SaveOutcome(TypedDict):
    "Atomic save result."

    directory: ExactPath
    manifest: SaveManifest


class SavedFile(TypedDict):
    "One manifested saved file."

    blake3: str
    byte_count: int
    media_type: str
    path: str
    renderer: str | None
    truth_layer: str | None


class ScreenAppearance(TypedDict):
    "Complete appearance shared by an exact cell or a lossless span."

    resolved_background: Rgb | None
    resolved_foreground: Rgb | None
    selected: bool
    style: CellStyle


class ScreenCell(TypedDict):
    "Exact projected terminal cell."

    appearance: ScreenAppearance
    column: int
    grapheme: str
    row: int
    wide_kind: WideKind


class ScreenCellRow(TypedDict):
    "Cell-detail row."

    cells: list[ScreenCell]
    row: int
    soft_wrapped: bool
    text: str
    wrap_continuation: bool


class ScreenChanges(TypedDict):
    "Presented-screen changes between two frames of the same projection."

    after_state_hash: str
    before_state_hash: str
    cursor: NotRequired[ValueChange | None]
    geometry: NotRequired[ValueChange2 | None]
    modes: NotRequired[ValueChange3 | None]
    moves: list[RowMove]
    rows: list[ScreenRowChange]


class ScreenFrame(TypedDict):
    "Versioned, bounded projection of one exact semantic state."

    capture_quality: CaptureQuality
    colors: ColorState
    cursor: CursorState
    data: ScreenFrameData
    frame_hash: str
    geometry: ScreenGeometry
    modes: ModeState
    presented: bool
    requested_region: ScreenRegion
    returned_region: ScreenRegion
    source: ScreenFrameSource
    state_hash: str
    version: int


ScreenFrameData: TypeAlias = "ScreenFrameDataNone | ScreenFrameDataText | ScreenFrameDataSpans | ScreenFrameDataCells"


class ScreenFrameDataCells(TypedDict):
    "Exact cells."

    detail: Literal["cells"]
    rows: list[ScreenCellRow]


class ScreenFrameDataNone(TypedDict):
    "Observation metadata only; no cell or text payload."

    detail: Literal["none"]


class ScreenFrameDataSpans(TypedDict):
    "Lossless appearance spans."

    detail: Literal["spans"]
    rows: list[ScreenSpanRow]


class ScreenFrameDataText(TypedDict):
    "Convenience text and authoritative wrap facts."

    detail: Literal["text"]
    rows: list[ScreenTextRow]


class ScreenFrameFailureDetails(TypedDict):
    "Bounded context for a rejected rich screen projection."

    cols: int | None
    detail: FrameDetail | None
    limit_kind: str | None
    maximum: int | None
    region: ScreenRegion | None
    region_reason: InvalidRegionReason | None
    requested: int | None
    rows: int | None


ScreenFrameSource: TypeAlias = "ScreenFrameSourceActor | ScreenFrameSourceResolved"


class ScreenFrameSourceActor(TypedDict):
    "State selected while the live actor still owned exclusivity."

    boundaries: ActorBoundaries
    observed_monotonic_ns: int
    session_id: str
    source: Literal["actor"]


class ScreenFrameSourceResolved(TypedDict):
    "State selected through a completed or explicit semantic resolution."

    boundary: CanonicalBoundary
    event_seq: int | None
    integrity: ObservationIntegrity
    monotonic_ns: int | None
    resolution: ObservationResolution
    session_id: str | None
    source: Literal["resolved"]
    source_completed: bool
    source_kind: ObservationSourceKind
    state_id: int | None
    state_point: StatePoint


class ScreenGeometry(TypedDict):
    "Complete terminal geometry, independent of the returned region."

    cols: int
    rows: int


ScreenMode: TypeAlias = "ScreenModeAlternateScreen | ScreenModeBracketedPaste | ScreenModeWraparound | ScreenModeSynchronizedOutput | ScreenModeAnsi | ScreenModeDec"


class ScreenModeAlternateScreen(TypedDict):
    "Any alternate-screen mode."

    mode: Literal["alternate_screen"]


class ScreenModeAnsi(TypedDict):
    "Numeric ANSI mode."

    mode: Literal["ansi"]
    number: int


class ScreenModeBracketedPaste(TypedDict):
    "DEC bracketed-paste mode 2004."

    mode: Literal["bracketed_paste"]


class ScreenModeDec(TypedDict):
    "Numeric DEC-private mode."

    mode: Literal["dec"]
    number: int


class ScreenModeSynchronizedOutput(TypedDict):
    "DEC synchronized-output mode 2026."

    mode: Literal["synchronized_output"]


class ScreenModeWraparound(TypedDict):
    "DEC auto-wrap mode 7."

    mode: Literal["wraparound"]


ScreenPostconditionQuality: TypeAlias = "Literal['matched', 'matched_unsettled', 'child_completed', 'output_closed', 'deadline']"


ScreenPredicate: TypeAlias = "ScreenPredicateContains | ScreenPredicateAbsent | ScreenPredicateMatchesRegex | ScreenPredicateExcludesRegex | ScreenPredicateRowContains | ScreenPredicateRowAbsent | ScreenPredicateRegionContains | ScreenPredicateRegionAbsent | ScreenPredicateCursor | ScreenPredicateMode | ScreenPredicateStateHash | ScreenPredicateAll | ScreenPredicateAny"


class ScreenPredicateAbsent(TypedDict):
    "Viewport text excludes an exact UTF-8 substring."

    predicate: Literal["absent"]
    text: str


class ScreenPredicateAll(TypedDict):
    "Every nested predicate must match."

    predicate: Literal["all"]
    predicates: list[ScreenPredicate]


class ScreenPredicateAny(TypedDict):
    "At least one nested predicate must match."

    predicate: Literal["any"]
    predicates: list[ScreenPredicate]


class ScreenPredicateContains(TypedDict):
    "Viewport text contains an exact UTF-8 substring."

    predicate: Literal["contains"]
    text: str


class ScreenPredicateCursor(TypedDict):
    "Cursor properties all equal the supplied values."

    column: int | None
    predicate: Literal["cursor"]
    row: int | None
    shape: CursorStyle | None
    visible: bool | None


class ScreenPredicateExcludesRegex(TypedDict):
    "Viewport text does not match a bounded linear-time regular expression."

    pattern: str
    predicate: Literal["excludes_regex"]


ScreenPredicateInput: TypeAlias = "ScreenPredicateInputContains | ScreenPredicateInputAbsent | ScreenPredicateInputMatchesRegex | ScreenPredicateInputExcludesRegex | ScreenPredicateInputRowContains | ScreenPredicateInputRowAbsent | ScreenPredicateInputRegionContains | ScreenPredicateInputRegionAbsent | ScreenPredicateInputCursor | ScreenPredicateInputMode | ScreenPredicateInputStateHash | ScreenPredicateInputAll | ScreenPredicateInputAny"


class ScreenPredicateInputAbsent(TypedDict):
    "Viewport text excludes an exact UTF-8 substring."

    predicate: Literal["absent"]
    text: str


class ScreenPredicateInputAll(TypedDict):
    "Every nested predicate must match."

    predicate: Literal["all"]
    predicates: list[ScreenPredicateInput]


class ScreenPredicateInputAny(TypedDict):
    "At least one nested predicate must match."

    predicate: Literal["any"]
    predicates: list[ScreenPredicateInput]


class ScreenPredicateInputContains(TypedDict):
    "Viewport text contains an exact UTF-8 substring."

    predicate: Literal["contains"]
    text: str


class ScreenPredicateInputCursor(TypedDict):
    "Cursor properties all equal the supplied values."

    column: NotRequired[int | None]
    predicate: Literal["cursor"]
    row: NotRequired[int | None]
    shape: NotRequired[CursorStyle | None]
    visible: NotRequired[bool | None]


class ScreenPredicateInputExcludesRegex(TypedDict):
    "Viewport text does not match a bounded linear-time regular expression."

    pattern: str
    predicate: Literal["excludes_regex"]


class ScreenPredicateInputMatchesRegex(TypedDict):
    "Viewport text matches a bounded linear-time regular expression."

    pattern: str
    predicate: Literal["matches_regex"]


class ScreenPredicateInputMode(TypedDict):
    "One stable terminal mode equals the requested value."

    enabled: bool
    mode: ScreenMode
    predicate: Literal["mode"]


class ScreenPredicateInputRegionAbsent(TypedDict):
    "A half-open cell region excludes exact UTF-8 text."

    predicate: Literal["region_absent"]
    region: ScreenRegion
    text: str


class ScreenPredicateInputRegionContains(TypedDict):
    "A half-open cell region contains exact UTF-8 text."

    predicate: Literal["region_contains"]
    region: ScreenRegion
    text: str


class ScreenPredicateInputRowAbsent(TypedDict):
    "One zero-based row excludes exact UTF-8 text."

    predicate: Literal["row_absent"]
    row: int
    text: str


class ScreenPredicateInputRowContains(TypedDict):
    "One zero-based row contains exact UTF-8 text."

    predicate: Literal["row_contains"]
    row: int
    text: str


class ScreenPredicateInputStateHash(TypedDict):
    "Presented state hash equals or differs from a supplied BLAKE3 hash."

    comparison: StateHashComparison
    hash: str
    predicate: Literal["state_hash"]


class ScreenPredicateMatchesRegex(TypedDict):
    "Viewport text matches a bounded linear-time regular expression."

    pattern: str
    predicate: Literal["matches_regex"]


class ScreenPredicateMode(TypedDict):
    "One stable terminal mode equals the requested value."

    enabled: bool
    mode: ScreenMode
    predicate: Literal["mode"]


class ScreenPredicateRegionAbsent(TypedDict):
    "A half-open cell region excludes exact UTF-8 text."

    predicate: Literal["region_absent"]
    region: ScreenRegion
    text: str


class ScreenPredicateRegionContains(TypedDict):
    "A half-open cell region contains exact UTF-8 text."

    predicate: Literal["region_contains"]
    region: ScreenRegion
    text: str


class ScreenPredicateRowAbsent(TypedDict):
    "One zero-based row excludes exact UTF-8 text."

    predicate: Literal["row_absent"]
    row: int
    text: str


class ScreenPredicateRowContains(TypedDict):
    "One zero-based row contains exact UTF-8 text."

    predicate: Literal["row_contains"]
    row: int
    text: str


class ScreenPredicateStateHash(TypedDict):
    "Presented state hash equals or differs from a supplied BLAKE3 hash."

    comparison: StateHashComparison
    hash: str
    predicate: Literal["state_hash"]


class ScreenRegion(TypedDict):
    "Absolute zero-based half-open terminal region."

    bottom: int
    left: int
    right: int
    top: int


class ScreenRowChange(TypedDict):
    "One changed row of a presented-screen frame."

    after: str | None
    appearance_only: bool
    before: str | None
    row: int


class ScreenSpan(TypedDict):
    "Lossless run of cells with identical complete appearance."

    appearance: ScreenAppearance
    cell_count: int
    cells: list[SpanCell]
    end_column: int
    start_column: int
    text: str


class ScreenSpanRow(TypedDict):
    "Span-detail row."

    row: int
    soft_wrapped: bool
    spans: list[ScreenSpan]
    text: str
    wrap_continuation: bool


class ScreenTextRow(TypedDict):
    "Text-detail row."

    row: int
    soft_wrapped: bool
    text: str
    wrap_continuation: bool


ScreenWaitQuality: TypeAlias = "Literal['matched', 'matched_unsettled', 'child_completed', 'output_closed', 'deadline', 'completed']"


class ScreenWaitResult(TypedDict):
    "Screen wait evidence with the derived satisfaction verdict and without its frame, which\nbindings attach as a native frame object."

    first_match_monotonic_ns: int | None
    matched: bool
    observed_monotonic_ns: int
    predicate: ScreenPredicate
    quality: ScreenWaitQuality
    satisfied: bool
    stable_for_ns: int
    frame: FrameHandle


class SdkArtifactMember(TypedDict):
    "Portable member metadata. Canonical `ArtifactMember` serialization is unchanged."

    blake3: str
    byte_length: DecimalU64
    media_type: str
    path: ArtifactRelativePath
    relationship: str
    schema_type: NullableString
    sensitivity: ArtifactSensitivity


class SdkCanonicalBoundary(TypedDict):
    "Exact canonical boundary with portable wide integers."

    event_hash: str
    next_event_seq: DecimalU64
    output_bytes: DecimalU64


class SecondaryFailure(TypedDict):
    "One bounded failure that could not replace the body's primary failure."

    message: str
    phase: str
    truncated: bool


SemanticDiff = TypedDict(
    "SemanticDiff",
    {
        "active_style": "ValueChange5 | None",
        "changed": "bool",
        "colors": "ValueChange4 | None",
        "cursor": "ValueChange | None",
        "from": "DiffEndpoint",
        "geometry": "ValueChange2 | None",
        "metadata": "ValueChange6 | None",
        "modes": "ValueChange3 | None",
        "moves": "list[RowMove]",
        "presented": "ValueChange7 | None",
        "rows": "list[RowChange]",
        "to": "DiffEndpoint",
    },
)


class SemanticFontResource(TypedDict):
    "Exact identity of one bundled semantic font face."

    decompressed_blake3: str
    decompressed_size: int
    family: str
    repository_source: str
    role: SemanticFontRole


SemanticFontRole: TypeAlias = (
    "Literal['regular', 'bold', 'italic', 'bold-italic', 'symbols-fallback']"
)


class SemanticImageResource(TypedDict):
    "Exact font and layout resources used by semantic image output."

    background_policy: str
    cell_height: int
    cell_width: int
    fallback_policy: str
    font_set_blake3: str
    font_size: int
    fonts: list[SemanticFontResource]
    padding: int
    raster_profile: SemanticRasterProfile
    symbols_baseline: int
    symbols_font_size: int
    text_baseline: int


SemanticPreset: TypeAlias = "Literal['bundled-v1']"


SemanticRasterProfile: TypeAlias = "Literal['direct', 'supersampled-2x']"


class SemanticVideoCanvas(TypedDict):
    "Fixed canvas used for every frame of one semantic video source."

    cell_height: int
    cell_width: int
    cols: int
    height: int
    policy: str
    rows: int
    width: int


class SemanticVideoSourcePlan(TypedDict):
    "Immutable semantic source description resolved during presentation planning."

    canvas: SemanticVideoCanvas
    end_monotonic_ns: int
    font_set_blake3: str
    fonts: list[SemanticFontResource]
    fps: int
    frame_count: int
    raster_profile: SemanticRasterProfile
    renderer: str
    scheduling_policy: str
    session: ExactPath
    session_id: str
    source_event_hash: str
    start_monotonic_ns: int
    version: int


class SemanticVisualProfile(TypedDict):
    "Semantic renderer selection."

    preset: SemanticPreset


class SendSpec(TypedDict):
    "Ordered mixed steps sent under one caller identity, in host form."

    if_state: str | None
    intent: str | None
    pace_ms: int | None
    projection: ProjectionSpec | None
    report_changes: bool
    request_id: str | None
    steps: list[SendStepSpec]
    until: ConditionSpec | None
    verbose: bool


SendStepSpec: TypeAlias = "SendStepSpecInput | SendStepSpecResize | SendStepSpecSignal | SendStepSpecMark | SendStepSpecPointer | SendStepSpecFocus | SendStepSpecActivate"


class SendStepSpecActivate(TypedDict):
    "Resolve visible text and click its center."

    step: Literal["activate"]
    strict: bool
    text: str
    unique: bool


class SendStepSpecFocus(TypedDict):
    "Report a focus transition."

    focused: bool
    step: Literal["focus"]
    strict: bool


SendStepSpecInput: TypeAlias = "SendStepSpecInputInput | SendStepSpecInputResize | SendStepSpecInputSignal | SendStepSpecInputMark | SendStepSpecInputPointer | SendStepSpecInputFocus | SendStepSpecInputActivate"


class SendStepSpecInputActivate(TypedDict):
    "Resolve visible text and click its center."

    step: Literal["activate"]
    strict: NotRequired[bool]
    text: str
    unique: NotRequired[bool]


class SendStepSpecInputFocus(TypedDict):
    "Report a focus transition."

    focused: bool
    step: Literal["focus"]
    strict: NotRequired[bool]


class SendStepSpecInputInput(TypedDict):
    "A run of input atoms applied as one actor-ordered action."

    atoms: list[AtomSpec]
    step: Literal["input"]


class SendStepSpecInputMark(TypedDict):
    "Append a canonical marker."

    label: str
    step: Literal["mark"]


class SendStepSpecInputPointer(TypedDict):
    "Apply one cell-addressed pointer operation."

    column: int
    operation: PointerOperation
    row: int
    step: Literal["pointer"]
    strict: NotRequired[bool]


class SendStepSpecInputResize(TypedDict):
    "Resize the child PTY; pixel dimensions derive from the default cell size."

    cols: int
    rows: int
    step: Literal["resize"]


class SendStepSpecInputSignal(TypedDict):
    "Send one allow-listed signal to the child process group."

    signal: ProcessSignal
    step: Literal["signal"]


class SendStepSpecMark(TypedDict):
    "Append a canonical marker."

    label: str
    step: Literal["mark"]


class SendStepSpecPointer(TypedDict):
    "Apply one cell-addressed pointer operation."

    column: int
    operation: PointerOperation
    row: int
    step: Literal["pointer"]
    strict: bool


class SendStepSpecResize(TypedDict):
    "Resize the child PTY; pixel dimensions derive from the default cell size."

    cols: int
    rows: int
    step: Literal["resize"]


class SendStepSpecSignal(TypedDict):
    "Send one allow-listed signal to the child process group."

    signal: ProcessSignal
    step: Literal["signal"]


class SessionCompletion(TypedDict):
    "Immutable completion facts written after the final event is durable."

    durability: DurabilityRecord
    event_count: int
    event_header_hash: str
    exit_code: int | None
    failure: SessionFailure | None
    final_event_hash: str
    format: str
    output_bytes: int
    resource_limit: ResourceLimitOutcome | None
    session_id: str
    session_manifest_hash: str
    signal_name: str | None
    terminal_event_seq: int
    validation_version: int
    version: int


class SessionFailure(TypedDict):
    "Recorder failure retained when the child could not complete normally."

    code: str
    message: str


class SessionGeometry(TypedDict):
    "Initial PTY geometry."

    cols: int
    pixel_height: int
    pixel_width: int
    rows: int


SessionPhase: TypeAlias = "Literal['live', 'completed', 'incomplete', 'stale', 'missing_evidence', 'identity_mismatch', 'incompatible']"


SessionStatus: TypeAlias = "SessionStatusLive | SessionStatusCompleted"


class SessionStatusCompleted(TypedDict):
    "The session has completed."

    completion: SessionCompletion
    kind: Literal["completed"]


class SessionStatusLive(TypedDict):
    "The live actor answered."

    boundaries: ActorBoundaries
    clock_sample: ClockSample
    kind: Literal["live"]
    pid: int | None


SnapshotProfile: TypeAlias = "Literal['text', 'spans', 'cells']"


class SourceRange(TypedDict):
    "A non-empty half-open interval on the canonical monotonic timeline."

    end_monotonic_ns: int
    start_monotonic_ns: int


class SpanCell(TypedDict):
    "Per-column content retained inside an appearance span."

    grapheme: str
    wide_kind: WideKind


StableCaptureQuality: TypeAlias = "Literal['stable', 'child_completed', 'output_closed', 'deadline', 'unchanged', 'completed']"


class StableScreenCapture(TypedDict):
    "Atomic stable-screen outcome retaining the exact frame.\n\nThe frame is not serialized; bindings attach it as a native frame object."

    first_observed_monotonic_ns: int
    observed_monotonic_ns: int
    quality: StableCaptureQuality
    semantic_state_zstd: Base64Bytes | None
    stable_for_ns: int
    frame: FrameHandle


class StateDelta(TypedDict):
    "Delta from the previous distinct state."

    active_style: CellStyle | None
    colors: ColorState | None
    cursor: CursorState | None
    geometry: tuple[int, int] | None
    metadata: TerminalMetadata | None
    modes: ModeState | None
    rows: list[RowDiff]


StateHashComparison: TypeAlias = "Literal['unchanged', 'changed']"


StatePoint: TypeAlias = "StatePointLatestPresented | StatePointStateId | StatePointStateHash | StatePointEventSeq | StatePointMonotonicNs | StatePointMarker"


class StatePointEventSeq(TypedDict):
    "Latest state at or before an event sequence."

    event_seq: int
    selector: Literal["event_seq"]


class StatePointLatestPresented(TypedDict):
    "Latest presentation-eligible state."

    selector: Literal["latest_presented"]


class StatePointMarker(TypedDict):
    "State at a uniquely named marker."

    label: str
    selector: Literal["marker"]


class StatePointMonotonicNs(TypedDict):
    "Latest state at or before a session-monotonic timestamp."

    monotonic_ns: int
    selector: Literal["monotonic_ns"]


class StatePointStateHash(TypedDict):
    "Exact content-addressed semantic state hash."

    selector: Literal["state_hash"]
    state_hash: str


class StatePointStateId(TypedDict):
    "Exact semantic state identifier."

    selector: Literal["state_id"]
    state_id: int


class StateRecord(TypedDict):
    "One distinct semantic state boundary."

    delta: StateDelta
    event_byte_offset: int | None
    event_seq: int | None
    ghostty_dirty: DirtyState
    ghostty_dirty_rows: list[int]
    monotonic_ns: int
    presented: bool
    state_hash: str
    state_id: int


StopReport: TypeAlias = "StopReportRequested | StopReportCompleted"


class StopReportCompleted(TypedDict):
    "Validated completion evidence, published before or after the request."

    request_id: str
    completion: SessionCompletion
    kind: Literal["completed"]


class StopReportRequested(TypedDict):
    "The actor acknowledged the request; completion was not awaited."

    request_id: str
    boundaries: ActorBoundaries
    kind: Literal["requested"]


class StyleFilter(TypedDict):
    "Complete optional appearance constraints applied to every covered logical cell."

    background: ColorSpec
    blink: bool
    bold: bool
    faint: bool
    foreground: ColorSpec
    inverse: bool
    invisible: bool
    italic: bool
    overline: bool
    resolved_background: Rgb | None
    resolved_foreground: Rgb | None
    selected: bool
    strikethrough: bool
    underline: int
    underline_color: ColorSpec
    wide_kind: WideKind


class StyleFilterInput(TypedDict):
    "Complete optional appearance constraints applied to every covered logical cell."

    background: NotRequired[ColorSpec]
    blink: NotRequired[bool]
    bold: NotRequired[bool]
    faint: NotRequired[bool]
    foreground: NotRequired[ColorSpec]
    inverse: NotRequired[bool]
    invisible: NotRequired[bool]
    italic: NotRequired[bool]
    overline: NotRequired[bool]
    resolved_background: NotRequired[Rgb | None]
    resolved_foreground: NotRequired[Rgb | None]
    selected: NotRequired[bool]
    strikethrough: NotRequired[bool]
    underline: NotRequired[int]
    underline_color: NotRequired[ColorSpec]
    wide_kind: NotRequired[WideKind]


class SubtitleTrack(TypedDict):
    "Caller-authored ASS resource copied and hash-manifested before rendering."

    ass: ExactPath


SuccessorReadiness: TypeAlias = (
    "Literal['live', 'completed', 'exited', 'timed_out', 'failed']"
)


SurvivingKind: TypeAlias = (
    "Literal['session', 'failed_visual', 'staging', 'sidecar', 'log']"
)


class SurvivingPathRecord(TypedDict):
    "A path verified to exist when the failure was reported."

    kind: SurvivingKind
    path: Base64Bytes


class TerminalGeometryReport(TypedDict):
    "Geometry reported by a live terminal through XTWINOPS queries."

    cell_height: int
    cell_width: int
    cols: int
    rows: int
    text_height: int
    text_width: int


class TerminalMetadata(TypedDict):
    "Terminal metadata controlled by OSC sequences."

    title: Base64Bytes
    working_directory: Base64Bytes


class TerminalMode(TypedDict):
    "One ANSI or DEC mode value."

    ansi: bool
    enabled: bool
    number: int


TestOutcome: TypeAlias = (
    "Literal['passed', 'assertion_failed', 'error', 'panic', 'timed_out']"
)


class TextAnnotation(TypedDict):
    "One text annotation with an exact scope, position, and visible style."

    position: AnnotationPosition
    scope: AnnotationScope
    style: AnnotationStyle
    template: str | None
    text: str | None
    z_order: int


class TextAnnotationInput(TypedDict):
    "One text annotation with an exact scope, position, and visible style."

    position: NotRequired[AnnotationPosition]
    scope: AnnotationScopeInput
    style: NotRequired[AnnotationStyle]
    template: NotRequired[str | None]
    text: NotRequired[str | None]
    z_order: NotRequired[int]


class TimelineEntry(TypedDict):
    "One ordered, source-bound timeline item."

    event_seq: int | None
    item: TimelineItem
    monotonic_ns: int


TimelineItem: TypeAlias = (
    "TimelineItemSemanticState | TimelineItemCanonicalEvent | TimelineItemVtToken"
)


class TimelineItemCanonicalEvent(TypedDict):
    "A non-output canonical event retained verbatim."

    event: Event
    kind: Literal["canonical_event"]


class TimelineItemSemanticState(TypedDict):
    "A distinct Ghostty semantic state transition."

    changed_cells: int
    changed_rows: list[int]
    cursor_changed: bool
    event_byte_offset: int | None
    ghostty_dirty: DirtyState
    kind: Literal["semantic_state"]
    metadata_changed: bool
    modes_changed: bool
    presented: bool
    state_hash: str
    state_id: int


class TimelineItemVtToken(TypedDict):
    "One lossless explanatory VT frame."

    kind: Literal["vt_token"]
    token: VtToken


class TimelinePage(TypedDict):
    "One bounded page of a concise diagnostic timeline."

    entries: list[TimelineEntry]
    next_offset: int | None


TmuxCatalog: TypeAlias = "TmuxCatalogTracks | TmuxCatalogGaps"


class TmuxCatalogGaps(TypedDict):
    "Coverage intervals."

    entries: list[TmuxGapCatalogEntry]
    kind: Literal["gaps"]
    manifest: TmuxRecordingManifest
    summary: TmuxCatalogSummary


class TmuxCatalogSummary(TypedDict):
    "Counts committed with the lifecycle journal's indexed inventory."

    coverage_gaps: int
    tracks: int
    unfinished_tracks: int


class TmuxCatalogTracks(TypedDict):
    "Track incarnations."

    entries: list[TmuxTrackCatalogEntry]
    kind: Literal["tracks"]
    manifest: TmuxRecordingManifest
    summary: TmuxCatalogSummary


class TmuxCompositeTrack(TypedDict):
    "Link from an aggregate recording to its outer tmux client recording."

    alias: str
    link: TmuxSessionLink | None


TmuxCoverage: TypeAlias = "Literal['warn', 'strict']"


class TmuxCoverageGap(TypedDict):
    "One uncovered pane or explicit observation limitation."

    first_observed_unix_ns: int
    kind: TmuxGapKind
    last_observed_unix_ns: int
    pane_id: str | None
    resolved_unix_ns: int | None


TmuxEventRecord: TypeAlias = "TmuxEventRecordRecordingStarted | TmuxEventRecordCompositeTrackLinked | TmuxEventRecordRecordingStopping | TmuxEventRecordRecordingCompleted | TmuxEventRecordServerIdentityBound | TmuxEventRecordObserverStarted | TmuxEventRecordSessionCreated | TmuxEventRecordSessionDestroyed | TmuxEventRecordWindowCreated | TmuxEventRecordWindowDestroyed | TmuxEventRecordPaneCreated | TmuxEventRecordPaneDestroyed | TmuxEventRecordPaneMoved | TmuxEventRecordLayoutChanged | TmuxEventRecordActiveWindowChanged | TmuxEventRecordActivePaneChanged | TmuxEventRecordZoomChanged | TmuxEventRecordPaneModeChanged | TmuxEventRecordPaneTrackLinked | TmuxEventRecordPaneTrackCompleted | TmuxEventRecordCoverageGapOpened | TmuxEventRecordCoverageGapResolved"


class TmuxEventRecordActivePaneChanged(TypedDict):
    "A pane became active or inactive in its window."

    observed_unix_ns: int
    sequence: int
    active: bool
    kind: Literal["active_pane_changed"]
    pane: TmuxPaneAddress


class TmuxEventRecordActiveWindowChanged(TypedDict):
    "A pane's window became active or inactive."

    observed_unix_ns: int
    sequence: int
    active: bool
    kind: Literal["active_window_changed"]
    pane: TmuxPaneAddress


class TmuxEventRecordCompositeTrackLinked(TypedDict):
    "The composite client track started."

    observed_unix_ns: int
    sequence: int
    kind: Literal["composite_track_linked"]
    link: TmuxSessionLink


class TmuxEventRecordCoverageGapOpened(TypedDict):
    "A coverage interval opened."

    observed_unix_ns: int
    sequence: int
    gap: TmuxCoverageGap
    kind: Literal["coverage_gap_opened"]


class TmuxEventRecordCoverageGapResolved(TypedDict):
    "A coverage interval resolved."

    observed_unix_ns: int
    sequence: int
    gap: TmuxCoverageGap
    kind: Literal["coverage_gap_resolved"]


class TmuxEventRecordLayoutChanged(TypedDict):
    "A pane's window layout changed."

    observed_unix_ns: int
    sequence: int
    kind: Literal["layout_changed"]
    layout: str
    pane: TmuxPaneAddress


class TmuxEventRecordObserverStarted(TypedDict):
    "A supervisor process started observing the server."

    observed_unix_ns: int
    sequence: int
    kind: Literal["observer_started"]
    pid: int


class TmuxEventRecordPaneCreated(TypedDict):
    "A pane appeared."

    observed_unix_ns: int
    sequence: int
    kind: Literal["pane_created"]
    pane: TmuxPaneAddress


class TmuxEventRecordPaneDestroyed(TypedDict):
    "A pane disappeared."

    observed_unix_ns: int
    sequence: int
    kind: Literal["pane_destroyed"]
    pane: TmuxPaneAddress


class TmuxEventRecordPaneModeChanged(TypedDict):
    "A pane entered, changed, or left a tmux mode such as copy mode."

    observed_unix_ns: int
    sequence: int
    kind: Literal["pane_mode_changed"]
    mode: str | None
    pane: TmuxPaneAddress


class TmuxEventRecordPaneMoved(TypedDict):
    "A pane moved into another window."

    observed_unix_ns: int
    sequence: int
    kind: Literal["pane_moved"]
    pane: TmuxPaneAddress


class TmuxEventRecordPaneTrackCompleted(TypedDict):
    "A linked pane recording published canonical completion."

    observed_unix_ns: int
    sequence: int
    kind: Literal["pane_track_completed"]
    pane_id: str
    session_id: str


class TmuxEventRecordPaneTrackLinked(TypedDict):
    "A pane recording incarnation was linked."

    observed_unix_ns: int
    sequence: int
    kind: Literal["pane_track_linked"]
    track: TmuxPaneTrack


class TmuxEventRecordRecordingCompleted(TypedDict):
    "Every track completed; no further events follow."

    observed_unix_ns: int
    sequence: int
    kind: Literal["recording_completed"]


class TmuxEventRecordRecordingStarted(TypedDict):
    "The aggregate identity was created."

    observed_unix_ns: int
    sequence: int
    coverage: TmuxRecordingCoverage
    kind: Literal["recording_started"]


class TmuxEventRecordRecordingStopping(TypedDict):
    "A stop request was committed."

    observed_unix_ns: int
    sequence: int
    kind: Literal["recording_stopping"]


class TmuxEventRecordServerIdentityBound(TypedDict):
    "The first reachable server identity was bound."

    observed_unix_ns: int
    sequence: int
    identity: TmuxServerIdentity
    kind: Literal["server_identity_bound"]


class TmuxEventRecordSessionCreated(TypedDict):
    "A tmux session appeared."

    observed_unix_ns: int
    sequence: int
    kind: Literal["session_created"]
    session_id: str


class TmuxEventRecordSessionDestroyed(TypedDict):
    "A tmux session disappeared."

    observed_unix_ns: int
    sequence: int
    kind: Literal["session_destroyed"]
    session_id: str


class TmuxEventRecordWindowCreated(TypedDict):
    "A tmux window appeared."

    observed_unix_ns: int
    sequence: int
    kind: Literal["window_created"]
    window_id: str


class TmuxEventRecordWindowDestroyed(TypedDict):
    "A tmux window disappeared."

    observed_unix_ns: int
    sequence: int
    kind: Literal["window_destroyed"]
    window_id: str


class TmuxEventRecordZoomChanged(TypedDict):
    "A pane's window was zoomed or unzoomed."

    observed_unix_ns: int
    sequence: int
    kind: Literal["zoom_changed"]
    pane: TmuxPaneAddress
    zoomed: bool


class TmuxEvents(TypedDict):
    "One aggregate history page with the manifest it belongs to."

    manifest: TmuxRecordingManifest
    page: EventPage


class TmuxFailureFacts(TypedDict):
    "Actual tmux progress receipts, independent of the actor mutation/retry protocol."

    action: str
    completed_track_stops: list[Base64Bytes]
    completed_track_waits: list[Base64Bytes]
    integration_disable_attempted: bool
    manifest: Base64Bytes | None
    pane_shutdown_attempted: list[str]
    phase: TmuxFailurePhase
    stop_requested: bool | None


TmuxFailurePhase: TypeAlias = "Literal['inspect_state', 'stop_requested', 'composite_stop_completed', 'waiting_tracks', 'settling']"


class TmuxGapCatalogEntry(TypedDict):
    "One historical coverage interval from the lifecycle catalog."

    gap: TmuxCoverageGap
    sequence: int


TmuxGapKind: TypeAlias = "Literal['polling_interval', 'client_overlay_index', 'observer_start', 'uncovered_pane']"


TmuxGhosttyProfile: TypeAlias = "Literal['auto', 'required', 'off']"


class TmuxJournalBoundary(TypedDict):
    "Committed prefix of the append-only lifecycle journal beside the snapshot."

    bytes: int
    hash: str
    sequence: int
    version: int


class TmuxPaneAddress(TypedDict):
    "Stable tmux object identifiers of one pane at observation time."

    pane_id: str
    session_id: str
    window_id: str


class TmuxPaneSnapshot(TypedDict):
    "Latest supervisor observation for one tmux pane."

    copy_mode: bool
    layout: str
    pane_active: bool
    pane_id: str
    pane_mode: str
    recording_session: Base64Bytes | None
    session_id: str
    session_name: str
    window_active: bool
    window_id: str
    window_index: int
    zoomed: bool


class TmuxPaneStatus(TypedDict):
    "One pane in a status report."

    current_command: Base64Bytes
    pane_id: str
    pane_pid: int | None
    recorded: bool
    recording_session: Base64Bytes | None
    selected: bool
    session_id: str
    window_id: str


class TmuxPaneTrack(TypedDict):
    "One immutable pane recording incarnation linked to an aggregate."

    linked_unix_ns: int
    pane_id: str
    session: Base64Bytes
    session_id: str


class TmuxRecordConfiguration(TypedDict):
    "Aggregate start settings. Exact bytes identify command arguments, targets, and paths."

    cell_height: int
    cell_width: int
    command: list[Base64Bytes]
    coverage: TmuxCoverage
    explicit_only: bool
    ghostty_config: Base64Bytes | None
    ghostty_profile: TmuxGhosttyProfile
    launch_hook: bool
    name: str
    new_server: bool
    panes: list[Base64Bytes]
    retain_bytes: int | None
    retain_days: int | None
    sessions: list[Base64Bytes]
    term: str | None
    windows: list[Base64Bytes]


class TmuxRecordConfigurationInput(TypedDict):
    "Aggregate start settings. Exact bytes identify command arguments, targets, and paths."

    cell_height: NotRequired[int]
    cell_width: NotRequired[int]
    command: NotRequired[list[Base64Bytes]]
    coverage: NotRequired[TmuxCoverage]
    explicit_only: NotRequired[bool]
    ghostty_config: NotRequired[Base64Bytes | None]
    ghostty_profile: NotRequired[TmuxGhosttyProfile]
    launch_hook: NotRequired[bool]
    name: NotRequired[str]
    new_server: NotRequired[bool]
    panes: NotRequired[list[Base64Bytes]]
    retain_bytes: NotRequired[int | None]
    retain_days: NotRequired[int | None]
    sessions: NotRequired[list[Base64Bytes]]
    term: NotRequired[str | None]
    windows: NotRequired[list[Base64Bytes]]


TmuxRecordingCoverage: TypeAlias = "Literal['warn', 'strict']"


TmuxRecordingLifecycle: TypeAlias = (
    "Literal['starting', 'live', 'stopping', 'completed']"
)


class TmuxRecordingManifest(TypedDict):
    "Aggregate identity and current state; complete history lives in the lifecycle journal.\n\nThe snapshot holds no history: events, resolved gaps, and every track incarnation are\nread through the journal-backed catalog."

    catalog: TmuxCatalogSummary
    composite_track: TmuxCompositeTrack
    coverage: TmuxRecordingCoverage
    coverage_gaps: list[TmuxCoverageGap]
    created_unix_ns: int
    format: str
    lifecycle: TmuxRecordingLifecycle
    lifecycle_journal: TmuxJournalBoundary
    name: str
    pane_tracks: dict[str, Base64Bytes]
    panes: list[TmuxPaneSnapshot]
    recording_id: str
    server: TmuxServerIdentity | None
    socket: TmuxSocket
    tmux_executable: Base64Bytes
    updated_unix_ns: int
    version: int


class TmuxRecordingStart(TypedDict):
    "Aggregate admission result with an exact binding to its persisted manifest."

    manifest: Base64Bytes
    manifest_display: str
    recording: TmuxRecordingManifest


TmuxScopeMode: TypeAlias = "Literal['all', 'explicit', 'selected']"


class TmuxScopePolicy(TypedDict):
    "Versioned recording scope stored in a global tmux option."

    mode: TmuxScopeMode
    pane_ids: list[str]
    session_ids: list[str]
    session_names: list[Base64Bytes]
    version: int
    window_ids: list[str]
    window_names: list[Base64Bytes]


class TmuxServerIdentity(TypedDict):
    "Immutable tmux server identity bound by the first successful observation."

    pid: int
    socket_device: int
    socket_inode: int
    socket_path: Base64Bytes


class TmuxSessionLink(TypedDict):
    "Immutable identity and exact path of one canonical Termtrace session."

    session: Base64Bytes
    session_id: str


TmuxSocket: TypeAlias = "Literal['default'] | TmuxSocketName | TmuxSocketPath"


class TmuxSocketName(TypedDict):
    "A socket name, equivalent to `tmux -L`."

    name: Base64Bytes


class TmuxSocketPath(TypedDict):
    "A socket path, equivalent to `tmux -S`."

    path: Base64Bytes


class TmuxStatusReport(TypedDict):
    "Integration and coverage status of one tmux server."

    configured_executable: Base64Bytes | None
    default_command_matches: bool
    enabled: bool
    excluded_panes: int
    integration_version: int
    observers: Observers | None
    output_root: Base64Bytes | None
    pane_count: int
    panes: list[TmuxPaneStatus]
    recorded_panes: int
    scope: TmuxScopePolicy | None
    selected_panes: int
    strict: bool
    unrecorded_panes: int


class TmuxStreamingBundleManifest(TypedDict):
    "Bounded manifest for an aggregate export with a streamed track inventory."

    aggregate_manifest: Base64Bytes
    aggregate_manifest_blake3: str
    format: str
    recording_id: str
    recording_name: str
    track_count: int
    track_inventory_blake3: str
    track_inventory_bytes: int
    version: int


class TmuxTrackCatalogEntry(TypedDict):
    "One bounded-page result from the complete recording inventory."

    completed: bool
    sequence: int
    track: TmuxPaneTrack


TruthLayer: TypeAlias = "Literal['semantic', 'native']"


class ValueChange(TypedDict):
    "Before/after values for one changed semantic property."

    after: CursorState
    before: CursorState


class ValueChange2(TypedDict):
    "Before/after values for one changed semantic property."

    after: tuple[int, int]
    before: tuple[int, int]


class ValueChange3(TypedDict):
    "Before/after values for one changed semantic property."

    after: ModeState
    before: ModeState


class ValueChange4(TypedDict):
    "Before/after values for one changed semantic property."

    after: ColorState
    before: ColorState


class ValueChange5(TypedDict):
    "Before/after values for one changed semantic property."

    after: CellStyle
    before: CellStyle


class ValueChange6(TypedDict):
    "Before/after values for one changed semantic property."

    after: TerminalMetadata
    before: TerminalMetadata


class ValueChange7(TypedDict):
    "Before/after values for one changed semantic property."

    after: bool
    before: bool


class ValueChange8(TypedDict):
    "Before/after values for one changed semantic property."

    after: bool | None
    before: bool | None


class VerificationFinding(TypedDict):
    "One stable verification diagnostic."

    code: str
    message: str


VerificationLevel: TypeAlias = (
    "Literal['structure', 'content', 'durability_record', 'signature']"
)


class VerificationReport(TypedDict):
    "Machine-readable result from offline session verification."

    artifact_authenticity_established: bool
    artifact_roots_verified: int
    final_event_hash: str | None
    findings: list[VerificationFinding]
    format: str
    level: VerificationLevel
    mutation_journal_error: str | None
    mutation_journal_records_verified: int
    mutation_journal_status: MutationJournalStatus
    output_page: NotRequired[OutputPage | None]
    session: ExactPath
    session_authenticity_established: bool
    session_id: str | None
    session_manifest_hash: str | None
    signature_cryptographic_valid: bool
    signature_receipt_present: bool
    signature_signer_key_id: str | None
    signature_signer_trusted: bool
    valid: bool
    version: int


class Versions(TypedDict):
    "Build and component versions of this Termtrace library."

    control_protocol: str
    ghostty_revision: str
    target_arch: str
    target_os: str
    termtrace: str


class VideoOptions(TypedDict):
    "SDK options for a video of an explicit edit."

    fps: int | None
    gif_loop: GifLoop | None
    overwrite: bool
    preview: PreviewOptions | None
    renderer: VideoRenderer
    semantic_raster_profile: SemanticRasterProfile | None


class VideoPreview(TypedDict):
    "A rendered preview and its plan."

    contact_sheet: ExactPath
    directory: ExactPath
    plan: PresentationPlan
    video: ExactPath


class VideoRender(TypedDict):
    "A published video, its plan, and its manifested sidecar."

    artifact: MediaArtifactResult | None
    output: ExactPath
    plan: PresentationPlan
    sidecar: ExactPath


class VideoRenderConfig(TypedDict):
    "Effective user-facing video facade settings."

    cursor_style: str
    end_marker: str | None
    fps: int
    hold_end_ms: int
    hold_start_ms: int
    idle_threshold_ms: int | None
    preset: str
    renderer: str
    resolution: list[int] | None
    scale: int | None
    start_marker: str | None
    trim_idle: bool


class VideoRenderConfigInput(TypedDict):
    "Effective user-facing video facade settings."

    cursor_style: str
    end_marker: NotRequired[str | None]
    fps: int
    hold_end_ms: int
    hold_start_ms: int
    idle_threshold_ms: NotRequired[int | None]
    preset: str
    renderer: str
    resolution: NotRequired[list[int] | None]
    scale: NotRequired[int | None]
    start_marker: NotRequired[str | None]
    trim_idle: bool


VideoRenderer: TypeAlias = (
    "Literal['native', 'semantic', 'native_replay', 'original_execution']"
)


class VisualProfile(TypedDict):
    "Immutable visual configuration selected for one launch.\n\nThe native section configures Ghostty for native pixels. The semantic section selects the\nbundled semantic renderer and never changes native configuration."

    native: NotRequired[NativeVisualProfile | None]
    semantic: NotRequired[SemanticVisualProfile | None]
    version: int


VisualProfileFailure: TypeAlias = "Literal['invalid_visual_profile', 'native_profile_unavailable', 'ghostty_config_invalid', 'font_unavailable', 'viewport_mismatch', 'native_profile_failed']"


class VisualProfileReport(TypedDict):
    "Result of validating a visual profile before launch.\n\nThe report holds settings and resource identities only, never local paths or timestamps.\nIt is advisory: a later launch resolves the profile again."

    guarantees: list[str]
    limitations: list[str]
    native: NativeProfileReport | None
    profile: VisualProfile
    profile_hash: str
    semantic: SemanticImageResource | None
    version: int


class VtSourceSegment(TypedDict):
    "Half-open byte range within one canonical output event."

    end_byte_offset: int
    event_seq: int
    monotonic_ns: int
    start_byte_offset: int


class VtToken(TypedDict):
    "One lossless VT frame with exact canonical source ranges."

    description: str
    kind: VtTokenKind
    raw: Base64Bytes
    source: list[VtSourceSegment]
    timing_resolution: str
    token_id: int


VtTokenKind: TypeAlias = "Literal['printable', 'c0', 'c1', 'esc', 'csi', 'osc', 'dcs', 'apc', 'unknown', 'incomplete']"


class WaitOutcome(TypedDict):
    "Successful raw-output wait evidence."

    event_seq: int
    monotonic_ns: int
    output_end_offset: int


WideKind: TypeAlias = "Literal['narrow', 'wide', 'spacer_tail', 'spacer_head']"


class WindowPadding(TypedDict):
    "Solver-selected Ghostty window padding in points; zero leaves the profile's own padding in\neffect."

    x: int
    y: int
